//! Native discovery adapter for Indonesia's SIDARMA CMAX metadata API.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, Duration, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "id_sidarma";
const PRODUCT: &str = "cmax";
const API_URL: &str = "https://api.bmkg.go.id/radar/v1/arsip";
const ARTIFACT_DOMAIN: &str = "bmkg.go.id";
const LOCATOR_VERSION: &str = "id_sidarma-legacy-v1";
const REQUEST_HEADERS: [(&str, &str); 2] =
    [("Accept", "application/json, text/plain, */*"), ("User-Agent", "SidarmaMobile/2")];
const RADAR_RESOURCE: &str = include_str!("../../resources/builtin/sources/id_sidarma.json");

/// Discovers the latest SIDARMA CMAX frame in a bounded UTC archive window.
pub struct IdSidarmaSourceAdapter;

impl SourceAdapter for IdSidarmaSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        is_bmkg_host(host)
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
        let Ok(radars) = radar_inventory() else {
            return false;
        };
        let Some(radar) = radars.get(station) else {
            return false;
        };
        if !locator_matches_radar(frame, radar) {
            return false;
        }
        let Some((expected, revision, artifact_host, artifact_path)) =
            safe_artifact_url(url.as_str())
        else {
            return false;
        };
        frame.revision.as_deref() == Some(revision.as_str())
            && frame.locator.get("revision").and_then(Value::as_str) == Some(revision.as_str())
            && frame.locator.get("url").and_then(Value::as_str) == Some(expected.as_str())
            && frame.locator.get("artifact_host").and_then(Value::as_str)
                == Some(artifact_host.as_str())
            && artifact_host == url.host_str().unwrap_or_default()
            && frame.locator.get("artifact_path").and_then(Value::as_str)
                == Some(artifact_path.as_str())
            && artifact_path == url.path()
            && DateTime::parse_from_rfc3339(&frame.valid_time).is_ok()
            && logical_id(frame).ok().as_deref() == Some(frame.logical_id.as_str())
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
                    "source id_sidarma discovery requires network access".into(),
                ));
            }
            let api_key = api_key_from_options(&context.source_options).ok_or_else(|| {
                CoreError::Transport("source id_sidarma requires sources.id_sidarma.api_key".into())
            })?;
            let radars = radar_inventory()?;
            let radar_ids = selected_radar_ids(&radars, context.source_options.get("radar_ids"))?
                .into_iter()
                .filter(|station| {
                    target.station.as_deref().is_none_or(|value| value == station)
                        && (context.query.stations.is_empty()
                            || context.query.stations.iter().any(|value| value == station))
                })
                .collect::<Vec<_>>();
            if radar_ids.is_empty() {
                return Ok(Vec::new());
            }

            let request_context = context.clone();
            let request_key = api_key.clone();
            let end = Utc::now();
            let start = end - Duration::hours(1);
            let requests = radar_ids
                .into_iter()
                .map(|station| {
                    let radar = radars[&station].clone();
                    (station, radar)
                })
                .collect::<Vec<_>>();
            let results = stream::iter(requests)
                .map(move |(station, radar)| {
                    let context = request_context.clone();
                    let api_key = request_key.clone();
                    async move {
                        let address = api_url(&station, start, end)?;
                        let headers = [
                            REQUEST_HEADERS[0],
                            REQUEST_HEADERS[1],
                            ("x-api-key", api_key.as_str()),
                        ];
                        let payload = context
                            .http_transport
                            .get_bytes_coalesced(&address, &headers, &context.request_coalescer)
                            .await
                            .map_err(sanitize_request_error)?;
                        parse_response(&station, &radar, &payload)
                    }
                })
                .buffer_unordered(context.discovery_workers.max(1))
                .collect::<Vec<_>>()
                .await;

            let mut entries = Vec::new();
            let mut failures = Vec::new();
            for result in results {
                match result {
                    Ok(response) => entries.extend(response),
                    Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                    Err(error @ CoreError::NetworkDisabled(_)) => return Err(error),
                    Err(error) => failures.push(error),
                }
            }
            if !failures.is_empty() && entries.is_empty() {
                return Err(failures.remove(0));
            }

            let unique = deduplicate_entries(entries);
            let frames = unique
                .into_iter()
                .map(|entry| {
                    let radar = radars.get(&entry.station).ok_or_else(|| {
                        CoreError::Transport("source id_sidarma returned an unknown station".into())
                    })?;
                    frame_from_entry(entry, radar)
                })
                .collect::<CoreResult<Vec<_>>>()?;
            Ok(select_latest_per_station(frames))
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Radar {
    id: String,
    bbox: [f64; 4],
}

#[derive(Debug, Deserialize)]
struct RadarResource {
    schema_version: u32,
    source: String,
    radars: Vec<Radar>,
}

#[derive(Clone, Debug)]
struct Entry {
    station: String,
    valid_time: DateTime<Utc>,
    revision: String,
    url: String,
    artifact_host: String,
    artifact_path: String,
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport(
            "source id_sidarma received a mismatched source query".into(),
        ));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport(
            "source id_sidarma only supports the cmax product".into(),
        ));
    }
    let radars = radar_inventory()?;
    if target.station.as_deref().is_some_and(|station| !radars.contains_key(station))
        || query.stations.iter().any(|station| !radars.contains_key(station))
    {
        return Err(CoreError::Transport("source id_sidarma requires a supported radar".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source id_sidarma does not expose base times".into()));
    }
    if !matches!(query.selector, TimeSelector::Latest) {
        return Err(CoreError::Transport("source id_sidarma only supports latest frames".into()));
    }
    Ok(())
}

fn api_url(station: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> CoreResult<String> {
    if !valid_radar_id(station) {
        return Err(CoreError::Transport("source id_sidarma has an invalid radar id".into()));
    }
    let mut url = Url::parse(API_URL)
        .map_err(|_| CoreError::Transport("source id_sidarma has an invalid API URL".into()))?;
    if start > end || end - start > Duration::hours(1) {
        return Err(CoreError::Transport("source id_sidarma has an invalid archive window".into()));
    }
    url.query_pairs_mut()
        .append_pair("startTime", &start.format("%Y%m%d%H%M").to_string())
        .append_pair("endTime", &end.format("%Y%m%d%H%M").to_string())
        .append_pair("radar", station)
        .append_pair("product", "CMAX");
    Ok(url.to_string())
}

fn radar_inventory() -> CoreResult<BTreeMap<String, Radar>> {
    let resource: RadarResource = serde_json::from_str(RADAR_RESOURCE)
        .map_err(|_| CoreError::Transport("source id_sidarma radar resource is invalid".into()))?;
    if resource.schema_version != 1 || resource.source != SOURCE || resource.radars.is_empty() {
        return Err(CoreError::Transport("source id_sidarma radar resource is invalid".into()));
    }
    let mut radars = BTreeMap::new();
    for radar in resource.radars {
        let [west, south, east, north] = radar.bbox;
        if !valid_radar_id(&radar.id)
            || ![west, south, east, north].iter().all(|value| value.is_finite())
            || west < -180.0
            || east > 180.0
            || south < -90.0
            || north > 90.0
            || west >= east
            || south >= north
            || radars.insert(radar.id.clone(), radar).is_some()
        {
            return Err(CoreError::Transport("source id_sidarma radar resource is invalid".into()));
        }
    }
    Ok(radars)
}

fn valid_radar_id(value: &str) -> bool {
    (3..=4).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

fn selected_radar_ids(
    radars: &BTreeMap<String, Radar>,
    configured: Option<&serde_yaml_ng::Value>,
) -> CoreResult<Vec<String>> {
    let Some(values) = configured else {
        return Ok(radars.keys().cloned().collect());
    };
    let Some(values) = values.as_sequence() else {
        return Err(CoreError::Transport("sources.id_sidarma.radar_ids must be a list".into()));
    };
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values {
        let Some(station) = value.as_str() else {
            return Err(CoreError::Transport("sources.id_sidarma.radar_ids must be a list".into()));
        };
        let station = station.trim().to_ascii_uppercase();
        if station.is_empty() {
            continue;
        }
        if !radars.contains_key(&station) {
            return Err(CoreError::Transport("source id_sidarma has an unknown radar id".into()));
        }
        if seen.insert(station.clone()) {
            selected.push(station);
        }
    }
    Ok(selected)
}

fn api_key_from_options(options: &BTreeMap<String, serde_yaml_ng::Value>) -> Option<String> {
    options
        .get("api_key")
        .and_then(serde_yaml_ng::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn sanitize_request_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source id_sidarma discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => CoreError::ResourceLimit(
            "source id_sidarma metadata response exceeds configured limits".into(),
        ),
        _ => CoreError::Transport("source id_sidarma metadata request failed".into()),
    }
}

fn parse_response(station: &str, radar: &Radar, payload: &[u8]) -> CoreResult<Vec<Entry>> {
    let document: Value = serde_json::from_slice(payload).map_err(|_| {
        CoreError::Transport("source id_sidarma returned invalid metadata JSON".into())
    })?;
    if document.get("listURL").is_some() || document.get("listTime").is_some() {
        return parse_archive_response(station, radar, &document);
    }
    let Some(cmax) = document.get("CMAX").and_then(Value::as_object) else {
        return Err(CoreError::Transport(
            "source id_sidarma returned unsupported metadata JSON".into(),
        ));
    };
    let mut entries = Vec::new();
    for bucket_name in ["LastOneHour", "Latest"] {
        let Some(bucket) = cmax.get(bucket_name).and_then(Value::as_object) else {
            continue;
        };
        let Some(files) = bucket.get("file") else {
            continue;
        };
        let Some(times) = bucket.get("timeUTC") else {
            continue;
        };
        for (raw_url, raw_time) in as_values(files).into_iter().zip(as_values(times)) {
            let (Some(raw_url), Some(raw_time)) = (raw_url.as_str(), raw_time.as_str()) else {
                continue;
            };
            if raw_time.trim().is_empty() || raw_time.trim().eq_ignore_ascii_case("no data") {
                continue;
            }
            let Some(valid_time) = parse_provider_time(raw_time) else {
                continue;
            };
            let Some((url, revision, artifact_host, artifact_path)) = safe_artifact_url(raw_url)
            else {
                continue;
            };
            // The official radar inventory is also the station identity boundary.
            if !valid_radar_id(station) || radar.id != station {
                continue;
            }
            entries.push(Entry {
                station: station.to_owned(),
                valid_time,
                revision,
                url,
                artifact_host,
                artifact_path,
            });
        }
    }
    Ok(entries)
}

fn parse_archive_response(
    station: &str,
    radar: &Radar,
    document: &Value,
) -> CoreResult<Vec<Entry>> {
    let malformed =
        || CoreError::Transport("source id_sidarma returned invalid archive metadata".into());
    let urls = document.get("listURL").and_then(Value::as_array).ok_or_else(malformed)?;
    let times = document.get("listTime").and_then(Value::as_array).ok_or_else(malformed)?;
    if urls.len() != times.len() || !valid_radar_id(station) || radar.id != station {
        return Err(malformed());
    }
    let mut entries = Vec::new();
    for (raw_url, raw_time) in urls.iter().zip(times) {
        let raw_url = raw_url.as_str().ok_or_else(malformed)?;
        let raw_time = raw_time.as_str().ok_or_else(malformed)?.trim();
        if raw_time.eq_ignore_ascii_case("no data") {
            continue;
        }
        let valid_time = parse_provider_time(raw_time).ok_or_else(malformed)?;
        let (url, revision, artifact_host, artifact_path) =
            safe_artifact_url(raw_url).ok_or_else(malformed)?;
        // Bind archive URLs to their station, product and provider UTC time.
        let expected_path = format!(
            "/sidarma-mobile/arsip/{station}/{}/CMAX/{station}-{}.png",
            valid_time.format("%Y%m%d"),
            valid_time.format("%Y%m%d-%H%M"),
        );
        if artifact_host != "api.bmkg.go.id" || artifact_path != expected_path {
            return Err(malformed());
        }
        entries.push(Entry {
            station: station.into(),
            valid_time,
            revision,
            url,
            artifact_host,
            artifact_path,
        });
    }
    Ok(entries)
}

fn as_values(value: &Value) -> Vec<&Value> {
    value.as_array().map_or_else(|| vec![value], |values| values.iter().collect())
}

fn parse_provider_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    let normalized = value
        .strip_suffix(" [UTC]")
        .or_else(|| value.strip_suffix(" UTC"))
        .map(|prefix| format!("{prefix}+00:00"))
        .unwrap_or_else(|| value.to_owned());
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&normalized) {
        return Some(parsed.with_timezone(&Utc));
    }
    const AWARE_FORMATS: [&str; 4] = [
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M%:z",
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M%:z",
    ];
    for format in AWARE_FORMATS {
        if let Ok(parsed) = DateTime::parse_from_str(&normalized, format) {
            return Some(parsed.with_timezone(&Utc));
        }
    }
    const NAIVE_FORMATS: [&str; 4] =
        ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"];
    NAIVE_FORMATS
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(&normalized, format).ok())
        .map(|value| value.and_utc())
}

fn safe_artifact_url(value: &str) -> Option<(String, String, String, String)> {
    if value.chars().any(char::is_control)
        || value.contains('\\')
        || contains_encoded_path_separator_or_dot(value)
        || !(value.starts_with("https://") || value.starts_with("http://"))
    {
        return None;
    }
    let url = Url::parse(value).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if !matches!(url.scheme(), "http" | "https")
        || !is_bmkg_host(&host)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let raw_path = raw_path(value)?;
    if raw_path.split('/').any(|part| matches!(part, "." | "..")) {
        return None;
    }
    let path = url.path();
    let revision = path.rsplit('/').next()?.to_owned();
    if path == "/"
        || revision.is_empty()
        || revision == "."
        || revision == ".."
        || revision
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | '\0'))
    {
        return None;
    }
    Some((url.to_string(), revision, host, path.to_owned()))
}

fn raw_path(value: &str) -> Option<&str> {
    let after_scheme = value.split_once("://")?.1;
    let path_start = after_scheme.find('/')?;
    let path = &after_scheme[path_start..];
    let end = path.find(['?', '#']).unwrap_or(path.len());
    Some(&path[..end])
}

fn contains_encoded_path_separator_or_dot(value: &str) -> bool {
    let lowercase = value.to_ascii_lowercase();
    ["%2e", "%2f", "%5c", "%00"].iter().any(|marker| lowercase.contains(marker))
}

fn is_bmkg_host(host: &str) -> bool {
    host.eq_ignore_ascii_case(ARTIFACT_DOMAIN)
        || host
            .to_ascii_lowercase()
            .strip_suffix(ARTIFACT_DOMAIN)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn deduplicate_entries(entries: Vec<Entry>) -> Vec<Entry> {
    let mut seen = BTreeSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert((entry.station.clone(), entry.url.clone(), entry.valid_time)))
        .collect()
}

fn frame_from_entry(entry: Entry, radar: &Radar) -> CoreResult<FrameRef> {
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
            "bbox": radar.bbox,
            "station": entry.station,
            "revision": entry.revision,
            "artifact_host": entry.artifact_host,
            "artifact_path": entry.artifact_path,
            "headers": {"User-Agent": "SidarmaMobile/2"},
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source id_sidarma frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn locator_matches_radar(frame: &FrameRef, radar: &Radar) -> bool {
    frame.locator.get("station").and_then(Value::as_str) == Some(radar.id.as_str())
        && frame.locator.get("bbox").and_then(Value::as_array).is_some_and(|bbox| {
            bbox.len() == 4
                && bbox
                    .iter()
                    .zip(radar.bbox)
                    .all(|(actual, expected)| actual.as_f64() == Some(expected))
        })
}

fn select_latest_per_station(mut frames: Vec<FrameRef>) -> Vec<FrameRef> {
    let mut latest = BTreeMap::<String, String>::new();
    for frame in &frames {
        if let Some(station) = frame.station.as_ref() {
            latest
                .entry(station.clone())
                .and_modify(|time| *time = time.clone().max(frame.valid_time.clone()))
                .or_insert_with(|| frame.valid_time.clone());
        }
    }
    frames.retain(|frame| {
        frame
            .station
            .as_ref()
            .and_then(|station| latest.get(station))
            .is_some_and(|time| time == &frame.valid_time)
    });
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

    const RESPONSE: &[u8] = br#"{
        "CMAX": {
            "LastOneHour": {
                "file": [
                    "https://radar.bmkg.go.id/sidarma/CGK_20260918_0200.png",
                    "https://radar.bmkg.go.id/sidarma/CGK_20260918_0210.png"
                ],
                "timeUTC": ["2026-09-18 02:00 UTC", "2026-09-18 02:10 UTC"]
            },
            "Latest": {
                "file": "https://radar.bmkg.go.id/sidarma/CGK_20260918_0210.png",
                "timeUTC": "2026-09-18 02:10 UTC"
            }
        }
    }"#;

    #[test]
    fn radar_inventory_and_api_urls_are_constrained() {
        let radars = radar_inventory().unwrap();
        assert_eq!(radars.len(), 47);
        let end = DateTime::parse_from_rfc3339("2026-09-30T07:11:23Z").unwrap().with_timezone(&Utc);
        let start = end - Duration::hours(1);
        assert_eq!(
            api_url("JAK", start, end).unwrap(),
            "https://api.bmkg.go.id/radar/v1/arsip?startTime=202609300611&endTime=202609300711&radar=JAK&product=CMAX"
        );
        assert!(api_url("../CGK", start, end).is_err());
        assert!(api_url("cgk", start, end).is_err());
        assert!(api_url("JAK", end, start).is_err());
        assert!(api_url("JAK", start - Duration::minutes(1), end).is_err());
    }

    #[test]
    fn live_archive_fixture_binds_latest_url_time_station_and_raw_bytes() {
        let radar = radar_inventory().unwrap().remove("JAK").unwrap();
        let payload =
            include_bytes!("../../../../tests/fixtures/sources/id_sidarma/archive-response.json");
        let entries = parse_response("JAK", &radar, payload).unwrap();
        assert_eq!(entries.len(), 9);
        let frames =
            entries.into_iter().map(|entry| frame_from_entry(entry, &radar).unwrap()).collect();
        let latest = select_latest_per_station(frames);
        assert_eq!(latest.len(), 1);
        let frame = &latest[0];
        assert_eq!(frame.valid_time, "2026-09-30T07:05:00.000000Z");
        assert_eq!(frame.station.as_deref(), Some("JAK"));
        assert_eq!(frame.revision.as_deref(), Some("JAK-20260930-0705.png"));
        assert_eq!(frame.locator["headers"], json!({"User-Agent": "SidarmaMobile/2"}));
        assert!(frame.locator["headers"].get("x-api-key").is_none());
        assert!(IdSidarmaSourceAdapter.allows_artifact_url(
            frame,
            &Url::parse(frame.locator["url"].as_str().unwrap()).unwrap()
        ));
        let raw = include_bytes!(
            "../../../../tests/fixtures/sources/id_sidarma/raw/JAK-20260930-0705.png"
        );
        use sha2::{Digest, Sha256};
        assert_eq!(
            hex::encode(Sha256::digest(raw)),
            "b48b2cfa0bebe5bc63f72910af0b4816b71bed89a295d8ab6dcb1a5f11c3d8e5"
        );
        let reader =
            image::ImageReader::new(std::io::Cursor::new(raw)).with_guessed_format().unwrap();
        assert_eq!(reader.into_dimensions().unwrap(), (4084, 4084));
    }

    #[test]
    fn archive_no_data_placeholder_is_not_a_frame() {
        let radar = radar_inventory().unwrap().remove("JAK").unwrap();
        let payload = br#"{"listURL":["https://api.bmkg.go.id/sidarma-mobile/sidarma-nowcast/data/raster/nodata.png"],"listTime":["No Data"]}"#;
        assert!(parse_response("JAK", &radar, payload).unwrap().is_empty());
        assert!(
            parse_response("JAK", &radar, br#"{"listURL":[],"listTime":[]}"#).unwrap().is_empty()
        );
    }

    #[test]
    fn archive_rejects_malformed_pairs_errors_and_mismatched_frame_identity() {
        let radar = radar_inventory().unwrap().remove("JAK").unwrap();
        for payload in [
            json!({"status":403,"message":"Forbidden"}),
            json!({"listURL":[],"listTime":["2026-09-30 07:05 UTC"]}),
            json!({"listURL":[],"listTime":null}),
            json!({"listURL":[null],"listTime":["No Data"]}),
            json!({"listURL":["https://evil.example/frame.png"],"listTime":["2026-09-30 07:05 UTC"]}),
            json!({"listURL":["https://api.bmkg.go.id/sidarma-mobile/arsip/ACE/20260930/CMAX/ACE-20260930-0705.png"],"listTime":["2026-09-30 07:05 UTC"]}),
            json!({"listURL":["https://api.bmkg.go.id/sidarma-mobile/arsip/JAK/20260930/CMAX/JAK-20260930-0705.png"],"listTime":["2026-09-30 07:13 UTC"]}),
        ] {
            assert!(parse_response("JAK", &radar, &serde_json::to_vec(&payload).unwrap()).is_err());
        }
    }

    #[test]
    fn source_options_preserve_configured_credentials_and_radar_selection() {
        let radars = radar_inventory().unwrap();
        let options = BTreeMap::from([
            ("api_key".to_owned(), serde_yaml_ng::Value::String(" external-key ".to_owned())),
            (
                "radar_ids".to_owned(),
                serde_yaml_ng::Value::Sequence(vec![serde_yaml_ng::Value::String(
                    "CGK".to_owned(),
                )]),
            ),
        ]);

        assert_eq!(api_key_from_options(&options).as_deref(), Some("external-key"));
        assert_eq!(selected_radar_ids(&radars, options.get("radar_ids")).unwrap(), vec!["CGK"]);
        assert!(api_key_from_options(&BTreeMap::new()).is_none());
        assert_eq!(selected_radar_ids(&radars, None).unwrap().len(), 47);
    }

    #[test]
    fn parses_recent_buckets_deduplicates_and_keeps_verified_station_geometry() {
        let radars = radar_inventory().unwrap();
        let radar = &radars["CGK"];
        let entries = parse_response("CGK", radar, RESPONSE).unwrap();
        assert_eq!(entries.len(), 3);
        let unique = deduplicate_entries(entries);
        assert_eq!(unique.len(), 2);
        let frames = unique
            .into_iter()
            .map(|entry| frame_from_entry(entry, radar).unwrap())
            .collect::<Vec<_>>();
        let latest = select_latest_per_station(frames);
        assert_eq!(latest.len(), 1);
        let frame = &latest[0];
        assert_eq!(frame.product, PRODUCT);
        assert_eq!(frame.station.as_deref(), Some("CGK"));
        assert_eq!(frame.valid_time, "2026-09-18T02:10:00.000000Z");
        assert_eq!(frame.revision.as_deref(), Some("CGK_20260918_0210.png"));
        assert_eq!(frame.locator["bbox"], json!(radar.bbox));
        assert_eq!(frame.logical_id, logical_id(frame).unwrap());
        assert_eq!(
            frame.logical_id,
            "388fa342c99a4bd40d74537ef2c1fd649ee96a92a3d94076dee39c45b465e9f4",
            "Rust identity: {}",
            frame_identity(frame).unwrap()
        );
        assert!(frame_identity(frame).unwrap()["locator"].get("url").is_none());
    }

    #[test]
    fn rejects_untrusted_artifact_urls_and_locator_substitution() {
        assert!(safe_artifact_url("https://radar.bmkg.go.id/sidarma/frame.png").is_some());
        for value in [
            "https://evil.example/sidarma/frame.png",
            "https://radar.bmkg.go.id:8443/sidarma/frame.png",
            "https://user@radar.bmkg.go.id/sidarma/frame.png",
            "https://radar.bmkg.go.id/sidarma/../frame.png",
            "https://radar.bmkg.go.id/sidarma/%2e%2e/frame.png",
            "https://radar.bmkg.go.id/sidarma/frame.png?token=secret",
        ] {
            assert!(safe_artifact_url(value).is_none(), "accepted {value}");
        }

        let radar = radar_inventory().unwrap().remove("CGK").unwrap();
        let entry = parse_response("CGK", &radar, RESPONSE).unwrap().remove(0);
        let mut frame = frame_from_entry(entry, &radar).unwrap();
        let adapter = IdSidarmaSourceAdapter;
        let url = Url::parse(frame.locator["url"].as_str().unwrap()).unwrap();
        assert!(adapter.allows_artifact_url(&frame, &url));
        let forged = Url::parse("https://evil.example/sidarma/CGK_20260918_0200.png").unwrap();
        assert!(!adapter.allows_artifact_url(&frame, &forged));

        frame.locator["artifact_path"] = Value::String("/sidarma/other.png".into());
        assert!(!adapter.allows_artifact_url(&frame, &url));
    }

    #[test]
    fn parses_utc_provider_formats_and_skips_invalid_times() {
        assert_eq!(
            parse_provider_time("2026-09-18 02:10 UTC").unwrap().to_rfc3339(),
            "2026-09-18T02:10:00+00:00"
        );
        assert_eq!(
            parse_provider_time("2026-09-18T10:10:00+08:00").unwrap().to_rfc3339(),
            "2026-09-18T02:10:00+00:00"
        );
        assert!(parse_provider_time("no data").is_none());
        assert!(parse_provider_time("2026-02-30 02:10 UTC").is_none());
    }

    #[test]
    fn latest_only_query_and_target_contract_is_enforced() {
        let good_target = DiscoveryTarget {
            source: SOURCE.into(),
            product: Some(PRODUCT.into()),
            station: Some("CGK".into()),
        };
        let good_query = Query { source: Some(SOURCE.into()), ..Query::default() };
        validate_target_and_query(&good_target, &good_query).unwrap();
        let historical = Query {
            selector: TimeSelector::At { time: "2026-09-18T02:10:00Z".into() },
            ..good_query.clone()
        };
        assert!(validate_target_and_query(&good_target, &historical).is_err());
        let unsupported_station = DiscoveryTarget { station: Some("../CGK".into()), ..good_target };
        assert!(validate_target_and_query(&unsupported_station, &good_query).is_err());
    }
}
