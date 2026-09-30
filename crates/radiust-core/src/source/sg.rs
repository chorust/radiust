//! Native discovery adapter for Singapore's MSS 240 km rain-area images.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, FixedOffset, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use futures_util::future::BoxFuture;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "sg";
const PRODUCT: &str = "composite";
const STATION: &str = "SGCOMP";
const PAGE_URL: &str = "https://www.weather.gov.sg/weather-rain-area-240km";
const IMAGE_HOST: &str = "www.weather.gov.sg";
const IMAGE_BASE_PATH: &str = "/files/rainarea/240km/";
const LOCATOR_VERSION: &str = "sg-legacy-v1";
const SINGAPORE_UTC_OFFSET_SECONDS: i32 = 8 * 60 * 60;
const PAGE_HEADERS: [(&str, &str); 3] = [
    ("User-Agent", "Mozilla/5.0 (compatible; radiust/1)"),
    ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/*,*/*;q=0.8"),
    ("Accept-Language", "en-US,en;q=0.9"),
];

/// Discovers Singapore's legacy 240 km rain-area image frames.
pub struct SgSourceAdapter;

impl SourceAdapter for SgSourceAdapter {
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
            || !self.allows_artifact_host(url.host_str().unwrap_or_default())
        {
            return false;
        }
        let Some(revision) = frame.revision.as_deref() else {
            return false;
        };
        if revision.len() != 16 || !revision.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let Ok(valid_time) = DateTime::parse_from_rfc3339(&frame.valid_time) else {
            return false;
        };
        let Some(singapore_offset) = FixedOffset::east_opt(SINGAPORE_UTC_OFFSET_SECONDS) else {
            return false;
        };
        let local_prefix =
            valid_time.with_timezone(&singapore_offset).format("%Y%m%d%H%M").to_string();
        if !revision.starts_with(&local_prefix)
            || frame.locator.get("revision").and_then(serde_json::Value::as_str) != Some(revision)
        {
            return false;
        }
        let expected =
            format!("https://{IMAGE_HOST}{IMAGE_BASE_PATH}dpsri_240km_{revision}dBR.dpsri.png");
        url.as_str() == expected
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
                    "source sg discovery requires network access".into(),
                ));
            }

            let payload = context
                .http_transport
                .get_bytes_coalesced(PAGE_URL, &PAGE_HEADERS, &context.request_coalescer)
                .await
                .map_err(sanitize_page_error)?;
            let entries = parse_page(payload.as_ref());
            select_entries(entries, &context.query.selector)
        })
    }
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source sg received a mismatched source query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source sg only supports the composite product".into()));
    }
    if target.station.as_deref().is_some_and(|station| station != STATION) {
        return Err(CoreError::Transport("source sg only supports the SGCOMP station".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source sg does not expose base times".into()));
    }
    Ok(())
}

fn sanitize_page_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source sg discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source sg page response exceeds configured limits".into())
        }
        _ => CoreError::Transport("source sg page request failed".into()),
    }
}

#[derive(Clone, Debug)]
struct Entry {
    station: String,
    valid_time: DateTime<Utc>,
    revision: String,
    url: String,
}

/// Extracts legacy slideshow image URLs. A page with no recognized, safe
/// images produces no entries; discovery never invents a frame or timestamp.
fn parse_page(payload: &[u8]) -> Vec<Entry> {
    let text = String::from_utf8_lossy(payload);
    let mut entries = Vec::new();
    let mut seen_urls = BTreeSet::new();
    let mut cursor = 0;

    while cursor < text.len() {
        let Some((start, scheme_len)) = next_absolute_url(&text, cursor) else {
            break;
        };
        let token_end = text[start..]
            .char_indices()
            .find(|(_, character)| {
                character.is_whitespace() || matches!(character, '\'' | '"' | ',' | '<' | '>')
            })
            .map_or(text.len(), |(offset, _)| start + offset);
        let token = &text[start..token_end];

        if let Some((url, revision, valid_time)) = parse_image_url_token(token) {
            if seen_urls.insert(url.clone()) {
                entries.push(Entry { station: STATION.into(), valid_time, revision, url });
            }
        }
        cursor = token_end.max(start + scheme_len);
    }

    entries
}

fn next_absolute_url(text: &str, cursor: usize) -> Option<(usize, usize)> {
    let remaining = text.get(cursor..)?;
    let https = remaining.find("https://").map(|offset| (offset, 8));
    let http = remaining.find("http://").map(|offset| (offset, 7));
    match (https, http) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(found), None) | (None, Some(found)) => Some(found),
        (None, None) => None,
    }
    .map(|(offset, scheme_len)| (cursor + offset, scheme_len))
}

fn parse_image_url_token(token: &str) -> Option<(String, String, DateTime<Utc>)> {
    const MARKER: &str = "/dpsri_240km_";
    const SUFFIX: &str = "dBR.dpsri.png";

    let mut search_from = 0;
    while let Some(offset) = token.get(search_from..)?.find(MARKER) {
        let filename_start = search_from + offset + MARKER.len();
        let revision_end = filename_start.checked_add(16)?;
        let suffix_end = revision_end.checked_add(SUFFIX.len())?;
        let revision = token.get(filename_start..revision_end)?;
        if revision.bytes().all(|byte| byte.is_ascii_digit())
            && token.get(revision_end..suffix_end) == Some(SUFFIX)
        {
            let candidate_url = &token[..suffix_end];
            if safe_image_url(candidate_url, revision) {
                if let Some(valid_time) = singapore_time(revision) {
                    return Some((candidate_url.to_owned(), revision.to_owned(), valid_time));
                }
            }
        }
        search_from = filename_start;
    }
    None
}

fn safe_image_url(value: &str, revision: &str) -> bool {
    if value.chars().any(char::is_control)
        || value.contains('\\')
        || contains_encoded_path_separator_or_dot(value)
    {
        return false;
    }
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https"
        || !url.host_str().is_some_and(|host| host.eq_ignore_ascii_case(IMAGE_HOST))
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }

    let Some(filename) = url.path().strip_prefix(IMAGE_BASE_PATH) else {
        return false;
    };
    !filename.is_empty()
        && !filename.contains('/')
        && filename == format!("dpsri_240km_{revision}dBR.dpsri.png")
}

fn contains_encoded_path_separator_or_dot(value: &str) -> bool {
    let lowercase = value.to_ascii_lowercase();
    ["%2e", "%2f", "%5c", "%00"].iter().any(|marker| lowercase.contains(marker))
}

fn singapore_time(revision: &str) -> Option<DateTime<Utc>> {
    if revision.len() != 16 || !revision.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let local_time = NaiveDateTime::parse_from_str(&revision[..12], "%Y%m%d%H%M").ok()?;
    // Asia/Singapore uses fixed UTC+08:00, so no daylight-saving transition
    // needs to be applied to these provider-local timestamps.
    let singapore = FixedOffset::east_opt(SINGAPORE_UTC_OFFSET_SECONDS)?;
    singapore.from_local_datetime(&local_time).single().map(|time| time.with_timezone(&Utc))
}

fn select_entries(entries: Vec<Entry>, selector: &TimeSelector) -> CoreResult<Vec<FrameRef>> {
    let selected = match selector {
        TimeSelector::Latest => {
            let mut latest_by_station = BTreeMap::<String, DateTime<Utc>>::new();
            for entry in &entries {
                latest_by_station
                    .entry(entry.station.clone())
                    .and_modify(|latest| *latest = (*latest).max(entry.valid_time))
                    .or_insert(entry.valid_time);
            }
            entries
                .into_iter()
                .filter(|entry| latest_by_station.get(&entry.station) == Some(&entry.valid_time))
                .collect::<Vec<_>>()
        }
        TimeSelector::At { time } => {
            let requested = parse_selector_time(time)?;
            entries.into_iter().filter(|entry| entry.valid_time == requested).collect()
        }
        TimeSelector::Range { start, end } => {
            let start = parse_selector_time(start)?;
            let end = parse_selector_time(end)?;
            entries
                .into_iter()
                .filter(|entry| entry.valid_time >= start && entry.valid_time < end)
                .collect()
        }
    };

    let mut frames = selected.into_iter().map(frame_from_entry).collect::<CoreResult<Vec<_>>>()?;
    frames.sort_by(|left, right| {
        left.valid_time
            .cmp(&right.valid_time)
            .then_with(|| left.station.cmp(&right.station))
            .then_with(|| left.logical_id.cmp(&right.logical_id))
    });
    Ok(frames)
}

fn parse_selector_time(value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| CoreError::Transport("source sg query contains an invalid timestamp".into()))
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
            "headers": {
                "User-Agent": PAGE_HEADERS[0].1,
                "Accept": PAGE_HEADERS[1].1,
                "Accept-Language": PAGE_HEADERS[2].1,
            },
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source sg frame identity could not be computed".into())
    })?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use crate::limits::{Limits, RequestBudget};
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};

    const FIRST_URL: &str =
        "https://www.weather.gov.sg/files/rainarea/240km/dpsri_240km_2026091810450000dBR.dpsri.png";

    fn entry(station: &str, revision: &str) -> Entry {
        Entry {
            station: station.into(),
            valid_time: singapore_time(revision).unwrap(),
            revision: revision.into(),
            url: format!(
                "https://www.weather.gov.sg/files/rainarea/240km/dpsri_240km_{revision}dBR.dpsri.png"
            ),
        }
    }

    fn target() -> DiscoveryTarget {
        DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None }
    }

    fn query() -> Query {
        Query { source: Some(SOURCE.into()), ..Query::default() }
    }

    #[test]
    fn parses_page_urls_converts_singapore_time_and_skips_invalid_dates() {
        let html = format!(
            "<script>show('{FIRST_URL}'); show('{FIRST_URL}'); show('https://www.weather.gov.sg/files/rainarea/240km/dpsri_240km_2026131810450000dBR.dpsri.png');</script>"
        );
        let entries = parse_page(html.as_bytes());

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].station, STATION);
        assert_eq!(entries[0].revision, "2026091810450000");
        assert_eq!(entries[0].valid_time.to_rfc3339(), "2026-09-18T02:45:00+00:00");
        assert_eq!(entries[0].url, FIRST_URL);
        assert!(parse_page(b"<html>page format changed</html>").is_empty());
    }

    #[test]
    fn rejects_mismatched_sources_products_stations_and_base_times() {
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { source: "fr".into(), ..target() },
                &query(),
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(&target(), &Query { source: Some("fr".into()), ..query() },)
                .is_err()
        );
        assert!(
            validate_target_and_query(
                &target(),
                &Query { product: Some("rain".into()), ..query() },
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
                &target(),
                &Query { base_time: Some("2026-09-18T02:45:00Z".into()), ..query() },
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
    }

    #[tokio::test]
    async fn reports_network_disabled_without_requesting_the_page() {
        let limits = Limits::default();
        let request_budget = Arc::new(RequestBudget::new(&limits));
        let context = SourceContext {
            query: query(),
            allow_network: false,
            discovery_workers: 1,
            source_options: Arc::new(Default::default()),
            request_budget: request_budget.clone(),
            ftp_transport: Arc::new(FtpTransport::new(limits.clone(), false)),
            limits: limits.clone(),
            http_transport: Arc::new(
                HttpTransport::with_budget(limits, false, request_budget).unwrap(),
            ),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        };

        let error = Arc::new(SgSourceAdapter).discover(target(), context).await.unwrap_err();
        assert!(matches!(error, CoreError::NetworkDisabled(_)));
    }

    #[test]
    fn latest_is_per_station_at_is_exact_and_range_is_half_open() {
        let entries = vec![
            entry("north", "2026091810450000"),
            entry("north", "2026091811000000"),
            entry("south", "2026091810300000"),
        ];
        let latest = select_entries(entries.clone(), &TimeSelector::Latest).unwrap();
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[0].station.as_deref(), Some("south"));
        assert_eq!(latest[0].valid_time, "2026-09-18T02:30:00.000000Z");
        assert_eq!(latest[1].station.as_deref(), Some("north"));
        assert_eq!(latest[1].valid_time, "2026-09-18T03:00:00.000000Z");

        let at = select_entries(
            entries.clone(),
            &TimeSelector::At { time: "2026-09-18T10:45:00+08:00".into() },
        )
        .unwrap();
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].valid_time, "2026-09-18T02:45:00.000000Z");
        assert!(
            select_entries(
                entries.clone(),
                &TimeSelector::At { time: "2026-09-18T02:45:00.000001Z".into() },
            )
            .unwrap()
            .is_empty()
        );

        let range = select_entries(
            entries,
            &TimeSelector::Range {
                start: "2026-09-18T02:45:00Z".into(),
                end: "2026-09-18T03:00:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 1);
        assert_eq!(range[0].valid_time, "2026-09-18T02:45:00.000000Z");
    }

    #[test]
    fn rejects_urls_outside_the_provider_host_and_image_directory() {
        for url in [
            "http://www.weather.gov.sg/files/rainarea/240km/dpsri_240km_2026091810450000dBR.dpsri.png",
            "https://weather.gov.sg/files/rainarea/240km/dpsri_240km_2026091810450000dBR.dpsri.png",
            "https://www.weather.gov.sg/files/rainarea/240km-evil/dpsri_240km_2026091810450000dBR.dpsri.png",
            "https://www.weather.gov.sg/other/dpsri_240km_2026091810450000dBR.dpsri.png",
            "https://user@www.weather.gov.sg/files/rainarea/240km/dpsri_240km_2026091810450000dBR.dpsri.png",
            "https://www.weather.gov.sg/files/rainarea/240km/dpsri_240km_2026091810450000dBR.dpsri.png?download=1",
        ] {
            assert!(!safe_image_url(url, "2026091810450000"), "unexpectedly accepted {url}");
        }

        let mixed_page = format!(
            "{FIRST_URL} https://www.weather.gov.sg/files/rainarea/240km-evil/dpsri_240km_2026091811000000dBR.dpsri.png"
        );
        let parsed = parse_page(mixed_page.as_bytes());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].url, FIRST_URL);
        assert!(SgSourceAdapter.allows_artifact_host("www.weather.gov.sg"));
        assert!(SgSourceAdapter.allows_artifact_host("WWW.WEATHER.GOV.SG"));
        assert!(!SgSourceAdapter.allows_artifact_host("weather.gov.sg"));
        assert!(!SgSourceAdapter.allows_artifact_host("api-open.data.gov.sg"));
    }

    #[test]
    fn frame_identity_keeps_the_legacy_revision_and_ignores_locator_transport_details() {
        let frame = frame_from_entry(entry(STATION, "2026091810450000")).unwrap();
        assert_eq!(frame.locator_version, LOCATOR_VERSION);
        assert_eq!(frame.revision.as_deref(), Some("2026091810450000"));
        assert_eq!(frame.locator["revision"], "2026091810450000");
        assert_eq!(
            frame.logical_id, "b240b3fbbb7e4cc50c5e2288645935b60f9ea22216ff3490e697d4d603e57ee9",
            "SG identity must match the existing Python adapter golden",
        );
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());

        let identity = frame_identity(&frame).unwrap();
        assert!(identity["locator"].get("url").is_none());
        assert!(identity["locator"].get("headers").is_none());
        assert_eq!(identity["locator"]["station"], STATION);
        assert_eq!(identity["locator"]["revision"], "2026091810450000");

        let mut changed_transport_details = frame.clone();
        changed_transport_details.locator["url"] =
            json!("https://www.weather.gov.sg/files/rainarea/240km/other-cdn-path.png");
        changed_transport_details.locator["headers"] = json!({"User-Agent": "different"});
        assert_eq!(logical_id(&changed_transport_details).unwrap(), frame.logical_id);
    }

    #[test]
    fn artifact_url_must_match_the_selected_frame_revision_and_valid_time() {
        let adapter = SgSourceAdapter;
        let frame = frame_from_entry(entry(STATION, "2026091810450000")).unwrap();
        let selected = Url::parse(FIRST_URL).unwrap();
        assert!(adapter.allows_artifact_url(&frame, &selected));

        let forged = Url::parse(
            "https://www.weather.gov.sg/files/rainarea/240km/dpsri_240km_2026091811000000dBR.dpsri.png",
        )
        .unwrap();
        assert!(!adapter.allows_artifact_url(&frame, &forged));

        let mut inconsistent_time = frame.clone();
        inconsistent_time.valid_time = "2026-09-18T03:00:00Z".into();
        assert!(!adapter.allows_artifact_url(&inconsistent_time, &selected));
    }
}
