//! Native discovery adapter for Taiwan CWA's observation radar images.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, FixedOffset, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "tw-http";
const PRODUCT: &str = "observation";
const STATION: &str = "CV1_3600";
const INDEX_URL: &str = "https://www.cwa.gov.tw/Data/js/obs_img/Observe_radar.js";
const IMAGE_BASE: &str = "https://www.cwa.gov.tw/Data/radar/";
const IMAGE_HOST: &str = "www.cwa.gov.tw";
const REFERER: &str = "https://www.cwa.gov.tw/V8/C/W/OBS_Radar.html";
const LOCATOR_VERSION: &str = "tw-http-legacy-v1";
const MAX_INDEX_BYTES: usize = 1024 * 1024;
// CWA labels these recent observations in Asia/Taipei, which is UTC+08:00.
const TAIPEI_OFFSET_SECONDS: i32 = 8 * 60 * 60;

/// Discovers Taiwan CWA's CV1_3600 observation frames from its radar index.
pub struct TwHttpSourceAdapter;

impl SourceAdapter for TwHttpSourceAdapter {
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
        {
            return false;
        }
        let Some(revision) = frame.revision.as_deref() else {
            return false;
        };
        let Some(valid_time) = revision_time(revision) else {
            return false;
        };
        let Some(expected) = artifact_url(revision) else {
            return false;
        };
        let Ok(expected_url) = Url::parse(&expected) else {
            return false;
        };
        let Ok(frame_time) = DateTime::parse_from_rfc3339(&frame.valid_time) else {
            return false;
        };

        frame_time.with_timezone(&Utc) == valid_time
            && frame.locator.get("url").and_then(serde_json::Value::as_str)
                == Some(expected.as_str())
            && frame.locator.get("station").and_then(serde_json::Value::as_str) == Some(STATION)
            && frame.locator.get("revision").and_then(serde_json::Value::as_str) == Some(revision)
            && self.allows_artifact_host(url.host_str().unwrap_or_default())
            && url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url == &expected_url
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target_and_query(&target, &context.query)?;
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source tw-http discovery requires network access".into(),
                ));
            }

            let payload = context
                .http_transport
                .get_bytes_coalesced(
                    INDEX_URL,
                    &[("Accept", "*/*"), ("Referer", REFERER)],
                    &context.request_coalescer,
                )
                .await
                .map_err(sanitize_transport_error)?;
            let entries = parse_index(&payload)?;
            let frames =
                entries.into_iter().map(frame_from_entry).collect::<CoreResult<Vec<_>>>()?;
            select_frames(frames, &context.query.selector)
        })
    }
}

#[derive(Clone, Debug)]
struct Entry {
    valid_time: DateTime<Utc>,
    revision: String,
    url: String,
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport(
            "source tw-http received a mismatched source query".into(),
        ));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport(
            "source tw-http only supports the observation product".into(),
        ));
    }
    if target.station.as_deref().is_some_and(|station| station != STATION)
        || query.stations.iter().any(|station| station != STATION)
    {
        return Err(CoreError::Transport(
            "source tw-http only supports the CV1_3600 station".into(),
        ));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source tw-http does not expose base times".into()));
    }
    Ok(())
}

fn sanitize_transport_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source tw-http discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => CoreError::ResourceLimit(
            "source tw-http discovery response exceeds configured limits".into(),
        ),
        _ => CoreError::Transport("source tw-http index request failed".into()),
    }
}

/// Parse CWA's JavaScript object literals without evaluating JavaScript.
///
/// The provider index is expected to be small. Rejecting oversized input before
/// scanning keeps parser work and retained text bounded even if the endpoint
/// returns an unexpected document.
fn parse_index(payload: &[u8]) -> CoreResult<Vec<Entry>> {
    if payload.len() > MAX_INDEX_BYTES {
        return Err(CoreError::ResourceLimit(
            "source tw-http index exceeds the parser limit".into(),
        ));
    }

    let text = String::from_utf8_lossy(payload);
    let marker = "{\"img\":";
    let mut cursor = 0;
    let mut entries = Vec::new();
    while let Some(relative) = text[cursor..].find(marker) {
        let start = cursor + relative;
        let Some((filename, label, next)) = parse_index_item(&text, start + marker.len()) else {
            cursor = start + 1;
            continue;
        };
        cursor = next;

        let Some(revision) = filename.strip_suffix(".png") else {
            continue;
        };
        let Some(valid_time) = parse_taipei_label(label) else {
            continue;
        };
        if revision_time(revision) != Some(valid_time) {
            continue;
        }
        let Some(url) = artifact_url(revision) else {
            continue;
        };
        entries.push(Entry { valid_time, revision: revision.to_owned(), url });
    }
    Ok(entries)
}

fn parse_index_item(text: &str, mut position: usize) -> Option<(&str, &str, usize)> {
    let bytes = text.as_bytes();
    position = skip_ascii_whitespace(bytes, position);
    let (filename, next) = parse_single_quoted(text, position)?;
    position = skip_ascii_whitespace(bytes, next);
    if bytes.get(position) != Some(&b',') {
        return None;
    }
    position = skip_ascii_whitespace(bytes, position + 1);
    if bytes.get(position..position + 6)? != b"'text'" {
        return None;
    }
    position = skip_ascii_whitespace(bytes, position + 6);
    if bytes.get(position) != Some(&b':') {
        return None;
    }
    position = skip_ascii_whitespace(bytes, position + 1);
    let (label, next) = parse_single_quoted(text, position)?;
    Some((filename, label, next))
}

fn skip_ascii_whitespace(bytes: &[u8], mut position: usize) -> usize {
    while bytes.get(position).is_some_and(u8::is_ascii_whitespace) {
        position += 1;
    }
    position
}

fn parse_single_quoted(text: &str, position: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(position) != Some(&b'\'') {
        return None;
    }
    let start = position + 1;
    let end = start + text[start..].find('\'')?;
    Some((&text[start..end], end + 1))
}

fn parse_taipei_label(value: &str) -> Option<DateTime<Utc>> {
    let local_time = NaiveDateTime::parse_from_str(value, "%Y/%m/%d %H:%M").ok()?;
    let taipei = FixedOffset::east_opt(TAIPEI_OFFSET_SECONDS)?;
    taipei.from_local_datetime(&local_time).single().map(|time| time.with_timezone(&Utc))
}

fn revision_time(revision: &str) -> Option<DateTime<Utc>> {
    let timestamp = revision.strip_prefix("CV1_3600_")?;
    if timestamp.len() != 12 || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let local_time = NaiveDateTime::parse_from_str(timestamp, "%Y%m%d%H%M").ok()?;
    let taipei = FixedOffset::east_opt(TAIPEI_OFFSET_SECONDS)?;
    taipei.from_local_datetime(&local_time).single().map(|time| time.with_timezone(&Utc))
}

fn artifact_url(revision: &str) -> Option<String> {
    revision_time(revision)?;
    Some(format!("{IMAGE_BASE}{revision}.png"))
}

fn frame_from_entry(entry: Entry) -> CoreResult<FrameRef> {
    let expected_url = artifact_url(&entry.revision).ok_or_else(|| {
        CoreError::Transport("source tw-http index has an invalid revision".into())
    })?;
    if entry.url != expected_url || revision_time(&entry.revision) != Some(entry.valid_time) {
        return Err(CoreError::Transport("source tw-http index entry is inconsistent".into()));
    }

    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: Some(STATION.into()),
        valid_time: entry.valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(entry.revision.clone()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": entry.url,
            "artifacts": [],
            "station": STATION,
            "revision": entry.revision,
            "headers": {"Referer": REFERER},
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source tw-http frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn select_frames(mut frames: Vec<FrameRef>, selector: &TimeSelector) -> CoreResult<Vec<FrameRef>> {
    match selector {
        TimeSelector::Latest => {
            if let Some(latest) = frames.iter().map(|frame| frame.valid_time.clone()).max() {
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
        CoreError::Transport("source tw-http query contains an invalid timestamp".into())
    })?;
    Ok(parsed.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Micros, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use crate::limits::{Limits, RequestBudget};
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};

    const INDEX: &[u8] = br#"
        0:{"img":'CV1_3600_202609181040.png', 'text':'2026/09/18 10:40'}
        1:{"img": 'CV1_3600_202609181050.png', 'text': '2026/09/18 10:50'}
        2:{"img":'CV1_3600_202609181100.png', 'text':'2026/09/18 11:00'}
        3:{"img":'OTHER_202609181100.png', 'text':'2026/09/18 11:00'}
        4:{"img":'CV1_3600_202602301100.png', 'text':'2026/02/30 11:00'}
        5:{"img":'CV1_3600_202609181200.png', 'text':'2026/09/18 11:00'}
        6:{"img":'CV1_3600_202609181100.gif', 'text':'2026/09/18 11:00'}
        7:{"img":'CV1_3600_202609181100.png', 'text':'not a timestamp'}
    "#;

    fn entries() -> Vec<Entry> {
        parse_index(INDEX).expect("valid CWA test index")
    }

    fn target() -> DiscoveryTarget {
        DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None }
    }

    fn query() -> Query {
        Query { source: Some(SOURCE.into()), ..Query::default() }
    }

    fn context(query: Query, allow_network: bool) -> SourceContext {
        let limits = Limits::default();
        let request_budget = Arc::new(RequestBudget::new(&limits));
        let http_transport = Arc::new(
            HttpTransport::with_budget(limits.clone(), allow_network, request_budget.clone())
                .expect("HTTP transport"),
        );
        SourceContext {
            query,
            allow_network,
            discovery_workers: 1,
            source_options: Arc::new(Default::default()),
            request_budget,
            ftp_transport: Arc::new(FtpTransport::new(limits.clone(), allow_network)),
            limits,
            http_transport,
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        }
    }

    #[test]
    fn parses_only_matching_cv1_frames_and_converts_taipei_time_to_utc() {
        let parsed = entries();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].revision, "CV1_3600_202609181040");
        assert_eq!(parsed[0].valid_time.to_rfc3339(), "2026-09-18T02:40:00+00:00");
        assert_eq!(parsed[0].url, "https://www.cwa.gov.tw/Data/radar/CV1_3600_202609181040.png");
        assert_eq!(parsed[2].valid_time.to_rfc3339(), "2026-09-18T03:00:00+00:00");
    }

    #[test]
    fn rejects_an_oversized_index_before_parsing() {
        let payload = vec![b'x'; MAX_INDEX_BYTES + 1];
        let error = parse_index(&payload).unwrap_err();
        assert!(matches!(error, CoreError::ResourceLimit(_)));
    }

    #[test]
    fn builds_python_compatible_legacy_identity_and_cwa_headers() {
        let frame = frame_from_entry(entries().remove(1)).unwrap();
        assert_eq!(frame.source, SOURCE);
        assert_eq!(frame.product, PRODUCT);
        assert_eq!(frame.station.as_deref(), Some(STATION));
        assert_eq!(frame.valid_time, "2026-09-18T02:50:00.000000Z");
        assert_eq!(frame.revision.as_deref(), Some("CV1_3600_202609181050"));
        assert_eq!(frame.locator_version, "tw-http-legacy-v1");
        assert_eq!(
            frame.locator,
            json!({
                "url": "https://www.cwa.gov.tw/Data/radar/CV1_3600_202609181050.png",
                "artifacts": [],
                "station": STATION,
                "revision": "CV1_3600_202609181050",
                "headers": {"Referer": REFERER},
            })
        );

        let identity = frame_identity(&frame).unwrap();
        assert!(identity["locator"].get("url").is_none());
        assert!(identity["locator"].get("headers").is_none());
        assert_eq!(identity["locator"]["station"], STATION);
        assert_eq!(identity["locator"]["revision"], "CV1_3600_202609181050");
        assert_eq!(
            frame.logical_id,
            "a883768a804af8ff371db672d801005a33ed8742946a4ea532a6526ed96dfee9"
        );
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
    }

    #[test]
    fn latest_at_and_half_open_range_select_utc_frames() {
        let frames =
            entries().into_iter().map(frame_from_entry).collect::<CoreResult<Vec<_>>>().unwrap();

        let latest = select_frames(frames.clone(), &TimeSelector::Latest).unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].valid_time, "2026-09-18T03:00:00.000000Z");

        let at = select_frames(
            frames.clone(),
            &TimeSelector::At { time: "2026-09-18T10:50:00+08:00".into() },
        )
        .unwrap();
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].valid_time, "2026-09-18T02:50:00.000000Z");

        let range = select_frames(
            frames,
            &TimeSelector::Range {
                start: "2026-09-18T02:40:00Z".into(),
                end: "2026-09-18T03:00:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 2);
        assert_eq!(range[0].valid_time, "2026-09-18T02:40:00.000000Z");
        assert_eq!(range[1].valid_time, "2026-09-18T02:50:00.000000Z");
    }

    #[test]
    fn artifact_url_is_bound_to_the_frame_revision_and_exact_cwa_path() {
        let adapter = TwHttpSourceAdapter;
        let frame = frame_from_entry(entries().remove(1)).unwrap();
        let valid = Url::parse(frame.locator["url"].as_str().unwrap()).unwrap();
        assert!(adapter.allows_artifact_url(&frame, &valid));
        assert!(adapter.allows_artifact_host("WWW.CWA.GOV.TW"));
        assert!(!adapter.allows_artifact_host("cwa.gov.tw"));

        for raw in [
            "http://www.cwa.gov.tw/Data/radar/CV1_3600_202609181050.png",
            "https://cwa.gov.tw/Data/radar/CV1_3600_202609181050.png",
            "https://www.cwa.gov.tw/Data/radar/CV1_3600_202609181100.png",
            "https://www.cwa.gov.tw/Data/radar/CV1_3600_202609181050.png%2fother",
            "https://user@www.cwa.gov.tw/Data/radar/CV1_3600_202609181050.png",
            "https://www.cwa.gov.tw:8443/Data/radar/CV1_3600_202609181050.png",
            "https://www.cwa.gov.tw/Data/radar/CV1_3600_202609181050.png?download=1",
        ] {
            let url = Url::parse(raw).unwrap();
            assert!(!adapter.allows_artifact_url(&frame, &url), "accepted {raw}");
        }

        let mut inconsistent_time = frame.clone();
        inconsistent_time.valid_time = "2026-09-18T03:00:00Z".into();
        assert!(!adapter.allows_artifact_url(&inconsistent_time, &valid));
    }

    #[test]
    fn validates_source_product_station_and_base_time() {
        assert!(validate_target_and_query(&target(), &query()).is_ok());
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { source: "tw".into(), ..target() },
                &query(),
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { product: Some("grid".into()), ..target() },
                &query(),
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { station: Some("OTHER".into()), ..target() },
                &query(),
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target(),
                &Query { stations: vec!["OTHER".into()], ..query() },
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target(),
                &Query { base_time: Some("2026-09-18T02:50:00Z".into()), ..query() },
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn requires_explicit_network_opt_in() {
        let error = Arc::new(TwHttpSourceAdapter)
            .discover(target(), context(query(), false))
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::NetworkDisabled(_)));
    }
}
