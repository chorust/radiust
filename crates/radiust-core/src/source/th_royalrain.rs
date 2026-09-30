//! Raw-only adapter for Thailand's Royal Rainmaking CAPPI listings.
//!
//! The provider filename is the only timestamp evidence currently retained.
//! The image palette and geographic registration remain unverified, so this
//! adapter deliberately exposes discovery and byte-preserving acquisition only.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector, parse_utc_time};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "th_royalrain";
const PRODUCT: &str = "cappi";
const HOST: &str = "file.royalrain.go.th";
const BASE_URL: &str = "https://file.royalrain.go.th/opendata/radar_data/cappi";
const BASE_PATH: &str = "/opendata/radar_data/cappi";
const LOCATOR_VERSION: &str = "th-royalrain-legacy-v1";
const USER_AGENT: &str = "Mozilla/5.0 (compatible; radiust/1)";
const STATIONS: [&str; 11] = [
    "omkoi",
    "rongkwang",
    "takhli",
    "rasisalai",
    "singha",
    "phimai",
    "banphue",
    "sattahip",
    "pathio",
    "phanom",
    "pluakdaeng",
];

pub struct ThRoyalRainSourceAdapter;

impl SourceAdapter for ThRoyalRainSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(HOST)
    }

    fn allows_artifact_url(&self, frame: &FrameRef, url: &Url) -> bool {
        frame_plan(frame).is_some_and(|(expected, _)| {
            url.as_str() == expected && url.scheme() == "https" && url.host_str() == Some(HOST)
        })
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_target_and_query(&target, &context.query)?;
            let stations = selected_stations(&target, &context.query);
            if stations.is_empty() {
                return Ok(Vec::new());
            }
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source th_royalrain discovery requires network access".into(),
                ));
            }

            let request_context = context.clone();
            let entries = stream::iter(stations)
                .map(|station| {
                    let context = request_context.clone();
                    async move {
                        let page_url = format!("{BASE_URL}/?station={station}");
                        let payload = context
                            .http_transport
                            .get_bytes_coalesced(
                                &page_url,
                                &[("User-Agent", USER_AGENT)],
                                &context.request_coalescer,
                            )
                            .await
                            .map_err(sanitize_page_error)?;
                        context
                            .limits
                            .validate_bytes(payload.len() as u64, payload.len() as u64)?;
                        Ok::<_, CoreError>(parse_page(&station, &page_url, &payload))
                    }
                })
                .buffer_unordered(context.discovery_workers.max(1))
                .try_collect::<Vec<_>>()
                .await?
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            select_entries(entries, &context.query.selector)
        })
    }
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport(
            "source th_royalrain received a mismatched source query".into(),
        ));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport(
            "source th_royalrain only supports the cappi product".into(),
        ));
    }
    if target.station.as_deref().is_some_and(|station| !STATIONS.contains(&station)) {
        return Err(CoreError::Transport(
            "source th_royalrain received an unsupported station".into(),
        ));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source th_royalrain has no base times".into()));
    }
    Ok(())
}

fn selected_stations(target: &DiscoveryTarget, query: &Query) -> Vec<String> {
    STATIONS
        .iter()
        .copied()
        .filter(|station| {
            target.station.as_deref().is_none_or(|requested| requested == *station)
                && (query.stations.is_empty()
                    || query.stations.iter().any(|requested| requested == station))
        })
        .map(str::to_owned)
        .collect()
}

#[derive(Clone, Debug)]
struct Entry {
    station: String,
    page_url: String,
    filename: String,
    url: String,
    revision: String,
    valid_time: DateTime<Utc>,
}

fn parse_page(station: &str, page_url: &str, payload: &[u8]) -> Vec<Entry> {
    let html = String::from_utf8_lossy(payload);
    let mut seen = BTreeSet::new();
    image_sources(&html)
        .into_iter()
        .filter_map(|source| {
            let filename = source.split('?').next()?.rsplit('/').next()?;
            let valid_time = timestamp_from_filename(filename)?;
            let url = safe_image_url(page_url, source, station, filename)?;
            if !seen.insert(url.clone()) {
                return None;
            }
            Some(Entry {
                station: station.to_owned(),
                page_url: page_url.to_owned(),
                revision: filename.strip_suffix(".png")?.to_owned(),
                filename: filename.to_owned(),
                url,
                valid_time,
            })
        })
        .collect()
}

/// Extract quoted `src` attributes from image tags without depending on a
/// permissive HTML parser. The result is subsequently constrained by origin,
/// station path, and a provider filename timestamp.
fn image_sources(html: &str) -> Vec<&str> {
    let lower = html.to_ascii_lowercase();
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = lower.get(cursor..).and_then(|tail| tail.find("<img")) {
        let start = cursor + offset + 4;
        let Some(end_offset) = lower.get(start..).and_then(|tail| tail.find('>')) else {
            break;
        };
        let end = start + end_offset;
        let mut pos = start;
        while pos < end {
            while pos < end && lower.as_bytes()[pos].is_ascii_whitespace() {
                pos += 1;
            }
            let name_start = pos;
            while pos < end
                && (lower.as_bytes()[pos].is_ascii_alphanumeric()
                    || matches!(lower.as_bytes()[pos], b'-' | b'_'))
            {
                pos += 1;
            }
            if name_start == pos {
                pos += 1;
                continue;
            }
            let name = &lower[name_start..pos];
            while pos < end && lower.as_bytes()[pos].is_ascii_whitespace() {
                pos += 1;
            }
            if lower.as_bytes().get(pos) != Some(&b'=') {
                continue;
            }
            pos += 1;
            while pos < end && lower.as_bytes()[pos].is_ascii_whitespace() {
                pos += 1;
            }
            let Some(&quote @ (b'\'' | b'"')) = lower.as_bytes().get(pos) else {
                while pos < end && !lower.as_bytes()[pos].is_ascii_whitespace() {
                    pos += 1;
                }
                continue;
            };
            pos += 1;
            let value_start = pos;
            while pos < end && lower.as_bytes()[pos] != quote {
                pos += 1;
            }
            if name == "src" {
                result.push(&html[value_start..pos]);
            }
            if pos < end {
                pos += 1;
            }
        }
        cursor = end + 1;
    }
    result
}

fn safe_image_url(page_url: &str, source: &str, station: &str, filename: &str) -> Option<String> {
    if !STATIONS.contains(&station)
        || filename.is_empty()
        || filename.contains(['/', '\\'])
        || filename.chars().any(char::is_control)
        || filename.to_ascii_lowercase().contains("%2e")
        || filename.to_ascii_lowercase().contains("%2f")
        || filename.to_ascii_lowercase().contains("%5c")
        || !filename.ends_with(".png")
        || !filename.to_ascii_lowercase().contains("cappi")
    {
        return None;
    }
    let page = Url::parse(page_url).ok()?;
    let url = page.join(source).ok()?;
    if url.scheme() != "https"
        || !url.host_str().is_some_and(|host| host.eq_ignore_ascii_case(HOST))
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let expected_path = format!("{BASE_PATH}/{station}/{filename}");
    (url.path() == expected_path).then(|| url.to_string())
}

fn timestamp_from_filename(filename: &str) -> Option<DateTime<Utc>> {
    let bytes = filename.as_bytes();
    if bytes.len() >= 29
        && bytes.get(12..16).is_some_and(|value| value == b"0200" || value == b"0400")
        && bytes.get(16..) == Some(b"dBZ.cappi.png")
        && bytes.get(..12).is_some_and(|value| value.iter().all(u8::is_ascii_digit))
    {
        return parse_compact_time(&filename[..12]);
    }

    if let Some(marker) = filename.find("THA-") {
        let stamp_start = marker + 4;
        if let Some(time) = parse_split_time(filename, stamp_start) {
            return Some(time);
        }
    }
    bytes.iter().enumerate().find_map(|(index, byte)| {
        (*byte == b'-').then(|| parse_split_time(filename, index + 1)).flatten()
    })
}

fn parse_split_time(filename: &str, start: usize) -> Option<DateTime<Utc>> {
    let date_end = start.checked_add(8)?;
    let time_start = date_end.checked_add(1)?;
    let time_end = time_start.checked_add(4)?;
    if filename.as_bytes().get(date_end) != Some(&b'-')
        || !filename.as_bytes().get(start..date_end)?.iter().all(u8::is_ascii_digit)
        || !filename.as_bytes().get(time_start..time_end)?.iter().all(u8::is_ascii_digit)
        || filename.as_bytes().get(time_end) != Some(&b'_')
    {
        return None;
    }
    let value = format!("{}{}", &filename[start..date_end], &filename[time_start..time_end]);
    parse_compact_time(&value)
}

fn parse_compact_time(value: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(value, "%Y%m%d%H%M").ok().map(|time| time.and_utc())
}

fn select_entries(entries: Vec<Entry>, selector: &TimeSelector) -> CoreResult<Vec<FrameRef>> {
    let selected: Vec<Entry> = match selector {
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
                .collect()
        }
        TimeSelector::At { time } => {
            let requested = parse_utc_time(time).map_err(|_| {
                CoreError::Transport("source th_royalrain query has an invalid timestamp".into())
            })?;
            entries.into_iter().filter(|entry| entry.valid_time == requested).collect()
        }
        TimeSelector::Range { start, end } => {
            let start = parse_utc_time(start).map_err(|_| {
                CoreError::Transport("source th_royalrain query has an invalid timestamp".into())
            })?;
            let end = parse_utc_time(end).map_err(|_| {
                CoreError::Transport("source th_royalrain query has an invalid timestamp".into())
            })?;
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
            "name": entry.filename,
            "media_type": "image/png",
            "headers": {
                "Referer": entry.page_url,
                "User-Agent": USER_AGENT,
            },
            "station": entry.station,
            "revision": entry.revision,
            "time_semantics": "filename_utc",
            "geometry_status": "unverified",
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source th_royalrain frame identity is invalid".into())
    })?;
    Ok(frame)
}

fn frame_plan(frame: &FrameRef) -> Option<(String, String)> {
    if frame.source != SOURCE
        || frame.product != PRODUCT
        || frame.base_time.is_some()
        || frame.locator_version != LOCATOR_VERSION
        || logical_id(frame).ok().as_deref() != Some(frame.logical_id.as_str())
        || frame.locator.get("time_semantics").and_then(Value::as_str) != Some("filename_utc")
        || frame.locator.get("geometry_status").and_then(Value::as_str) != Some("unverified")
    {
        return None;
    }
    let station = frame.station.as_deref()?;
    if !STATIONS.contains(&station)
        || frame.locator.get("station").and_then(Value::as_str) != Some(station)
    {
        return None;
    }
    let name = frame.locator.get("name").and_then(Value::as_str)?;
    let revision = frame.revision.as_deref()?;
    let page_url = format!("{BASE_URL}/?station={station}");
    let source = format!("{BASE_PATH}/{station}/{name}");
    let url = safe_image_url(&page_url, &source, station, name)?;
    if revision != name.strip_suffix(".png")?
        || frame.locator.get("revision").and_then(Value::as_str) != Some(revision)
        || timestamp_from_filename(name)?.to_rfc3339_opts(SecondsFormat::Micros, true)
            != frame.valid_time
        || frame.locator.get("url").and_then(Value::as_str) != Some(url.as_str())
        || frame.locator.get("media_type").and_then(Value::as_str) != Some("image/png")
        || frame
            .locator
            .get("headers")
            .and_then(|value| value.get("Referer"))
            .and_then(Value::as_str)
            != Some(page_url.as_str())
        || frame
            .locator
            .get("headers")
            .and_then(|value| value.get("User-Agent"))
            .and_then(Value::as_str)
            != Some(USER_AGENT)
    {
        return None;
    }
    Some((url, page_url))
}

fn sanitize_page_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => CoreError::NetworkDisabled(
            "source th_royalrain discovery requires network access".into(),
        ),
        CoreError::ResourceLimit(_) => CoreError::ResourceLimit(
            "source th_royalrain page response exceeds configured limits".into(),
        ),
        _ => CoreError::Transport("source th_royalrain page request failed".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use sha2::{Digest, Sha256};

    const PAGE: &[u8] = br#"<html><img src="/opendata/radar_data/cappi/takhli/2026091803420400dBZ.cappi.png"><img src="/opendata/radar_data/cappi/takhli/2026091803420400dBZ.cappi.png"><img src="https://evil.invalid/opendata/radar_data/cappi/takhli/2026091804000400dBZ.cappi.png"><img src="/opendata/radar_data/cappi/../other/2026091804000400dBZ.cappi.png"><img src="/opendata/radar_data/cappi/takhli/no-time.cappi.png"></html>"#;
    const RAW_IMAGE: &[u8] =
        include_bytes!("../../../../tests/fixtures/sources/th_royalrain/raw/takhli.png");

    #[test]
    fn parses_only_unique_canonical_station_images_with_valid_utc_times() {
        let page = format!("{BASE_URL}/?station=takhli");
        let entries = parse_page("takhli", &page, PAGE);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].revision, "2026091803420400dBZ.cappi");
        assert_eq!(entries[0].valid_time.to_rfc3339(), "2026-09-18T03:42:00+00:00");
        assert_eq!(entries[0].url, format!("{BASE_URL}/takhli/2026091803420400dBZ.cappi.png"));
    }

    #[test]
    fn retained_canonical_raw_image_matches_the_source_inventory_digest() {
        assert_eq!(
            hex::encode(Sha256::digest(RAW_IMAGE)),
            "8c13c0d0077bf5c66904ca0d1522f75890d96180647ca41dce986667d82e36cd"
        );
    }

    #[test]
    fn supports_the_two_retained_provider_filename_time_forms() {
        assert_eq!(
            timestamp_from_filename("1234THA-20260918-0342_product.png").unwrap().to_rfc3339(),
            "2026-09-18T03:42:00+00:00"
        );
        assert_eq!(
            timestamp_from_filename("radar-20260918-0342_cappi.png").unwrap().to_rfc3339(),
            "2026-09-18T03:42:00+00:00"
        );
        assert!(timestamp_from_filename("2026023003420400dBZ.cappi.png").is_none());
        assert!(timestamp_from_filename("2026091803420400dBZ.png").is_none());
    }

    #[test]
    fn frame_identity_binds_station_time_revision_and_exact_artifact_url() {
        let page = format!("{BASE_URL}/?station=takhli");
        let frame = frame_from_entry(parse_page("takhli", &page, PAGE).remove(0)).unwrap();
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
        assert_eq!(
            frame.logical_id,
            crate::identity::digest(&frame_identity(&frame).unwrap()).unwrap()
        );
        let adapter = ThRoyalRainSourceAdapter;
        assert!(adapter.allows_artifact_url(
            &frame,
            &Url::parse(frame.locator["url"].as_str().unwrap()).unwrap()
        ));
        assert!(
            !adapter.allows_artifact_url(
                &frame,
                &Url::parse(
                    "https://file.royalrain.go.th/opendata/radar_data/cappi/takhli/other.png"
                )
                .unwrap()
            )
        );
        let mut changed = frame;
        changed.locator["url"] = json!("https://evil.invalid/image.png");
        assert!(frame_plan(&changed).is_none());
    }

    #[test]
    fn latest_selection_is_per_station_and_ranges_are_half_open() {
        let entries = [
            ("takhli", "2026091803420400dBZ.cappi.png"),
            ("takhli", "2026091803570400dBZ.cappi.png"),
            ("omkoi", "2026091803420400dBZ.cappi.png"),
        ]
        .into_iter()
        .map(|(station, name)| {
            let page = format!("{BASE_URL}/?station={station}");
            let url =
                safe_image_url(&page, &format!("{BASE_PATH}/{station}/{name}"), station, name)
                    .unwrap();
            Entry {
                station: station.into(),
                page_url: page,
                filename: name.into(),
                revision: name.strip_suffix(".png").unwrap().into(),
                url,
                valid_time: timestamp_from_filename(name).unwrap(),
            }
        })
        .collect::<Vec<_>>();
        let latest = select_entries(entries.clone(), &TimeSelector::Latest).unwrap();
        assert_eq!(latest.len(), 2);
        assert!(latest.iter().any(|frame| frame.station.as_deref() == Some("omkoi")));
        let range = select_entries(
            entries,
            &TimeSelector::Range {
                start: "2026-09-18T03:42:00Z".into(),
                end: "2026-09-18T03:57:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 2);
    }
}
