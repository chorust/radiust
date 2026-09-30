//! Native discovery and raw tile acquisition for RainViewer.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{
    ArtifactReceipt, DiscoveryTarget, FrameRef, Query, RawArtifact, RawFrame, TimeSelector,
    parse_utc_time,
};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use url::Url;

const SOURCE: &str = "rainviewer";
const PRODUCT: &str = "composite";
const API_URL: &str = "https://api.rainviewer.com/public/weather-maps.json";
const TILE_ORIGIN: &str = "https://tilecache.rainviewer.com";
const TILE_HOST: &str = "tilecache.rainviewer.com";
pub(crate) const TILE_SIZE: u32 = 512;
pub(crate) const ZOOM: u32 = 1;
const COLOR_SCHEME: u32 = 2;
const TILE_OPTIONS: &str = "0_0";
const LOCATOR_VERSION: &str = "rainviewer-v2";
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

pub struct RainViewerSourceAdapter;

impl SourceAdapter for RainViewerSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(TILE_HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        let Ok(plan) = frame_plan(frame) else {
            return false;
        };
        (0..(1 << ZOOM)).any(|y| {
            (0..(1 << ZOOM)).any(|x| {
                url.as_str() == tile_url(&plan.path, x, y)
                    && url.host_str().is_some_and(|host| self.allows_artifact_host(host))
            })
        })
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target_and_query(&target, &context.query)?;
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(format!(
                    "{SOURCE} discovery requires network access"
                )));
            }
            if context.request_budget.cancellation.is_cancelled() {
                return Err(CoreError::Cancelled);
            }

            let payload = context
                .http_transport
                .get_bytes_coalesced(API_URL, &[], &context.request_coalescer)
                .await?;
            let (host, candidates) = parse_manifest(&payload)?;
            select_frames(&host, candidates, &context.query, Utc::now())
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
                return Err(CoreError::NetworkDisabled(format!(
                    "{SOURCE} acquisition requires network access"
                )));
            }
            let plan = frame_plan(&frame)?;
            let mut artifacts = Vec::with_capacity(((1 << ZOOM) * (1 << ZOOM)) as usize);
            let mut total_bytes = 0_u64;

            for y in 0..(1 << ZOOM) {
                for x in 0..(1 << ZOOM) {
                    if context.request_budget.cancellation.is_cancelled() {
                        return Err(CoreError::Cancelled);
                    }
                    let address = tile_url(&plan.path, x, y);
                    let parsed_url = Url::parse(&address).map_err(|_| invalid_frame())?;
                    if !self.allows_artifact_url(&frame, &parsed_url) {
                        return Err(invalid_frame());
                    }

                    let name = format!("tile-z{ZOOM}-x{x}-y{y}.png");
                    let destination = temp_root.join(format!("{}.png", uuid::Uuid::new_v4()));
                    let remaining_frame_bytes =
                        context.limits.max_frame_bytes.saturating_sub(total_bytes);
                    let response_limit =
                        context.limits.max_artifact_bytes.min(remaining_frame_bytes);
                    let receipt = context
                        .http_transport
                        .get_to_path_same_origin_limited_with_headers(
                            &address,
                            &[],
                            &destination,
                            response_limit,
                        )
                        .await?;
                    let path = match tempfile::TempPath::try_from_path(destination.clone()) {
                        Ok(path) => path,
                        Err(_) => {
                            let _ = std::fs::remove_file(destination);
                            return Err(CoreError::Temporary(
                                "RainViewer tile could not be retained".into(),
                            ));
                        }
                    };
                    validate_png_file(&path).await?;
                    if context.request_budget.cancellation.is_cancelled() {
                        return Err(CoreError::Cancelled);
                    }

                    total_bytes = total_bytes.saturating_add(receipt.size_bytes);
                    context.limits.validate_bytes(receipt.size_bytes, total_bytes)?;
                    artifacts.push(RawArtifact {
                        receipt: ArtifactReceipt {
                            name,
                            media_type: "image/png".into(),
                            size_bytes: receipt.size_bytes,
                            sha256: receipt.sha256,
                        },
                        path,
                    });
                }
            }

            Ok(RawFrame { frame, artifacts, private_locator: None })
        }))
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    valid_time: DateTime<Utc>,
    path: String,
}

#[derive(Clone, Debug)]
struct FramePlan {
    path: String,
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    let selects_source =
        query.source.as_deref().is_none_or(|source| source == SOURCE || source == "all")
            && (query.sources.is_empty() || query.sources.iter().any(|source| source == SOURCE));
    if target.source != SOURCE || !selects_source {
        return Err(CoreError::Transport(format!("{SOURCE} received an invalid discovery target")));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport(format!("{SOURCE} only supports the {PRODUCT} product")));
    }
    if target.station.is_some() || !query.stations.is_empty() {
        return Err(CoreError::Transport(format!("{SOURCE} does not expose station selection")));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport(format!("{SOURCE} does not expose base times")));
    }
    Ok(())
}

fn parse_manifest(payload: &[u8]) -> CoreResult<(String, Vec<Candidate>)> {
    let document: Value = serde_json::from_slice(payload)
        .map_err(|_| CoreError::Transport(format!("{SOURCE} manifest is invalid JSON")))?;
    if !document.is_object() {
        return Err(CoreError::Transport(format!("{SOURCE} manifest must be an object")));
    }
    let Some(radar) = document.get("radar").and_then(Value::as_object) else {
        return Ok((TILE_ORIGIN.into(), Vec::new()));
    };

    let host =
        document.get("host").and_then(Value::as_str).and_then(safe_host).ok_or_else(|| {
            CoreError::Transport(format!("{SOURCE} returned an unsafe tile host"))
        })?;
    let Some(past) = radar.get("past") else {
        return Ok((host, Vec::new()));
    };
    let past = past
        .as_array()
        .ok_or_else(|| CoreError::Transport(format!("{SOURCE} radar.past must be an array")))?;

    let mut candidates = Vec::with_capacity(past.len());
    for item in past {
        let Some(item) = item.as_object() else {
            continue;
        };
        let timestamp = item.get("time").and_then(parse_epoch).ok_or_else(|| {
            CoreError::Transport(format!("{SOURCE} returned an invalid frame time"))
        })?;
        let path = item.get("path").and_then(Value::as_str).and_then(safe_frame_path).ok_or_else(
            || CoreError::Transport(format!("{SOURCE} returned an invalid radar frame path")),
        )?;
        candidates.push(Candidate { valid_time: timestamp, path });
    }
    Ok((host, candidates))
}

fn safe_host(value: &str) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    let safe_path = parsed.path().trim_matches('/').is_empty();
    if parsed.scheme() != "https"
        || !parsed.host_str().is_some_and(|host| host.eq_ignore_ascii_case(TILE_HOST))
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !safe_path
    {
        return None;
    }
    Some(TILE_ORIGIN.to_owned())
}

fn safe_frame_path(value: &str) -> Option<String> {
    let value = value.strip_prefix("/v2/radar/")?.trim_end_matches('/');
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(format!("/v2/radar/{value}"))
}

fn parse_epoch(value: &Value) -> Option<DateTime<Utc>> {
    let seconds = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
        .or_else(|| {
            let value = value.as_f64()?;
            (value.is_finite() && value.fract() == 0.0 && value.abs() <= 253_402_300_799.0)
                .then_some(value as i64)
        })?;
    let timestamp = DateTime::from_timestamp(seconds, 0)?;
    (1..=9999).contains(&timestamp.year()).then_some(timestamp)
}

fn select_frames(
    host: &str,
    mut candidates: Vec<Candidate>,
    query: &Query,
    now: DateTime<Utc>,
) -> CoreResult<Vec<FrameRef>> {
    candidates.sort_by_key(|candidate| candidate.valid_time);
    let selected = match &query.selector {
        TimeSelector::Latest => {
            let Some(candidate) = candidates.pop() else {
                return Ok(Vec::new());
            };
            if let Some(max_age) = query.max_age_secs {
                let age_seconds = (now - candidate.valid_time).num_microseconds().unwrap_or(0)
                    as f64
                    / 1_000_000.0;
                if age_seconds >= 0.0 && age_seconds > max_age {
                    return Ok(Vec::new());
                }
            }
            vec![candidate]
        }
        TimeSelector::At { time } => {
            let selected_time = parse_utc_time(time).map_err(|_| {
                CoreError::Transport(format!("{SOURCE} received an invalid at time"))
            })?;
            candidates
                .into_iter()
                .filter(|candidate| candidate.valid_time == selected_time)
                .collect()
        }
        TimeSelector::Range { start, end } => {
            let start = parse_utc_time(start)
                .map_err(|_| CoreError::Transport(format!("{SOURCE} received an invalid range")))?;
            let end = parse_utc_time(end)
                .map_err(|_| CoreError::Transport(format!("{SOURCE} received an invalid range")))?;
            candidates
                .into_iter()
                .filter(|candidate| candidate.valid_time >= start && candidate.valid_time < end)
                .collect()
        }
    };
    selected.into_iter().map(|candidate| frame_from_candidate(host, candidate)).collect()
}

fn frame_from_candidate(host: &str, candidate: Candidate) -> CoreResult<FrameRef> {
    let revision = candidate.path.rsplit('/').next().unwrap_or_default().to_owned();
    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: None,
        valid_time: candidate.valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(revision),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "api_url": API_URL,
            "host": host,
            "path": candidate.path,
            "tile_size": TILE_SIZE,
            "zoom": ZOOM,
            "color": COLOR_SCHEME,
            "options": TILE_OPTIONS,
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport(format!("{SOURCE} frame identity could not be computed"))
    })?;
    Ok(frame)
}

fn frame_plan(frame: &FrameRef) -> CoreResult<FramePlan> {
    let invalid = || invalid_frame();
    if frame.source != SOURCE
        || frame.product != PRODUCT
        || frame.station.is_some()
        || frame.base_time.is_some()
        || frame.locator_version != LOCATOR_VERSION
        || parse_utc_time(&frame.valid_time).is_err()
        || logical_id(frame).ok().as_deref() != Some(frame.logical_id.as_str())
    {
        return Err(invalid());
    }
    let locator = frame.locator.as_object().ok_or_else(invalid)?;
    if locator.get("api_url").and_then(Value::as_str) != Some(API_URL)
        || locator.get("host").and_then(Value::as_str) != Some(TILE_ORIGIN)
        || locator.get("tile_size").and_then(Value::as_u64) != Some(TILE_SIZE as u64)
        || locator.get("zoom").and_then(Value::as_u64) != Some(ZOOM as u64)
        || locator.get("color").and_then(Value::as_u64) != Some(COLOR_SCHEME as u64)
        || locator.get("options").and_then(Value::as_str) != Some(TILE_OPTIONS)
    {
        return Err(invalid());
    }
    let path = locator
        .get("path")
        .and_then(Value::as_str)
        .and_then(safe_frame_path)
        .ok_or_else(invalid)?;
    let revision = path.rsplit('/').next().unwrap_or_default();
    if locator.get("path").and_then(Value::as_str) != Some(path.as_str())
        || frame.revision.as_deref() != Some(revision)
    {
        return Err(invalid());
    }
    Ok(FramePlan { path })
}

pub(crate) fn validate_science_frame(frame: &FrameRef) -> CoreResult<()> {
    frame_plan(frame).map(|_| ())
}

fn tile_url(path: &str, x: u32, y: u32) -> String {
    format!("{TILE_ORIGIN}{path}/{TILE_SIZE}/{ZOOM}/{x}/{y}/{COLOR_SCHEME}/{TILE_OPTIONS}.png")
}

fn invalid_frame() -> CoreError {
    CoreError::Transport(format!("{SOURCE} frame locator is invalid"))
}

async fn validate_png_file(path: &std::path::Path) -> CoreResult<()> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| CoreError::Temporary("RainViewer tile could not be read".into()))?;
    let mut header = [0_u8; 24];
    file.read_exact(&mut header)
        .await
        .map_err(|_| CoreError::Transport("RainViewer tile is not a complete PNG header".into()))?;
    if !valid_png_header(&header) {
        return Err(CoreError::Transport("RainViewer tile PNG header is invalid".into()));
    }
    Ok(())
}

fn valid_png_header(header: &[u8]) -> bool {
    header.len() >= 24
        && &header[..8] == PNG_SIGNATURE
        && header[8..12] == [0, 0, 0, 13]
        && &header[12..16] == b"IHDR"
        && u32::from_be_bytes(header[16..20].try_into().unwrap_or_default()) == TILE_SIZE
        && u32::from_be_bytes(header[20..24].try_into().unwrap_or_default()) == TILE_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use sha2::{Digest, Sha256};

    const FIXTURE: &str =
        include_str!("../../../../tests/fixtures/sources/rainviewer/fixture.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).unwrap()
    }

    fn fixture_manifest() -> (Value, i64) {
        let fixture = fixture();
        let discovery = &fixture["discovery"];
        let timestamp = discovery["frame_time"].as_i64().unwrap();
        let path = discovery["frame_path"].as_str().unwrap();
        let host = discovery["host"].as_str().unwrap();
        let past = json!([
            {"time": timestamp + 300, "path": "/v2/radar/newer"},
            {"time": timestamp, "path": path},
            {"time": timestamp - 300, "path": "/v2/radar/older"},
        ]);
        (
            json!({
                "version": "2.0",
                "generated": discovery["generated"],
                "host": host,
                "radar": {"past": past},
            }),
            timestamp,
        )
    }

    fn candidates_and_host() -> (String, Vec<Candidate>, i64) {
        let (manifest, timestamp) = fixture_manifest();
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let (host, candidates) = parse_manifest(&bytes).unwrap();
        (host, candidates, timestamp)
    }

    fn at_epoch(timestamp: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(timestamp, 0).unwrap()
    }

    #[test]
    fn parses_offline_manifest_and_applies_latest_at_range_and_max_age() {
        let (host, candidates, timestamp) = candidates_and_host();
        assert_eq!(host, TILE_ORIGIN);
        let latest =
            select_frames(&host, candidates.clone(), &Query::default(), at_epoch(timestamp + 400))
                .unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].revision.as_deref(), Some("newer"));

        let at = Query {
            selector: TimeSelector::At { time: at_epoch(timestamp).to_rfc3339() },
            ..Query::default()
        };
        let selected =
            select_frames(&host, candidates.clone(), &at, at_epoch(timestamp + 400)).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].revision.as_deref(), Some("7cc4a10f8d53"));

        let range = Query {
            selector: TimeSelector::Range {
                start: at_epoch(timestamp - 300).to_rfc3339(),
                end: at_epoch(timestamp + 300).to_rfc3339(),
            },
            ..Query::default()
        };
        let selected =
            select_frames(&host, candidates.clone(), &range, at_epoch(timestamp + 400)).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].revision.as_deref(), Some("older"));
        assert_eq!(selected[1].revision.as_deref(), Some("7cc4a10f8d53"));

        let stale = Query { max_age_secs: Some(99.0), ..Query::default() };
        assert!(
            select_frames(&host, candidates.clone(), &stale, at_epoch(timestamp + 400))
                .unwrap()
                .is_empty()
        );
        let boundary = Query { max_age_secs: Some(100.0), ..Query::default() };
        assert_eq!(
            select_frames(&host, candidates, &boundary, at_epoch(timestamp + 400)).unwrap()[0]
                .revision
                .as_deref(),
            Some("newer")
        );
    }

    #[test]
    fn rejects_unsafe_manifest_hosts_and_frame_paths() {
        for host in [
            "http://tilecache.rainviewer.com",
            "https://tilecache.rainviewer.com.evil.test",
            "https://user@tilecache.rainviewer.com",
            "https://tilecache.rainviewer.com:8443",
            "https://tilecache.rainviewer.com/other",
            "https://tilecache.rainviewer.com/?target=evil",
        ] {
            assert!(safe_host(host).is_none(), "accepted unsafe host: {host}");
        }
        for path in [
            "/v2/radar/../other",
            "/v2/radar/a/b",
            "/v2/radar/a?query=1",
            "/v2/radar/a%2fb",
            "/v2/radar/",
        ] {
            assert!(safe_frame_path(path).is_none(), "accepted unsafe path: {path}");
        }

        let (mut manifest, _) = fixture_manifest();
        manifest["host"] = json!("https://tilecache.rainviewer.com.evil.test");
        assert!(parse_manifest(&serde_json::to_vec(&manifest).unwrap()).is_err());
        manifest["host"] = json!(TILE_ORIGIN);
        manifest["radar"]["past"][1]["path"] = json!("/v2/radar/../forged");
        assert!(parse_manifest(&serde_json::to_vec(&manifest).unwrap()).is_err());
    }

    #[test]
    fn product_station_and_base_time_filters_match_python_rules() {
        let target =
            DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None };
        assert!(validate_target_and_query(&target, &Query::default()).is_ok());

        let wrong_product = Query { product: Some("rain".into()), ..Query::default() };
        assert!(validate_target_and_query(&target, &wrong_product).is_err());

        let station_query = Query { stations: vec!["station-1".into()], ..Query::default() };
        assert!(validate_target_and_query(&target, &station_query).is_err());
        let station_target =
            DiscoveryTarget { station: Some("station-1".into()), ..target.clone() };
        assert!(validate_target_and_query(&station_target, &Query::default()).is_err());

        let base_time =
            Query { base_time: Some("2026-09-18T02:00:00Z".into()), ..Query::default() };
        assert!(validate_target_and_query(&target, &base_time).is_err());
    }

    #[test]
    fn tile_url_policy_accepts_only_urls_derived_from_the_frame_locator() {
        let (host, candidates, timestamp) = candidates_and_host();
        let frame = select_frames(
            &host,
            candidates,
            &Query {
                selector: TimeSelector::At { time: at_epoch(timestamp).to_rfc3339() },
                ..Query::default()
            },
            at_epoch(timestamp),
        )
        .unwrap()
        .remove(0);
        let adapter = RainViewerSourceAdapter;
        assert!(adapter.allows_artifact_host(TILE_HOST));
        assert!(adapter.allows_artifact_host("TILECACHE.RAINVIEWER.COM"));
        assert!(!adapter.allows_artifact_host("sub.tilecache.rainviewer.com"));
        assert!(!adapter.allows_artifact_host("tilecache.rainviewer.com.evil.test"));
        for y in 0..2 {
            for x in 0..2 {
                let url = Url::parse(&tile_url("/v2/radar/7cc4a10f8d53", x, y)).unwrap();
                assert!(adapter.allows_artifact_url(&frame, &url));
            }
        }
        for forged in [
            "https://tilecache.rainviewer.com/v2/radar/other/512/1/0/0/2/0_0.png",
            "https://tilecache.rainviewer.com/v2/radar/7cc4a10f8d53/256/1/0/0/2/0_0.png",
            "https://tilecache.rainviewer.com/v2/radar/7cc4a10f8d53/512/1/2/0/2/0_0.png",
            "https://tilecache.rainviewer.com.evil.test/v2/radar/7cc4a10f8d53/512/1/0/0/2/0_0.png",
        ] {
            assert!(!adapter.allows_artifact_url(&frame, &Url::parse(forged).unwrap()));
        }
    }

    #[test]
    fn locator_and_logical_identity_match_the_python_rainviewer_contract() {
        let fixture = fixture();
        let (host, candidates, timestamp) = candidates_and_host();
        let frame = select_frames(
            &host,
            candidates,
            &Query {
                selector: TimeSelector::At { time: at_epoch(timestamp).to_rfc3339() },
                ..Query::default()
            },
            at_epoch(timestamp),
        )
        .unwrap()
        .remove(0);
        assert_eq!(frame.locator_version, "rainviewer-v2");
        assert_eq!(frame.valid_time, "2026-09-18T02:00:00.000000Z");
        assert_eq!(frame.locator, fixture["frames"][0]["locator"]);
        assert_eq!(frame_identity(&frame).unwrap()["locator"], fixture["frames"][0]["locator"]);
        assert_eq!(
            frame.logical_id,
            "8ff48aec2a18cb7264b544aab31e31feac01ff4a445b8ea4f772546719d39346"
        );
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
        assert!(frame_plan(&frame).is_ok());
    }

    #[test]
    fn four_offline_tile_fixtures_match_the_recorded_sha_and_png_geometry() {
        let fixture = fixture();
        let artifact_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sources/rainviewer");
        for artifact in fixture["frames"][0]["artifacts"].as_array().unwrap() {
            let path = artifact_root.join(artifact["path"].as_str().unwrap());
            let bytes = std::fs::read(path).unwrap();
            let digest = hex::encode(Sha256::digest(&bytes));
            assert_eq!(digest, artifact["sha256"].as_str().unwrap());
            assert!(valid_png_header(&bytes[..24]));
        }
    }

    #[tokio::test]
    async fn verified_science_decode_matches_python_reference_for_all_fixture_pixels() {
        let fixture = fixture();
        let (host, candidates, timestamp) = candidates_and_host();
        let frame = select_frames(
            &host,
            candidates,
            &Query {
                selector: TimeSelector::At { time: at_epoch(timestamp).to_rfc3339() },
                ..Query::default()
            },
            at_epoch(timestamp),
        )
        .unwrap()
        .remove(0);
        let fixture_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sources/rainviewer");
        let staging = tempfile::tempdir().unwrap();
        let artifacts = fixture["frames"][0]["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, artifact)| {
                let source = fixture_root.join(artifact["path"].as_str().unwrap());
                let destination = staging.path().join(format!("tile-{index}.png"));
                std::fs::copy(source, &destination).unwrap();
                RawArtifact {
                    receipt: ArtifactReceipt {
                        name: artifact["name"].as_str().unwrap().to_owned(),
                        media_type: "image/png".into(),
                        size_bytes: std::fs::metadata(&destination).unwrap().len(),
                        sha256: artifact["sha256"].as_str().unwrap().to_owned(),
                    },
                    path: tempfile::TempPath::try_from_path(destination).unwrap(),
                }
            })
            .collect();
        let raw = RawFrame { frame, artifacts, private_locator: None };

        let engine = crate::engine::Engine::new(
            crate::config::CoreConfig::default(),
            crate::source::SourceRegistry::default(),
        )
        .unwrap();
        let field = engine.decode_science(std::sync::Arc::new(raw)).await.unwrap();

        assert_eq!(field.name, "reflectivity");
        assert_eq!(field.units.as_deref(), Some("dBZ"));
        assert_eq!(field.valid_time, "2026-09-18T02:00:00.000000Z");
        assert_eq!(field.shape, [1024, 1024]);
        assert_eq!(field.grid.crs.as_deref(), Some("EPSG:4326"));
        assert_eq!(field.grid.x[576], 22.67578125);
        assert!((field.grid.y[215] - 71.58053179556501).abs() < 1e-12);
        assert_eq!(field.values[215 * 1024 + 576], 10.0);
        assert_eq!(field.quality[215 * 1024 + 576], 0);
        assert_eq!(field.quality.iter().filter(|&&value| value == 0).count(), 18_853);
        assert_eq!(field.quality.iter().filter(|&&value| value == 1).count(), 1_029_723);
        assert_eq!(field.quality.iter().filter(|&&value| value == 4).count(), 0);

        let values_bytes =
            field.values.iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>();
        let quality_bytes =
            field.quality.iter().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>();
        assert_eq!(
            hex::encode(Sha256::digest(values_bytes)),
            "69238bdcfbaafcbadb3dc72f44c101aaf48f9f295adb63fca478340e3111848d"
        );
        assert_eq!(
            hex::encode(Sha256::digest(quality_bytes)),
            "19ab46854829a5fc7c0cd20252fb15dd9803e903c2a660ccc07d3c92a70523f9"
        );
        assert!(
            field.provenance.iter().any(|entry| entry == "decoder=rainviewer-universal-blue-v1")
        );
        field.validate().unwrap();
    }

    #[test]
    fn science_decode_enforces_pixel_budget_and_receipt_digest() {
        let fixture = fixture();
        let (host, candidates, timestamp) = candidates_and_host();
        let frame = select_frames(
            &host,
            candidates,
            &Query {
                selector: TimeSelector::At { time: at_epoch(timestamp).to_rfc3339() },
                ..Query::default()
            },
            at_epoch(timestamp),
        )
        .unwrap()
        .remove(0);
        let fixture_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sources/rainviewer");
        let staging = tempfile::tempdir().unwrap();
        let artifacts = fixture["frames"][0]["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, artifact)| {
                let source = fixture_root.join(artifact["path"].as_str().unwrap());
                let destination = staging.path().join(format!("tile-{index}.png"));
                std::fs::copy(source, &destination).unwrap();
                RawArtifact {
                    receipt: ArtifactReceipt {
                        name: artifact["name"].as_str().unwrap().to_owned(),
                        media_type: "image/png".into(),
                        size_bytes: std::fs::metadata(&destination).unwrap().len(),
                        sha256: artifact["sha256"].as_str().unwrap().to_owned(),
                    },
                    path: tempfile::TempPath::try_from_path(destination).unwrap(),
                }
            })
            .collect::<Vec<_>>();
        let raw = RawFrame { frame, artifacts, private_locator: None };

        let small_limits =
            crate::limits::Limits { max_pixels: 1_000_000, ..crate::limits::Limits::default() };
        assert!(matches!(
            crate::science::decode_rainviewer(&raw, &small_limits),
            Err(CoreError::ResourceLimit(_))
        ));

        let mut tampered = raw;
        tampered.artifacts[0].receipt.sha256 = "00".repeat(32);
        assert!(matches!(
            crate::science::decode_rainviewer(&tampered, &crate::limits::Limits::default()),
            Err(CoreError::Transport(_))
        ));
    }
}
