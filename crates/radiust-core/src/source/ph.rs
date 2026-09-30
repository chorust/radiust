//! Native PAGASA Hybrid Reflectivity adapter using isolated Chromium sessions.
//! The provider owns session signatures; raw data images remain scientifically unverified.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{ArtifactReceipt, DiscoveryTarget, FrameRef, Query, RawArtifact, RawFrame};
use crate::source::browser::{
    BrowserResponse, ChromiumConfig, ChromiumSession, persist_response_body,
};
use crate::source::{SourceAdapter, SourceContext};
use base64::Engine as _;
use chrono::{DateTime, FixedOffset, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

const SOURCE: &str = "ph";
const PRODUCT: &str = "composite";
const STATION: &str = "PHCOMP4";
const BASE_URL: &str = "https://www.panahon.gov.ph";
const TIMELINE_PATH: &str = "/api/v1/radar/timeline";
const IMAGE_HOST: &str = "www.panahon.gov.ph";
const IMAGE_PATH: &str = "/radar/";
const LOCATOR_VERSION: &str = "ph-http-v1";
const MANILA_OFFSET_SECONDS: i32 = 8 * 60 * 60;

pub struct PhSourceAdapter;

impl SourceAdapter for PhSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(IMAGE_HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        if frame.source != SOURCE
            || frame.product != PRODUCT
            || frame.station.as_deref() != Some(STATION)
            || frame.base_time.is_some()
            || frame.locator_version != LOCATOR_VERSION
            || !self.allows_artifact_host(url.host_str().unwrap_or_default())
            || url.scheme() != "https"
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !safe_image_url(url)
        {
            return false;
        }
        let Some(expected_url) = frame.locator.get("url").and_then(Value::as_str) else {
            return false;
        };
        if url.path() == "/api/v1/radar-data-image"
            && url
                .query_pairs()
                .find(|(key, _)| key == "t")
                .and_then(|(_, value)| value.parse::<i64>().ok())
                != parse_frame_time(frame).map(|time| time.timestamp())
        {
            return false;
        }
        frame.locator.get("revision").and_then(Value::as_str) == frame.revision.as_deref()
            && expected_url == url.as_str()
            && parse_frame_time(frame).is_some()
            && logical_id(frame).ok().as_deref() == Some(frame.logical_id.as_str())
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target_and_query(&target, &context.query)?;
            if !context.query.stations.is_empty()
                && !context.query.stations.iter().any(|station| station == STATION)
            {
                return Ok(Vec::new());
            }
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source ph discovery requires network access".into(),
                ));
            }
            // The public site obtains a session token and signs API requests in
            // its JavaScript. A configured token alone cannot authenticate them.
            let response = fetch_browser_timeline(&context, TIMELINE_PATH).await?;
            ensure_browser_status(response.status, "timeline discovery")?;
            let payload = response.body;
            parse_timeline(&payload)
        })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        context: SourceContext,
        temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        Some(Box::pin(async move {
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source ph acquisition requires network access".into(),
                ));
            }
            let address =
                frame.locator.get("url").and_then(Value::as_str).ok_or_else(|| {
                    CoreError::Transport("source ph frame locator is invalid".into())
                })?;
            let url = Url::parse(address)
                .map_err(|_| CoreError::Transport("source ph frame locator is invalid".into()))?;
            if !self.allows_artifact_url(&frame, &url) {
                return Err(CoreError::Transport("source ph frame locator is invalid".into()));
            }
            let name = frame
                .locator
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("pagasa-radar.png")
                .to_owned();
            let media_type = frame
                .locator
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("image/png")
                .to_owned();
            let max_bytes = context
                .limits
                .max_artifact_bytes
                .min(context.limits.max_frame_bytes)
                .min(context.limits.max_temp_bytes);
            let destination = temp_root.join(format!("{}.browser", Uuid::new_v4()));
            let artifact = if url.path() == "/api/v1/radar-data-image" {
                let response = fetch_browser_image(&context, &temp_root, address).await?;
                ensure_browser_status(response.status, "artifact acquisition")?;
                validate_data_image(&response)?;
                persist_response_body(
                    response.body,
                    &temp_root,
                    name,
                    media_type,
                    &context.limits,
                    0,
                )
                .await?
            } else {
                let receipt = context
                    .http_transport
                    .get_to_path_same_origin_limited_with_headers(
                        address,
                        &[("Referer", BASE_URL)],
                        &destination,
                        max_bytes,
                    )
                    .await
                    .map_err(sanitize_request_error)?;
                let path =
                    tempfile::TempPath::try_from_path(destination.clone()).map_err(|_| {
                        CoreError::Temporary("source ph image could not be retained".into())
                    })?;
                RawArtifact {
                    receipt: ArtifactReceipt {
                        name,
                        media_type,
                        size_bytes: receipt.size_bytes,
                        sha256: receipt.sha256,
                    },
                    path,
                }
            };
            Ok(RawFrame { frame, artifacts: vec![artifact], private_locator: None })
        }))
    }
}

fn browser_config() -> ChromiumConfig {
    ChromiumConfig::new([IMAGE_HOST])
        .with_blocked_urls(&["*/images/*", "*/css/*", "*/fonts/*", "*/js-libs/*", "*/jsons/*"])
        .with_user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.7871.250 Safari/537.36")
}

async fn fetch_browser_timeline(
    context: &SourceContext,
    address: &str,
) -> CoreResult<BrowserResponse> {
    let profile_root = tempfile::Builder::new()
        .prefix("radiust-ph-browser-")
        .tempdir()
        .map_err(|_| CoreError::Temporary("browser temporary directory is unavailable".into()))?;
    fetch_session_response(context, profile_root.path(), address).await
}

async fn fetch_browser_image(
    context: &SourceContext,
    temp_root: &std::path::Path,
    address: &str,
) -> CoreResult<BrowserResponse> {
    fetch_session_response(context, temp_root, address).await
}

async fn fetch_session_response(
    context: &SourceContext,
    root: &std::path::Path,
    address: &str,
) -> CoreResult<BrowserResponse> {
    let mut browser = ChromiumSession::launch(context, root, browser_config()).await?;
    let result = async {
        browser.navigate(BASE_URL).await?;
        let limit = context.limits.max_artifact_bytes.min(context.limits.max_temp_bytes) / 2;
        let address = serde_json::to_string(address).unwrap();
        // Use the site's installed fetch wrapper so its session bootstrap and
        // rotating request-signature recipe remain owned by the provider.
        let expression = format!(r#"(async()=>{{
            // Load only the provider's session-signing bootstrap, independent
            // of its map UI (which requires blocked workers and third-party JS).
            if(!fetch.toString().includes('signable')){{
                const script=Array.from(document.scripts).find(s=>s.src&&new URL(s.src).origin===location.origin&&new URL(s.src).pathname==='/js/beta.js');
                if(!script) throw Error('signing script unavailable');
                const r=await fetch(script.src,{{credentials:'same-origin'}});
                const scriptReader=r.body.getReader();let scriptBytes=0,parts=[];
                while(true){{const x=await scriptReader.read();if(x.done)break;
                    scriptBytes+=x.value.length;if(scriptBytes>{limit}){{await scriptReader.cancel();throw Error('script size limit');}}
                    parts.push(x.value);}}
                const src=await new Blob(parts).text();
                const marker='var n=r(3355);';
                const start=src.indexOf(marker);
                const end=src.indexOf(';var m=r(9561)',start);
                if(!r.ok||start<0||end<0||end-start>100000) throw Error('unsupported signing bootstrap');
                const code=src.slice(start+marker.length,end);
                (0,eval)('(()=>{{const n={{nwpSharesSession:()=>false}};'+code+';g.install();}})()');
            }}
            const u=new URL({address},location.origin);
            if(u.origin!==location.origin) throw Error('origin');
            const token=document.querySelector('meta[name="csrf-token"]')?.content;
            if(!token) throw Error('session unavailable');
            u.searchParams.set('token',token);
            if(u.pathname.endsWith('/timeline')) u.searchParams.set('sublayer','mosaic-reflectivity');
            const r=await fetch(u.href,{{credentials:'same-origin',headers:{{'X-Requested-With':'XMLHttpRequest'}}}});
            const reader=r.body.getReader();let chunks=[],total=0;
            while(true){{const x=await reader.read();if(x.done)break;
                total+=x.value.length;if(total>{limit}){{await reader.cancel();throw Error('size limit');}}
                chunks.push(x.value);}}
            let binary='';for(const chunk of chunks) for(let i=0;i<chunk.length;i+=8192)
                binary+=String.fromCharCode(...chunk.subarray(i,i+8192));
            return {{status:r.status,content_type:r.headers.get('content-type'),body:btoa(binary)}};
        }})()"#);
        let value = browser.evaluate(&expression).await?;
        let body = base64::engine::general_purpose::STANDARD.decode(
            value["body"].as_str().unwrap_or_default())
            .map_err(|_| CoreError::Transport("source ph response encoding is invalid".into()))?;
        Ok(BrowserResponse { status: value["status"].as_u64().unwrap_or(0) as u16,
            content_type: value["content_type"].as_str().map(str::to_owned), body })
    }.await;
    let close = browser.close().await;
    let response = result?;
    close?;
    Ok(response)
}

fn ensure_browser_status(status: u16, stage: &str) -> CoreResult<()> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    if matches!(status, 401 | 403) {
        return Err(CoreError::Transport(format!(
            "source ph browser {stage} was rejected (HTTP {status})"
        )));
    }
    Err(CoreError::Transport(format!("source ph browser {stage} failed (HTTP {status})")))
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source ph received a mismatched source query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source ph only supports the composite product".into()));
    }
    if target.station.as_deref().is_some_and(|station| station != STATION) {
        return Err(CoreError::Transport("source ph only supports the PHCOMP4 station".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source ph does not expose base times".into()));
    }
    Ok(())
}

fn sanitize_request_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source ph discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source ph response exceeds configured limits".into())
        }
        _ => CoreError::Transport("source ph timeline request failed".into()),
    }
}

fn parse_timeline(payload: &[u8]) -> CoreResult<Vec<FrameRef>> {
    let document: Value = serde_json::from_slice(payload).map_err(|_| {
        CoreError::Transport("source ph timeline response is not valid JSON".into())
    })?;
    let timeline = document
        .get("data")
        .and_then(Value::as_object)
        .and_then(|data| data.get("timeline"))
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::Transport("source ph timeline response is invalid".into()))?;
    let mut frames = BTreeMap::<String, FrameRef>::new();
    for item in timeline {
        let Some(item) = item.as_object() else {
            continue;
        };
        let (Some(time), Some(raw_url)) = (
            item.get("observed_at").and_then(Value::as_str),
            item.get("image_url").and_then(Value::as_str),
        ) else {
            continue;
        };
        let Some(frame_time) = parse_provider_time(time) else {
            continue;
        };
        let url = if raw_url.is_empty() {
            let data = &document["data"];
            let version = data["tile_version"].as_u64().filter(|v| *v > 0).ok_or_else(|| {
                CoreError::Transport("source ph image version is unavailable".into())
            })?;
            let mut url = Url::parse(&format!("{BASE_URL}/api/v1/radar-data-image")).unwrap();
            url.query_pairs_mut()
                .append_pair("sublayer", "mosaic-reflectivity")
                .append_pair("t", &frame_time.timestamp().to_string())
                .append_pair("mode", "dbz")
                .append_pair("size", "896")
                .append_pair("v", &version.to_string());
            url
        } else {
            let Ok(url) = Url::parse(raw_url) else {
                continue;
            };
            url
        };
        if !safe_image_url(&url) {
            continue;
        }
        let revision = format!("{STATION}-{}", frame_time.format("%Y%m%d%H%M%S"));
        let name = url
            .path_segments()
            .and_then(Iterator::last)
            .filter(|name| !name.is_empty())
            .unwrap_or("pagasa-radar.png");
        let mut frame = FrameRef {
            source: SOURCE.into(),
            product: PRODUCT.into(),
            station: Some(STATION.into()),
            valid_time: frame_time.to_rfc3339_opts(SecondsFormat::Micros, true),
            base_time: None,
            logical_id: String::new(),
            revision: Some(revision.clone()),
            locator_version: LOCATOR_VERSION.into(),
            locator: json!({
                "url": url.as_str(),
                "name": name,
                "media_type": image_media_type(url.path()),
                "revision": revision,
                "headers": {"Referer": BASE_URL},
                "time_semantics": "provider_frame_time",
                "geometry_status": "unverified",
                "bounds": document["data"]["bounds"],
                "scale": document["data"]["scale"],
                "encoding": if url.path() == "/api/v1/radar-data-image" { "provider_data_image" } else { "legacy_image" },
            }),
        };
        let Ok(identity) = logical_id(&frame) else {
            continue;
        };
        frame.logical_id = identity.clone();
        frames.insert(identity, frame);
    }
    Ok(frames.into_values().collect())
}

fn parse_provider_time(value: &str) -> Option<DateTime<Utc>> {
    let local = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()?;
    let manila = FixedOffset::east_opt(MANILA_OFFSET_SECONDS)?;
    local.and_local_timezone(manila).single().map(|time| time.with_timezone(&Utc))
}

fn parse_frame_time(frame: &FrameRef) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&frame.valid_time).ok().map(|time| time.with_timezone(&Utc))
}

fn safe_image_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| host.eq_ignore_ascii_case(IMAGE_HOST))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && ((url.path().starts_with(IMAGE_PATH) && is_supported_image_path(url.path()))
            || valid_data_image_url(url))
}

fn validate_data_image(response: &BrowserResponse) -> CoreResult<()> {
    if response.content_type.as_deref().and_then(|v| v.split(';').next()) != Some("image/png")
        || response.body.get(..8) != Some(b"\x89PNG\r\n\x1a\n")
    {
        return Err(CoreError::Transport("source ph data image is not PNG".into()));
    }
    let dimensions = image::ImageReader::new(std::io::Cursor::new(&response.body))
        .with_guessed_format()
        .map_err(|_| CoreError::Transport("source ph data image is invalid".into()))?
        .into_dimensions()
        .map_err(|_| CoreError::Transport("source ph data image is invalid".into()))?;
    if dimensions.0 <= 1 || dimensions.1 <= 1 {
        return Err(CoreError::Transport("source ph returned a placeholder data image".into()));
    }
    Ok(())
}

fn valid_data_image_url(url: &Url) -> bool {
    if url.path() != "/api/v1/radar-data-image" {
        return false;
    }
    let pairs: BTreeMap<_, _> = url.query_pairs().collect();
    pairs.len() == 5
        && pairs.get("sublayer").is_some_and(|v| v == "mosaic-reflectivity")
        && pairs.get("mode").is_some_and(|v| v == "dbz")
        && pairs.get("size").is_some_and(|v| v == "896")
        && pairs.get("t").is_some_and(|v| v.parse::<i64>().is_ok())
        && pairs.get("v").is_some_and(|v| v.parse::<u64>().is_ok_and(|v| v > 0))
}

fn is_supported_image_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|suffix| lower.ends_with(suffix))
}

fn image_media_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_skips_bad_rows_and_rejects_bad_shapes() {
        let payload = br#"{"data":{"timeline":[
          {"observed_at":"not-a-time","image_url":"https://www.panahon.gov.ph/radar/a.png"},
          {"observed_at":"2026-09-18 12:00:00","image_url":"https://evil.test/radar/a.png"},
          {"observed_at":"2026-09-18 12:00:00","image_url":"https://www.panahon.gov.ph/radar/good.png"},
          null
        ]}}"#;
        let frames = parse_timeline(payload).unwrap();
        assert_eq!(frames.len(), 1);
        assert!(parse_timeline(br#"{"data":[]}"#).is_err());
        assert!(parse_timeline(b"not json").is_err());
    }

    #[test]
    fn artifact_urls_are_bound_to_the_verified_frame_and_official_path() {
        let frame = parse_timeline(&timeline_payload()).unwrap().remove(0);
        let adapter = PhSourceAdapter;
        let address = Url::parse(frame.locator["url"].as_str().unwrap()).unwrap();
        assert!(adapter.allows_artifact_url(&frame, &address));
        assert!(!adapter.allows_artifact_url(
            &frame,
            &Url::parse("https://www.panahon.gov.ph.evil.test/radar/frame.png").unwrap()
        ));
        assert!(!adapter.allows_artifact_url(
            &frame,
            &Url::parse("https://www.panahon.gov.ph/other/frame.png").unwrap()
        ));
        let mut forged = frame;
        forged.locator["revision"] = Value::String("other".into());
        assert!(!adapter.allows_artifact_url(&forged, &address));
    }

    #[test]
    fn rejects_upstream_placeholder_png_even_with_http_200() {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::new(1, 1));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let response = BrowserResponse {
            status: 200,
            content_type: Some("image/png".into()),
            body: bytes.into_inner(),
        };
        assert!(
            matches!(validate_data_image(&response), Err(CoreError::Transport(message)) if message.contains("placeholder"))
        );
        assert!(
            validate_data_image(&BrowserResponse { body: b"not PNG".to_vec(), ..response })
                .is_err()
        );
    }

    #[test]
    fn current_timeline_builds_token_free_data_image_locators() {
        let payload = br#"{"success":true,"data":{"tile_version":5,"bounds":[115,3,129,22],"scale":{"mode":"dbz"},"timeline":[{"observed_at":"2026-09-30 16:00:00","observed_at_unix":1790755200,"image_url":""}]}}"#;
        let frame = parse_timeline(payload).unwrap().remove(0);
        let url = Url::parse(frame.locator["url"].as_str().unwrap()).unwrap();
        assert!(PhSourceAdapter.allows_artifact_url(&frame, &url));
        assert_eq!(frame.valid_time, "2026-09-30T08:00:00.000000Z");
        assert!(!url.query_pairs().any(|(k, _)| k == "token"));
        let mut forged = url.clone();
        forged.query_pairs_mut().append_pair("token", "secret");
        assert!(!safe_image_url(&forged));
        assert!(
            parse_timeline(
                br#"{"data":{"timeline":[{"observed_at":"2026-09-30 16:00:00","image_url":""}]}}"#
            )
            .is_err()
        );
    }

    fn timeline_payload() -> Vec<u8> {
        br#"{"data":{"timeline":[{"observed_at":"2026-09-18 12:00:00","image_url":"https://www.panahon.gov.ph/radar/202609180400.png"}]}}"#.to_vec()
    }
}
