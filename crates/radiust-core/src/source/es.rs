//! Native discovery adapter for Spain's AEMET national radar composite.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, TimeSelector};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::sync::Arc;

const TIMELINE_URL: &str = "https://www.aemet.es/es/api-eltiempo/radar/timeline/compo/PB";
const IMAGE_BASE: &str = "https://www.aemet.es/es/api-eltiempo/radar/imagen-radar/compo/";
const REFERER: &str = "https://www.aemet.es/es/eltiempo/observacion/radar";
const PRODUCT: &str = "composite";
const STATION: &str = "ESCOMP";
const LOCATOR_VERSION: &str = "es-legacy-v1";

/// Discovers Spain's current AEMET national radar composite frames.
pub struct EsSourceAdapter;

impl SourceAdapter for EsSourceAdapter {
    fn source_id(&self) -> &'static str {
        "es"
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case("www.aemet.es")
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            if target.source != "es" {
                return Err(CoreError::Transport("source es received an invalid target".into()));
            }
            if target.product.as_deref().is_some_and(|product| product != PRODUCT) {
                return Err(CoreError::Transport(
                    "source es only supports the composite product".into(),
                ));
            }
            if target.station.as_deref().is_some_and(|station| station != STATION) {
                return Err(CoreError::Transport(
                    "source es only supports the ESCOMP station".into(),
                ));
            }
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source es discovery requires network access".into(),
                ));
            }

            let payload = context
                .http_transport
                .get_bytes_with_headers(TIMELINE_URL, &[("Referer", REFERER)])
                .await
                .map_err(sanitize_transport_error)?;
            let frames = parse_timeline(&payload)?;
            filter_frames(frames, &context.query.selector)
        })
    }
}

fn sanitize_transport_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        _ => CoreError::Transport("source es timeline request failed".into()),
    }
}

fn parse_timeline(payload: &[u8]) -> CoreResult<Vec<FrameRef>> {
    let document: Value = serde_json::from_slice(payload)
        .map_err(|_| CoreError::Transport("source es returned invalid timeline JSON".into()))?;
    let Some(elements) = document
        .as_array()
        .and_then(|items| items.first())
        .and_then(Value::as_object)
        .and_then(|first| first.get("Elementos"))
        .and_then(Value::as_array)
    else {
        // The Python adapter treats an empty or incomplete timeline as no data.
        return Ok(Vec::new());
    };

    let mut frames = Vec::with_capacity(elements.len());
    for item in elements {
        let Some(item) = item.as_object() else {
            continue;
        };
        let (Some(filename), Some(raw_time)) = (
            item.get("Nombre fichero").and_then(Value::as_str),
            item.get("Fecha").and_then(Value::as_str),
        ) else {
            continue;
        };
        // These provider fields are used as a path suffix. Ignore malformed
        // entries rather than allowing them to change the image URL's origin
        // or path structure.
        if filename.is_empty()
            || filename.chars().any(|character| {
                character.is_control() || matches!(character, '/' | '\\' | '?' | '#')
            })
        {
            continue;
        }
        let Ok(valid_time) = DateTime::parse_from_rfc3339(raw_time) else {
            continue;
        };
        let valid_time =
            valid_time.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Micros, true);
        frames.push(frame_from_entry(filename, valid_time)?);
    }
    Ok(frames)
}

fn frame_from_entry(filename: &str, valid_time: String) -> CoreResult<FrameRef> {
    let mut frame = FrameRef {
        source: "es".into(),
        product: PRODUCT.into(),
        station: Some(STATION.into()),
        valid_time,
        base_time: None,
        logical_id: String::new(),
        revision: Some(filename.strip_suffix(".png").unwrap_or(filename).to_owned()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": format!("{IMAGE_BASE}{filename}"),
            "artifacts": [],
            "station": STATION,
            "headers": {"Referer": REFERER},
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source es frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn filter_frames(mut frames: Vec<FrameRef>, selector: &TimeSelector) -> CoreResult<Vec<FrameRef>> {
    match selector {
        TimeSelector::Latest => Ok(frames
            .drain(..)
            .max_by(|left, right| left.valid_time.cmp(&right.valid_time))
            .into_iter()
            .collect()),
        TimeSelector::At { time } => {
            let time = normalize_selector_time(time)?;
            Ok(frames.into_iter().filter(|frame| frame.valid_time == time).collect())
        }
        TimeSelector::Range { start, end } => {
            let start = normalize_selector_time(start)?;
            let end = normalize_selector_time(end)?;
            if start >= end {
                return Ok(Vec::new());
            }
            Ok(frames
                .into_iter()
                .filter(|frame| frame.valid_time >= start && frame.valid_time < end)
                .collect())
        }
    }
}

fn normalize_selector_time(value: &str) -> CoreResult<String> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        CoreError::Transport("source es query contains an invalid timestamp".into())
    })?;
    Ok(parsed.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Micros, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;

    const TIMELINE: &[u8] = br#"[{"Elementos":[
        {"Nombre fichero":"radw202609171440_3857.png","Fecha":"2026-09-17T16:40:00+02:00"},
        {"Nombre fichero":"radw202609171430_3857.png","Fecha":"2026-09-17T14:30:00Z"},
        {"Nombre fichero":"radw202609171420_3857.png","Fecha":"2026-09-17T14:20:00Z"}
    ]}]"#;

    #[test]
    fn parses_aemet_timeline_and_builds_legacy_identity() {
        let frames = parse_timeline(TIMELINE).unwrap();
        let frame = &frames[0];

        assert_eq!(frame.source, "es");
        assert_eq!(frame.product, "composite");
        assert_eq!(frame.station.as_deref(), Some("ESCOMP"));
        assert_eq!(frame.valid_time, "2026-09-17T14:40:00.000000Z");
        assert_eq!(frame.revision.as_deref(), Some("radw202609171440_3857"));
        assert_eq!(frame.locator_version, "es-legacy-v1");
        assert_eq!(
            frame.locator,
            json!({
                "url": "https://www.aemet.es/es/api-eltiempo/radar/imagen-radar/compo/radw202609171440_3857.png",
                "artifacts": [],
                "station": "ESCOMP",
                "headers": {"Referer": "https://www.aemet.es/es/eltiempo/observacion/radar"},
            })
        );

        let identity = frame_identity(frame).unwrap();
        assert!(identity["locator"].get("url").is_none());
        assert_eq!(identity["locator"]["artifacts"], json!([]));
        assert_eq!(identity["locator"]["station"], "ESCOMP");
        assert_eq!(
            frame.logical_id,
            "6901534f5869bab0398feb87edc64b6bdae97342ac81550c59819058060a9779"
        );
        assert_eq!(frame.logical_id, logical_id(frame).unwrap());
    }

    #[test]
    fn latest_at_and_range_select_by_normalized_utc_time() {
        let frames = parse_timeline(TIMELINE).unwrap();
        let latest = filter_frames(frames.clone(), &TimeSelector::Latest).unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].valid_time, "2026-09-17T14:40:00.000000Z");

        let at = filter_frames(
            frames.clone(),
            &TimeSelector::At { time: "2026-09-17T16:40:00+02:00".into() },
        )
        .unwrap();
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].valid_time, "2026-09-17T14:40:00.000000Z");

        let range = filter_frames(
            frames,
            &TimeSelector::Range {
                start: "2026-09-17T14:20:00Z".into(),
                end: "2026-09-17T14:30:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 1);
        assert_eq!(range[0].valid_time, "2026-09-17T14:20:00.000000Z");
    }

    #[test]
    fn invalid_json_returns_safe_error_and_missing_timeline_fields_mean_no_data() {
        let invalid_json = parse_timeline(b"private-body-marker").unwrap_err().to_string();
        assert!(invalid_json.contains("invalid timeline JSON"));
        assert!(!invalid_json.contains("private-body-marker"));
        assert!(!invalid_json.contains("aemet.es"));

        assert!(parse_timeline(br#"[{"items":[]}]"#).unwrap().is_empty());
        assert!(parse_timeline(b"[]").unwrap().is_empty());
    }

    #[test]
    fn entries_with_missing_or_invalid_fields_are_skipped() {
        let payload = br#"[{"Elementos":[
            {"Nombre fichero":"missing-date.png"},
            {"Fecha":"2026-09-17T14:40:00Z"},
            {"Nombre fichero":"bad-date.png","Fecha":"not-a-time"},
            {"Nombre fichero":"good.png","Fecha":"2026-09-17T14:40:00Z"}
        ]}]"#;
        let frames = parse_timeline(payload).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].revision.as_deref(), Some("good"));
    }
}
