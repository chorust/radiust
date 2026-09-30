//! Native discovery adapter for Vietnam Hymetnet CMAX radar images.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, Duration, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde_json::json;
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "vn";
const PRODUCT: &str = "cmax";
const BASE_URL: &str = "http://hymetnet.gov.vn";
const HOST: &str = "hymetnet.gov.vn";
const LOCATOR_VERSION: &str = "vn-legacy-v1";
const STATIONS: [&str; 11] =
    ["PLI", "VTR", "PHA", "VIN", "DHA", "TKY", "QNH", "PLE", "NHT", "NHB", "HUE"];

/// Discovers Vietnam's public CMAX frames from the station slideshow pages.
pub struct VnSourceAdapter;

impl SourceAdapter for VnSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        if frame.source != SOURCE
            || frame.product != PRODUCT
            || frame.base_time.is_some()
            || frame.locator_version != LOCATOR_VERSION
        {
            return false;
        }
        let Some(station) = frame.station.as_deref() else {
            return false;
        };
        let Some(revision) = frame.revision.as_deref() else {
            return false;
        };
        let Some(timestamp) = revision.strip_prefix(&format!("{station}-")) else {
            return false;
        };
        let Some(expected) = artifact_url(station, timestamp) else {
            return false;
        };
        let Ok(expected_url) = Url::parse(&expected) else {
            return false;
        };
        let Some(valid_time) = parse_timestamp(timestamp) else {
            return false;
        };
        let Ok(frame_time) = DateTime::parse_from_rfc3339(&frame.valid_time) else {
            return false;
        };
        frame_time.with_timezone(&Utc) == valid_time
            && frame.locator.get("url").and_then(serde_json::Value::as_str)
                == Some(expected.as_str())
            && frame.locator.get("station").and_then(serde_json::Value::as_str) == Some(station)
            && frame.locator.get("revision").and_then(serde_json::Value::as_str) == Some(revision)
            && self.allows_artifact_host(url.host_str().unwrap_or_default())
            && url.scheme() == "http"
            && url.port().is_none()
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
                    "source vn discovery requires network access".into(),
                ));
            }

            // Fetch all stations before applying station filters: Hymetnet's
            // advertised freshest timestamp defines the source-wide window.
            // try_collect short-circuits on an error and drops the remaining
            // buffered futures, which in turn drops their HTTP requests.
            let request_context = context.clone();
            let stations = STATIONS.iter().map(|station| (*station).to_owned()).collect::<Vec<_>>();
            let pages = stream::iter(stations)
                .map(move |station: String| {
                    let context = request_context.clone();
                    async move {
                        let address = station_page_url(&station).ok_or_else(|| {
                            CoreError::Transport("source vn has an invalid station".into())
                        })?;
                        let payload = context
                            .http_transport
                            .get_bytes_coalesced(&address, &[], &context.request_coalescer)
                            .await
                            .map_err(sanitize_page_error)?;
                        Ok::<_, CoreError>(parse_station_page(&station, &payload))
                    }
                })
                .buffer_unordered(context.discovery_workers.max(1))
                .try_collect::<Vec<_>>()
                .await?;

            let entries = freshest_window(pages.into_iter().flatten().collect());
            let frames = entries
                .into_iter()
                .filter(|entry| {
                    target.station.as_deref().is_none_or(|station| entry.station == station)
                        && (context.query.stations.is_empty()
                            || context
                                .query
                                .stations
                                .iter()
                                .any(|station| station == &entry.station))
                })
                .map(frame_from_entry)
                .collect::<CoreResult<Vec<_>>>()?;
            Ok(sort_frames(frames))
        })
    }
}

#[derive(Clone, Debug)]
struct Entry {
    station: String,
    valid_time: DateTime<Utc>,
    revision: String,
    url: String,
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source vn received a mismatched source query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source vn only supports the cmax product".into()));
    }
    if target.station.as_deref().is_some_and(|station| !STATIONS.contains(&station))
        || query.stations.iter().any(|station| !STATIONS.contains(&station.as_str()))
    {
        return Err(CoreError::Transport("source vn requires a supported station".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source vn does not expose base times".into()));
    }
    Ok(())
}

fn station_page_url(station: &str) -> Option<String> {
    STATIONS.contains(&station).then(|| format!("{BASE_URL}/radar/{station}"))
}

fn artifact_url(station: &str, timestamp: &str) -> Option<String> {
    if !STATIONS.contains(&station) || parse_timestamp(timestamp).is_none() {
        return None;
    }
    Some(format!(
        "{BASE_URL}/dataout_web/{station}/{}/{station}_{timestamp}_CMAX00.png",
        &timestamp[..8]
    ))
}

fn sanitize_page_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source vn discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source vn station page exceeds configured limits".into())
        }
        _ => CoreError::Transport("source vn station page request failed".into()),
    }
}

/// Parse assignments matching `tentimesett[... ] = "YYYYmmddHHMM";`.
fn parse_station_page(station: &str, payload: &[u8]) -> Vec<Entry> {
    if !STATIONS.contains(&station) {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(payload);
    let marker = "tentimesett[";
    let mut cursor = 0;
    let mut entries = Vec::new();
    while let Some(relative_start) = text[cursor..].find(marker) {
        let start = cursor + relative_start;
        let expression_start = start + marker.len();
        let Some(close_offset) = text[expression_start..].find(']') else {
            break;
        };
        let close = expression_start + close_offset;
        if text[expression_start..close].contains('\n') {
            cursor = expression_start;
            continue;
        }

        let mut position = skip_whitespace(&text, close + 1);
        let bytes = text.as_bytes();
        if bytes.get(position) != Some(&b'=') {
            cursor = expression_start;
            continue;
        }
        position = skip_whitespace(&text, position + 1);
        if bytes.get(position) != Some(&b'"') {
            cursor = expression_start;
            continue;
        }
        let timestamp_start = position + 1;
        let Some(timestamp_end) = timestamp_start.checked_add(12) else {
            break;
        };
        let Some(timestamp_bytes) = bytes.get(timestamp_start..timestamp_end) else {
            break;
        };
        if !timestamp_bytes.iter().all(u8::is_ascii_digit)
            || bytes.get(timestamp_end..timestamp_end + 2) != Some(b"\";")
        {
            cursor = expression_start;
            continue;
        }
        let timestamp = &text[timestamp_start..timestamp_end];
        let Some(valid_time) = parse_timestamp(timestamp) else {
            cursor = timestamp_end + 2;
            continue;
        };
        if let Some(url) = artifact_url(station, timestamp) {
            entries.push(Entry {
                station: station.to_owned(),
                valid_time,
                revision: format!("{station}-{timestamp}"),
                url,
            });
        }
        cursor = timestamp_end + 2;
    }
    entries
}

fn skip_whitespace(value: &str, mut position: usize) -> usize {
    while let Some(character) = value[position..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        position += character.len_utf8();
    }
    position
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    if value.len() != 12 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    NaiveDateTime::parse_from_str(value, "%Y%m%d%H%M").ok().map(|time| time.and_utc())
}

/// Apply Hymetnet's rolling-current window using the freshest timestamp from
/// every station page, before callers filter to a requested station.
fn freshest_window(entries: Vec<Entry>) -> Vec<Entry> {
    let Some(freshest) = entries.iter().map(|entry| entry.valid_time).max() else {
        return entries;
    };
    let Some(cutoff) = freshest.checked_sub_signed(Duration::days(1)) else {
        return entries;
    };
    entries.into_iter().filter(|entry| entry.valid_time >= cutoff).collect()
}

fn frame_from_entry(entry: Entry) -> CoreResult<FrameRef> {
    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: Some(entry.station.clone()),
        valid_time: entry.valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(entry.revision.clone()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": entry.url,
            "artifacts": [],
            "station": entry.station,
            "revision": entry.revision,
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source vn frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn sort_frames(mut frames: Vec<FrameRef>) -> Vec<FrameRef> {
    frames.sort_by(|left, right| {
        left.valid_time
            .cmp(&right.valid_time)
            .then_with(|| left.station.cmp(&right.station))
            .then_with(|| left.logical_id.cmp(&right.logical_id))
    });
    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use crate::limits::{Limits, RequestBudget};
    use crate::model::TimeSelector;
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};

    const VN_FIXTURE: &str = include_str!("../../../../tests/fixtures/sources/vn/fixture.json");

    fn entry(station: &str, timestamp: &str) -> Entry {
        let valid_time = parse_timestamp(timestamp).expect("valid test timestamp");
        Entry {
            station: station.into(),
            valid_time,
            revision: format!("{station}-{timestamp}"),
            url: artifact_url(station, timestamp).expect("known station and valid timestamp"),
        }
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
            discovery_workers: 3,
            source_options: Arc::new(Default::default()),
            request_budget,
            ftp_transport: Arc::new(FtpTransport::new(limits.clone(), allow_network)),
            limits,
            http_transport,
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        }
    }

    #[test]
    fn preserves_the_eleven_station_inventory_and_legacy_endpoints() {
        assert_eq!(
            STATIONS,
            ["PLI", "VTR", "PHA", "VIN", "DHA", "TKY", "QNH", "PLE", "NHT", "NHB", "HUE"]
        );
        for station in STATIONS {
            let expected = format!("{BASE_URL}/radar/{station}");
            assert_eq!(station_page_url(station).as_deref(), Some(expected.as_str()));
        }
        assert_eq!(
            artifact_url("PLI", "202609180350").as_deref(),
            Some("http://hymetnet.gov.vn/dataout_web/PLI/20260918/PLI_202609180350_CMAX00.png")
        );
        assert_eq!(station_page_url("UNKNOWN"), None);
    }

    #[test]
    fn parses_tentimesett_as_utc_and_skips_invalid_or_malformed_values() {
        let html = br#"
            <script>
              tentimesett[0]
                =   "202609180350";
              tentimesett[1] = "202609180355";
              tentimesett[2] = "202602300100";
              tentimesett[3] = "20260918036";
              tentimesett[4] = "202609180400"
            </script>
        "#;
        let entries = parse_station_page("PLI", html);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].revision, "PLI-202609180350");
        assert_eq!(entries[0].valid_time.to_rfc3339(), "2026-09-18T03:50:00+00:00");
        assert_eq!(entries[1].valid_time.to_rfc3339(), "2026-09-18T03:55:00+00:00");
        assert!(parse_station_page("UNKNOWN", html).is_empty());
    }

    #[test]
    fn applies_the_inclusive_one_day_window_from_source_widest_freshness() {
        let entries = vec![
            entry("HUE", "202609160349"),
            entry("PLI", "202609170350"),
            entry("VTR", "202609170351"),
            entry("DHA", "202609180350"),
        ];
        let fresh = freshest_window(entries);
        assert_eq!(fresh.len(), 3);
        assert!(!fresh.iter().any(|item| item.station == "HUE"));
        assert!(fresh.iter().any(|item| item.station == "PLI"));
        assert!(fresh.iter().any(|item| item.station == "VTR"));
        assert!(fresh.iter().any(|item| item.station == "DHA"));
    }

    #[test]
    fn keeps_window_candidates_for_engine_latest_at_and_half_open_range_selection() {
        let entries = freshest_window(vec![
            entry("PLI", "202609180340"),
            entry("PLI", "202609180350"),
            entry("VTR", "202609180350"),
        ]);
        for selector in [
            TimeSelector::Latest,
            TimeSelector::At { time: "2026-09-18T11:50:00+08:00".into() },
            TimeSelector::Range {
                start: "2026-09-18T03:40:00Z".into(),
                end: "2026-09-18T03:50:00Z".into(),
            },
        ] {
            let query = Query { selector, ..query() };
            validate_target_and_query(&target(), &query).unwrap();
            let frames = entries
                .iter()
                .cloned()
                .map(frame_from_entry)
                .collect::<CoreResult<Vec<_>>>()
                .unwrap();
            // The Engine owns temporal selection; source discovery keeps every
            // candidate so exact-time and [start, end) queries remain possible.
            assert_eq!(frames.len(), 3);
            assert!(frames.iter().any(|frame| frame.valid_time == "2026-09-18T03:40:00.000000Z"));
            assert_eq!(
                frames
                    .iter()
                    .filter(|frame| frame.valid_time == "2026-09-18T03:50:00.000000Z")
                    .count(),
                2
            );
        }
    }

    #[test]
    fn matches_fixture_url_revision_and_python_logical_id() {
        let fixture: serde_json::Value = serde_json::from_str(VN_FIXTURE).unwrap();
        let row = &fixture["frames"][0];
        let timestamp = "202609180350";
        let frame = frame_from_entry(entry("PLI", timestamp)).unwrap();

        assert_eq!(fixture["source"], SOURCE);
        assert_eq!(row["product"], frame.product);
        assert_eq!(row["station"], frame.station.as_deref().unwrap());
        assert_eq!(row["valid_time"], "2026-09-18T03:50:00Z");
        assert_eq!(row["uri"], frame.locator["url"]);
        assert_eq!(row["revision"], frame.revision.as_deref().unwrap());
        assert_eq!(row["locator_version"], LOCATOR_VERSION);
        assert_eq!(frame.valid_time, "2026-09-18T03:50:00.000000Z");
        assert_eq!(frame_identity(&frame).unwrap()["locator"]["artifacts"], json!([]));
        // Golden value from python/radiust/identity.py for the fixture frame.
        assert_eq!(
            frame.logical_id,
            "578a167f72340b934146345414697635cc8baa5937ec5830af226e073ce10e8d"
        );
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
    }

    #[test]
    fn artifact_urls_are_bound_to_supported_station_time_host_and_path() {
        let adapter = VnSourceAdapter;
        let frame = frame_from_entry(entry("PLI", "202609180350")).unwrap();
        let good = Url::parse(frame.locator["url"].as_str().unwrap()).unwrap();
        assert!(adapter.allows_artifact_url(&frame, &good));
        assert!(adapter.allows_artifact_host(HOST));
        assert!(!adapter.allows_artifact_host("evilhymetnet.gov.vn"));

        for raw in [
            "https://hymetnet.gov.vn/dataout_web/PLI/20260918/PLI_202609180350_CMAX00.png",
            "http://evil.invalid/dataout_web/PLI/20260918/PLI_202609180350_CMAX00.png",
            "http://hymetnet.gov.vn/dataout_web/PLI/20260918/../PLI_202609180350_CMAX00.png",
            "http://hymetnet.gov.vn/dataout_web/VTR/20260918/VTR_202609180350_CMAX00.png",
            "http://user@hymetnet.gov.vn/dataout_web/PLI/20260918/PLI_202609180350_CMAX00.png",
            "http://hymetnet.gov.vn:8080/dataout_web/PLI/20260918/PLI_202609180350_CMAX00.png",
            "http://hymetnet.gov.vn/dataout_web/PLI/20260918/PLI_202609180350_CMAX00.png?x=1",
        ] {
            let url = Url::parse(raw).unwrap();
            assert!(!adapter.allows_artifact_url(&frame, &url), "accepted {raw}");
        }
        assert_eq!(artifact_url("PLI", "202602300100"), None);
        assert_eq!(artifact_url("UNKNOWN", "202609180350"), None);
    }

    #[test]
    fn validates_source_product_station_and_base_time_contracts() {
        assert!(validate_target_and_query(&target(), &query()).is_ok());
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { source: "fr".into(), ..target() },
                &query(),
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { product: Some("rain".into()), ..target() },
                &query(),
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { station: Some("NOPE".into()), ..target() },
                &query(),
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target(),
                &Query { stations: vec!["NOPE".into()], ..query() },
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target(),
                &Query { base_time: Some("2026-09-18T03:50:00Z".into()), ..query() },
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(&target(), &Query { source: Some("my".into()), ..query() },)
                .is_err()
        );
    }

    #[tokio::test]
    async fn requires_explicit_public_network_opt_in() {
        let error = Arc::new(VnSourceAdapter)
            .discover(target(), context(query(), false))
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::NetworkDisabled(_)));
    }
}
