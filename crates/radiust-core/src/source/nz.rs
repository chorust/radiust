//! Native discovery adapter for New Zealand MetService rural radar images.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector, parse_utc_time};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "nz";
const PRODUCT: &str = "rain";
const BASE_URL: &str = "https://mobile-apps.metservice.com";
const HOST: &str = "mobile-apps.metservice.com";
const IMAGE_HOST: &str = "api.metservice.com";
// MetService mobile application key; callers can override it through sources.nz.api_key.
const MOBILE_API_KEY: &str = "l7xx2a4b9096debf47acb352d84a6ef39461";
const LOCATOR_VERSION: &str = "nz-legacy-v1";
const REQUEST_HEADERS: [(&str, &str); 3] = [
    ("Accept", "*/*"),
    ("User-Agent", "MetServiceNZ/518 CFNetwork/3860.700.2 Darwin/25.6.0"),
    ("Accept-Language", "en-US,en;q=0.9"),
];

/// MetService station IDs and their rural API endpoint names.
const STATIONS: [(&str, &str); 10] = [
    ("NZAU2", "Kumeu"),
    ("NZBA2", "Rotorua"),
    ("NZCA2", "Darfield"),
    ("NZNO2", "Whangarei"),
    ("NZSO2", "Gore"),
    ("NZMA2", "Gisborne"),
    ("NZOT2", "Oamaru"),
    ("NZTA2", "New-Plymouth"),
    ("NZWE2", "Ohariu-Valley"),
    ("NZWS2", "Hokitika"),
];

/// Discovers MetService's rural mobile radar frames for the requested stations.
pub struct NzSourceAdapter;

impl SourceAdapter for NzSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(HOST) || host.eq_ignore_ascii_case(IMAGE_HOST)
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            validate_source_query(&target, &context.query)?;
            if target.product.as_deref().is_some_and(|product| product != PRODUCT)
                || context.query.product.as_deref().is_some_and(|product| product != PRODUCT)
            {
                return Err(CoreError::Transport(
                    "source nz only supports the rain product".into(),
                ));
            }
            if context.query.base_time.is_some() {
                return Err(CoreError::Transport("source nz does not expose base times".into()));
            }
            if let Some(station) = target.station.as_deref() {
                if rural_id(station).is_none() {
                    return Err(CoreError::Transport(
                        "source nz requires a supported station".into(),
                    ));
                }
                if !context.query.stations.is_empty()
                    && !context.query.stations.iter().any(|requested| requested == station)
                {
                    return Ok(Vec::new());
                }
            }
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source nz discovery requires network access".into(),
                ));
            }

            let stations = STATIONS
                .iter()
                .filter(|(station, _)| {
                    !target.station.as_deref().is_some_and(|requested| requested != *station)
                        && (context.query.stations.is_empty()
                            || context.query.stations.iter().any(|requested| requested == station))
                })
                .map(|(station, rural)| ((*station).to_owned(), (*rural).to_owned()))
                .collect::<Vec<_>>();
            let request_context = context.clone();
            let results = stream::iter(stations)
                .map(|(station, rural)| {
                    let context = request_context.clone();
                    async move {
                        let url = station_url(&rural);
                        let payload = context
                            .http_transport
                            .get_bytes_with_headers(&url, &REQUEST_HEADERS)
                            .await
                            .map_err(sanitize_request_error)?;
                        Ok::<_, CoreError>(parse_image_list(&station, &payload))
                    }
                })
                .buffer_unordered(context.discovery_workers.max(1))
                .try_collect::<Vec<_>>()
                .await?;
            let entries = results.into_iter().flatten().collect();
            let mut frames = select_entries(entries, &context.query.selector)?;
            let api_key = context
                .source_options
                .get("api_key")
                .and_then(|value| value.as_str())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(MOBILE_API_KEY);
            for frame in &mut frames {
                if frame.locator["url"]
                    .as_str()
                    .and_then(|address| Url::parse(address).ok())
                    .is_some_and(|url| url.host_str() == Some(IMAGE_HOST))
                {
                    frame.locator["headers"]["apiKey"] = Value::String(api_key.to_owned());
                }
            }
            Ok(frames)
        })
    }
}

fn validate_source_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source nz received an invalid target".into()));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Entry {
    station: String,
    valid_time: DateTime<Utc>,
    revision: String,
    url: String,
}

fn rural_id(station: &str) -> Option<&'static str> {
    STATIONS.iter().find(|(id, _)| *id == station).map(|(_, rural)| *rural)
}

fn station_url(rural: &str) -> String {
    format!("{BASE_URL}/publicData/mobileRainRadar_rural_{rural}")
}

fn sanitize_request_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source nz discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source nz station response exceeds configured limits".into())
        }
        _ => CoreError::Transport("source nz station request failed".into()),
    }
}

fn parse_image_list(station: &str, payload: &[u8]) -> Vec<Entry> {
    let Ok(document) = serde_json::from_slice::<Value>(payload) else {
        // The legacy adapter skips a station response when its JSON is invalid.
        return Vec::new();
    };
    let Some(images) = document.get("imageList").and_then(Value::as_array) else {
        return Vec::new();
    };

    images
        .iter()
        .filter_map(|item| {
            let image = item.as_object()?;
            let raw_url = image.get("url")?.as_str()?;
            let raw_time = image.get("dateTimeISO")?.as_str()?;
            let valid_time = parse_provider_time(raw_time)?;
            let url = provider_image_url(raw_url)?;
            Some(Entry {
                station: station.to_owned(),
                revision: format!("{station}-{}", valid_time.timestamp()),
                valid_time,
                url: url.to_string(),
            })
        })
        .collect()
}

/// Parse MetService's ISO times (naive values are UTC) and legacy epoch strings.
fn parse_provider_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }

    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        match value.len() {
            10 => return DateTime::from_timestamp(value.parse().ok()?, 0),
            13 => {
                let milliseconds: i64 = value.parse().ok()?;
                return DateTime::from_timestamp(
                    milliseconds.div_euclid(1_000),
                    milliseconds.rem_euclid(1_000) as u32 * 1_000_000,
                );
            }
            _ => {}
        }
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }

    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
        "%Y-%m-%d %H:%M:%S%.f%z",
    ] {
        if let Ok(parsed) = DateTime::parse_from_str(value, format) {
            return Some(parsed.with_timezone(&Utc));
        }
    }

    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed.and_utc());
        }
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0).map(|date_time| date_time.and_utc()))
}

/// Resolve imageList URLs on the reviewed listing and mobile API HTTPS origins.
fn provider_image_url(raw_url: &str) -> Option<Url> {
    if raw_url.is_empty()
        || raw_url.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == '\\'
        })
    {
        return None;
    }

    let base = Url::parse(if raw_url.starts_with("/mobile/nz/assets/radars/") {
        "https://api.metservice.com"
    } else {
        BASE_URL
    })
    .ok()?;
    let url = match Url::parse(raw_url) {
        Ok(url) => url,
        Err(_) => {
            // A broken absolute URL must not be reinterpreted as a local path.
            if raw_url.starts_with("//") || raw_url.contains("://") || has_uri_scheme(raw_url) {
                return None;
            }
            base.join(raw_url).ok()?
        }
    };

    if url.scheme() != "https"
        || !url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case(HOST) || host.eq_ignore_ascii_case(IMAGE_HOST)
        })
        || has_user_info(raw_url)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let mut url = url;
    if let Some(query) = url.query().map(str::to_owned) {
        url.set_query(Some(&query.replace('+', "%2B")));
    }
    Some(url)
}

fn has_user_info(raw_url: &str) -> bool {
    raw_url
        .split_once("://")
        .map(|(_, authority_and_path)| authority_and_path)
        .or_else(|| raw_url.strip_prefix("//"))
        .and_then(|authority_and_path| authority_and_path.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'))
}

fn has_uri_scheme(value: &str) -> bool {
    let Some((scheme, _)) = value.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'.' | b'-'))
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
            let requested = parse_utc_time(time).map_err(|_| {
                CoreError::Transport("source nz query contains an invalid timestamp".into())
            })?;
            entries.into_iter().filter(|entry| entry.valid_time == requested).collect()
        }
        TimeSelector::Range { start, end } => {
            let start = parse_utc_time(start).map_err(|_| {
                CoreError::Transport("source nz query contains an invalid timestamp".into())
            })?;
            let end = parse_utc_time(end).map_err(|_| {
                CoreError::Transport("source nz query contains an invalid timestamp".into())
            })?;
            if start >= end {
                Vec::new()
            } else {
                entries
                    .into_iter()
                    .filter(|entry| entry.valid_time >= start && entry.valid_time < end)
                    .collect()
            }
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
    let mut headers = serde_json::Map::new();
    for (name, value) in REQUEST_HEADERS {
        headers.insert(name.to_owned(), Value::String(value.to_owned()));
    }
    if Url::parse(&entry.url).ok().is_some_and(|url| url.host_str() == Some(IMAGE_HOST)) {
        headers.insert("apiKey".into(), Value::String(MOBILE_API_KEY.into()));
    }
    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: Some(entry.station.clone()),
        valid_time: entry.valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(entry.revision),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": entry.url,
            "artifacts": [],
            "station": entry.station,
            "headers": headers,
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source nz frame identity could not be computed".into())
    })?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_all_and_explicit_multi_source_queries_but_rejects_mismatches() {
        let target =
            DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None };
        for query in [
            Query { source: Some("all".into()), ..Query::default() },
            Query { sources: vec!["nz".into(), "pt".into()], ..Query::default() },
        ] {
            validate_source_query(&target, &query).unwrap();
        }
        for query in [
            Query { source: Some("pt".into()), ..Query::default() },
            Query { sources: vec!["pt".into()], ..Query::default() },
        ] {
            assert!(validate_source_query(&target, &query).is_err());
        }
    }

    const IMAGE_LIST: &[u8] = br#"{"imageList":[
        {"url":"/radarImage","dateTimeISO":"2026-09-18T15:50:00+12:00"},
        {"url":"https://mobile-apps.metservice.com/radarImage?frame=older","dateTimeISO":"2026-09-18T03:40:00Z"},
        {"url":"https://example.invalid/radarImage","dateTimeISO":"2026-09-18T03:30:00Z"},
        {"url":"https://[::1","dateTimeISO":"2026-09-18T03:20:00Z"},
        {"url":"/radarImage","dateTimeISO":"not-a-time"},
        {"url":7,"dateTimeISO":"2026-09-18T03:10:00Z"},
        null
    ]}"#;

    fn entry(station: &str, valid_time: &str, url: &str) -> Entry {
        let valid_time = parse_provider_time(valid_time).expect("test timestamp");
        Entry {
            station: station.into(),
            revision: format!("{station}-{}", valid_time.timestamp()),
            valid_time,
            url: url.into(),
        }
    }

    #[test]
    fn parses_image_list_times_urls_headers_and_revisions() {
        let entries = parse_image_list("NZAU2", IMAGE_LIST);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
            "2026-09-18T03:50:00.000000Z"
        );
        assert_eq!(entries[0].revision, "NZAU2-1789703400");
        assert_eq!(entries[0].url, "https://mobile-apps.metservice.com/radarImage");
        assert_eq!(
            parse_provider_time("2026-09-18T03:50:00")
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Micros, true),
            "2026-09-18T03:50:00.000000Z"
        );
        assert_eq!(
            parse_provider_time("1789703400123")
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Micros, true),
            "2026-09-18T03:50:00.123000Z"
        );

        let frames =
            select_entries(entries, &TimeSelector::At { time: "2026-09-18T15:50:00+12:00".into() })
                .unwrap();
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!(frame.source, SOURCE);
        assert_eq!(frame.product, PRODUCT);
        assert_eq!(frame.station.as_deref(), Some("NZAU2"));
        assert_eq!(frame.revision.as_deref(), Some("NZAU2-1789703400"));
        assert_eq!(frame.locator_version, LOCATOR_VERSION);
        assert_eq!(frame.locator["headers"]["Accept"], "*/*");
        assert_eq!(frame.locator["headers"]["User-Agent"], REQUEST_HEADERS[1].1);
        assert_eq!(frame.locator["headers"]["Accept-Language"], "en-US,en;q=0.9");
        assert_eq!(frame.locator["url"], "https://mobile-apps.metservice.com/radarImage");
    }

    #[test]
    fn station_ids_map_to_the_original_rural_endpoints_and_headers() {
        let expected = [
            ("NZAU2", "Kumeu"),
            ("NZBA2", "Rotorua"),
            ("NZCA2", "Darfield"),
            ("NZNO2", "Whangarei"),
            ("NZSO2", "Gore"),
            ("NZMA2", "Gisborne"),
            ("NZOT2", "Oamaru"),
            ("NZTA2", "New-Plymouth"),
            ("NZWE2", "Ohariu-Valley"),
            ("NZWS2", "Hokitika"),
        ];
        assert_eq!(STATIONS, expected);
        for (station, rural) in expected {
            assert_eq!(rural_id(station), Some(rural));
            assert_eq!(
                station_url(rural),
                format!("{BASE_URL}/publicData/mobileRainRadar_rural_{rural}")
            );
        }
        assert_eq!(rural_id("NZUNKNOWN"), None);
        assert_eq!(
            REQUEST_HEADERS,
            [
                ("Accept", "*/*"),
                ("User-Agent", "MetServiceNZ/518 CFNetwork/3860.700.2 Darwin/25.6.0"),
                ("Accept-Language", "en-US,en;q=0.9"),
            ]
        );
    }

    #[test]
    fn latest_is_per_station_at_is_exact_and_range_is_half_open() {
        let entries = vec![
            entry("NZAU2", "2026-09-18T03:40:00Z", "https://mobile-apps.metservice.com/a"),
            entry("NZAU2", "2026-09-18T03:50:00Z", "https://mobile-apps.metservice.com/b"),
            entry("NZAU2", "2026-09-18T03:50:00Z", "https://mobile-apps.metservice.com/c"),
            entry("NZBA2", "2026-09-18T03:40:00Z", "https://mobile-apps.metservice.com/d"),
        ];

        let latest = select_entries(entries.clone(), &TimeSelector::Latest).unwrap();
        assert_eq!(latest.len(), 3);
        assert!(
            latest
                .iter()
                .filter(|frame| frame.station.as_deref() == Some("NZAU2"))
                .all(|frame| frame.valid_time == "2026-09-18T03:50:00.000000Z")
        );
        assert!(latest.iter().any(|frame| {
            frame.station.as_deref() == Some("NZBA2")
                && frame.valid_time == "2026-09-18T03:40:00.000000Z"
        }));

        let at = select_entries(
            entries.clone(),
            &TimeSelector::At { time: "2026-09-18T15:50:00+12:00".into() },
        )
        .unwrap();
        assert_eq!(at.len(), 2);

        let range = select_entries(
            entries,
            &TimeSelector::Range {
                start: "2026-09-18T03:40:00Z".into(),
                end: "2026-09-18T03:50:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 2);
        assert!(range.iter().all(|frame| frame.valid_time == "2026-09-18T03:40:00.000000Z"));
    }

    #[test]
    fn mobile_api_urls_preserve_timezone_and_allow_download_host() {
        let entries = parse_image_list("NZWE2", br#"{"imageList":[{"url":"/mobile/nz/assets/radars/radarImage?loc=WELLINGTON&type=RADAR&res=300K&time=2026-09-30T19:35:00+13:00","dateTimeISO":"2026-09-30T19:35:00+13:00"}]}"#);
        assert_eq!(entries.len(), 1);
        let url = Url::parse(&entries[0].url).unwrap();
        assert_eq!(url.host_str(), Some(IMAGE_HOST));
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "time").unwrap().1,
            "2026-09-30T19:35:00+13:00"
        );
        assert_eq!(entries[0].valid_time.to_rfc3339(), "2026-09-30T06:35:00+00:00");
        assert!(NzSourceAdapter.allows_artifact_host(IMAGE_HOST));
        let frame = frame_from_entry(entries[0].clone()).unwrap();
        assert_eq!(frame.locator["headers"]["apiKey"], MOBILE_API_KEY);
        assert_eq!(frame.locator["headers"]["User-Agent"], REQUEST_HEADERS[1].1);
    }

    #[test]
    fn rejects_foreign_and_malformed_provider_urls() {
        assert_eq!(
            provider_image_url("/radarImage").unwrap().as_str(),
            "https://mobile-apps.metservice.com/radarImage"
        );
        assert_eq!(
            provider_image_url("https://MOBILE-APPS.METSERVICE.COM/radarImage").unwrap().host_str(),
            Some(HOST)
        );
        for raw_url in [
            "https://example.invalid/radarImage",
            "http://mobile-apps.metservice.com/radarImage",
            "https://user@mobile-apps.metservice.com/radarImage",
            "https://@mobile-apps.metservice.com/radarImage",
            "https://mobile-apps.metservice.com:8443/radarImage",
            "https://[::1",
            "https://mobile-apps.metservice.com/has space",
            "https://mobile-apps.metservice.com\\@example.invalid/radarImage",
            "javascript:alert(1)",
        ] {
            assert!(provider_image_url(raw_url).is_none(), "accepted {raw_url}");
        }

        assert!(parse_image_list("NZAU2", b"not-json").is_empty());
        assert!(parse_image_list("NZAU2", br#"{"imageList":{}}"#).is_empty());
    }
}
