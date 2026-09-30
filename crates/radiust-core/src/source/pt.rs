//! Native discovery adapter for Portugal's IPMA Madeira radar timeline.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;
use url::Url;

const INDEX_URL: &str = "https://www.ipma.pt/resources.www/transf/radar/imgs-radar-md.json";
const IMAGE_BASE: &str = "https://www.ipma.pt/resources.www/transf/radar/mad/";
const IMAGE_PATH_PREFIX: &str = "/resources.www/transf/radar/mad/";
const REFERER: &str = "https://www.ipma.pt/pt/otempo/obs.remote/index-md.jsp";
const PRODUCT: &str = "composite";
const STATION: &str = "PTST2";
const LOCATOR_VERSION: &str = "pt-legacy-v1";

/// Discovers image references from IPMA's public Madeira timeline.
///
/// This adapter only migrates discovery. It does not provide a native image
/// decoder or claim that the scientific decoding path has migrated.
pub struct PtSourceAdapter;

impl SourceAdapter for PtSourceAdapter {
    fn source_id(&self) -> &'static str {
        "pt"
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case("www.ipma.pt")
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target(&target, &context.query)?;
            if !context.query.stations.is_empty()
                && !context.query.stations.iter().any(|station| station == STATION)
            {
                return Ok(Vec::new());
            }
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source pt discovery requires network access".into(),
                ));
            }

            let payload = context
                .http_transport
                .get_bytes_with_headers(INDEX_URL, &[("Referer", REFERER)])
                .await
                .map_err(sanitize_transport_error)?;
            let frames = parse_timeline(&payload)?;
            filter_frames(frames, &context.query.selector)
        })
    }
}

fn validate_target(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != "pt"
        || query.source.as_deref().is_some_and(|source| source != "pt" && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == "pt"))
    {
        return Err(CoreError::Transport("source pt received an invalid target".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source pt only supports the composite product".into()));
    }
    if target.station.as_deref().is_some_and(|station| station != STATION) {
        return Err(CoreError::Transport("source pt only supports the PTST2 station".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source pt does not expose base times".into()));
    }
    Ok(())
}

fn sanitize_transport_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source pt timeline request requires network access".into())
        }
        _ => CoreError::Transport("source pt timeline request failed".into()),
    }
}

fn parse_timeline(payload: &[u8]) -> CoreResult<Vec<FrameRef>> {
    let document: Value = serde_json::from_slice(payload)
        .map_err(|_| CoreError::Transport("source pt returned invalid timeline JSON".into()))?;
    let Some(entries) =
        document.as_object().and_then(|object| object.get("Madeira")).and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };

    let mut frames = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let (Some(path), Some(raw_time)) =
            (entry.get("path").and_then(Value::as_str), entry.get("date").and_then(Value::as_str))
        else {
            continue;
        };
        if raw_time.len() != 16 {
            continue;
        }
        let Some(url) = safe_image_url(path) else {
            continue;
        };
        let Ok(valid_time) = NaiveDateTime::parse_from_str(raw_time, "%Y-%m-%d %H:%M") else {
            continue;
        };
        frames.push(frame_from_entry(path, url, valid_time.and_utc())?);
    }
    Ok(frames)
}

fn safe_image_url(path: &str) -> Option<String> {
    // IPMA supplies a single image filename. Restricting it to this small
    // character set excludes absolute URLs, path separators, escapes, query
    // strings, fragments, and traversal segments before URL joining.
    if path.is_empty()
        || path.starts_with('.')
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return None;
    }

    let base = Url::parse(IMAGE_BASE).ok()?;
    let joined = base.join(path).ok()?;
    let remainder = joined.path().strip_prefix(IMAGE_PATH_PREFIX)?;
    if joined.scheme() != "https"
        || joined.host_str() != Some("www.ipma.pt")
        || joined.port().is_some()
        || !joined.username().is_empty()
        || joined.password().is_some()
        || joined.query().is_some()
        || joined.fragment().is_some()
        || remainder.is_empty()
        || remainder.contains('/')
    {
        return None;
    }
    Some(joined.into())
}

fn frame_from_entry(path: &str, url: String, valid_time: DateTime<Utc>) -> CoreResult<FrameRef> {
    let revision = path.strip_suffix(".png").unwrap_or(path).to_owned();
    let mut frame = FrameRef {
        source: "pt".into(),
        product: PRODUCT.into(),
        station: Some(STATION.into()),
        valid_time: valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(revision.clone()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": url,
            "artifacts": [],
            "station": STATION,
            "revision": revision,
            "headers": {"Referer": REFERER},
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source pt frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn filter_frames(mut frames: Vec<FrameRef>, selector: &TimeSelector) -> CoreResult<Vec<FrameRef>> {
    match selector {
        TimeSelector::Latest => {
            if let Some(latest) =
                frames.iter().map(|frame| frame.valid_time.as_str()).max().map(str::to_owned)
            {
                frames.retain(|frame| frame.valid_time == latest);
            }
        }
        TimeSelector::At { time } => {
            let time = normalize_selector_time(time)?;
            frames.retain(|frame| frame.valid_time == time);
        }
        TimeSelector::Range { start, end } => {
            let start = normalize_selector_time(start)?;
            let end = normalize_selector_time(end)?;
            if start >= end {
                frames.clear();
            } else {
                frames.retain(|frame| frame.valid_time >= start && frame.valid_time < end);
            }
        }
    }
    frames.sort_by(|left, right| {
        left.valid_time
            .cmp(&right.valid_time)
            .then_with(|| left.station.cmp(&right.station))
            .then_with(|| left.logical_id.cmp(&right.logical_id))
    });
    Ok(frames)
}

fn normalize_selector_time(value: &str) -> CoreResult<String> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        CoreError::Transport("source pt query contains an invalid timestamp".into())
    })?;
    Ok(parsed.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Micros, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;

    #[test]
    fn accepts_all_and_explicit_multi_source_queries_but_rejects_mismatches() {
        let target =
            DiscoveryTarget { source: "pt".into(), product: Some(PRODUCT.into()), station: None };
        for query in [
            Query { source: Some("all".into()), ..Query::default() },
            Query { sources: vec!["nz".into(), "pt".into()], ..Query::default() },
        ] {
            validate_target(&target, &query).unwrap();
        }
        for query in [
            Query { source: Some("nz".into()), ..Query::default() },
            Query { sources: vec!["nz".into()], ..Query::default() },
        ] {
            assert!(validate_target(&target, &query).is_err());
        }
    }

    const TIMELINE: &[u8] = br#"{
        "Madeira": [
            {"date":"2026-09-18 02:50","path":"pcr_pst-2026-09-18T0250.png"},
            {"date":"2026-09-18 03:00","path":"pcr_pst-2026-09-18T0300.png"},
            {"date":"2026-09-18 02:55","path":"../outside.png"},
            {"date":"2026-09-18 02:55","path":"https://attacker.invalid/frame.png"},
            {"date":"2026-09-18 02:55:00","path":"seconds-are-not-supported.png"},
            {"path":"missing-date.png"},
            null
        ]
    }"#;

    #[test]
    fn parses_madeira_entries_and_matches_python_logical_id_fixture() {
        let frames = parse_timeline(TIMELINE).unwrap();
        assert_eq!(frames.len(), 2);
        let frame = &frames[0];

        assert_eq!(frame.source, "pt");
        assert_eq!(frame.product, PRODUCT);
        assert_eq!(frame.station.as_deref(), Some(STATION));
        assert_eq!(frame.valid_time, "2026-09-18T02:50:00.000000Z");
        assert_eq!(frame.revision.as_deref(), Some("pcr_pst-2026-09-18T0250"));
        assert_eq!(frame.locator_version, LOCATOR_VERSION);
        assert_eq!(
            frame.locator,
            json!({
                "url": "https://www.ipma.pt/resources.www/transf/radar/mad/pcr_pst-2026-09-18T0250.png",
                "artifacts": [],
                "station": STATION,
                "revision": "pcr_pst-2026-09-18T0250",
                "headers": {"Referer": REFERER},
            })
        );

        let identity = frame_identity(frame).unwrap();
        assert!(identity["locator"].get("url").is_none());
        assert!(identity["locator"].get("headers").is_none());
        assert_eq!(identity["locator"]["artifacts"], json!([]));
        assert_eq!(identity["locator"]["station"], STATION);
        assert_eq!(identity["locator"]["revision"], "pcr_pst-2026-09-18T0250");
        // Golden logical_id computed by python/radiust.models.FrameRef.logical_id
        // for the matching LegacyImageSource locator.
        assert_eq!(
            frame.logical_id,
            "9db7673c68de6c0265af218af359e0d1fc6b83b7ef98b20670ea739b45fb73ff"
        );
        assert_eq!(frame.logical_id, logical_id(frame).unwrap());
    }

    #[test]
    fn latest_at_and_half_open_range_select_utc_minute_frames() {
        let frames = parse_timeline(TIMELINE).unwrap();
        let latest = filter_frames(frames.clone(), &TimeSelector::Latest).unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].valid_time, "2026-09-18T03:00:00.000000Z");

        let at = filter_frames(
            frames.clone(),
            &TimeSelector::At { time: "2026-09-18T04:50:00+02:00".into() },
        )
        .unwrap();
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].valid_time, "2026-09-18T02:50:00.000000Z");

        let range = filter_frames(
            frames,
            &TimeSelector::Range {
                start: "2026-09-18T02:50:00Z".into(),
                end: "2026-09-18T03:00:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 1);
        assert_eq!(range[0].valid_time, "2026-09-18T02:50:00.000000Z");
    }

    #[test]
    fn safe_image_urls_remain_under_the_ipma_madeira_prefix() {
        assert_eq!(
            safe_image_url("pcr_pst-2026-09-18T0250.png").as_deref(),
            Some("https://www.ipma.pt/resources.www/transf/radar/mad/pcr_pst-2026-09-18T0250.png")
        );
        for path in [
            "../outside.png",
            "%2e%2e/outside.png",
            "/outside.png",
            "\\\\attacker\\frame.png",
            "https://attacker.invalid/frame.png",
            "frame.png?download=1",
            "frame.png#fragment",
            ".hidden.png",
            "",
        ] {
            assert!(safe_image_url(path).is_none(), "accepted unsafe IPMA path: {path}");
        }
    }

    #[test]
    fn invalid_or_incomplete_timeline_data_is_empty_or_skipped() {
        assert!(parse_timeline(br#"{"other":[]}"#).unwrap().is_empty());
        assert!(parse_timeline(br#"[]"#).unwrap().is_empty());
        assert!(
            parse_timeline(b"not-json").unwrap_err().to_string().contains("invalid timeline JSON")
        );
        assert!(
            parse_timeline(br#"{"Madeira":[{"date":42,"path":"frame.png"}]}"#).unwrap().is_empty()
        );
    }
}
