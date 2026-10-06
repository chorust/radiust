//! Native raw-frame discovery for BMKG's regular Indonesia radar source.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector, parse_utc_time};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "id";
const PRODUCT: &str = "composite";
const API_URL: &str = "https://radar.bmkg.go.id:8090/sidarmaimage";
const API_HOST: &str = "radar.bmkg.go.id";
const API_PATH: &str = "/sidarmaimage";
const ARTIFACT_DOMAIN: &str = "bmkg.go.id";
const LOCATOR_VERSION: &str = "id-legacy-v1";
const REQUEST_HEADERS: [(&str, &str); 2] = [
    ("Referer", "https://kalteng.bmkg.go.id/"),
    ("User-Agent", "Mozilla/5.0 (compatible; radiust/1)"),
];
const RADAR_RESOURCE: &str = include_str!("../../resources/builtin/sources/id.json");

/// Discovers BMKG regular-ID composite images. The adapter only exposes raw
/// image artifacts; no scientific decoder or geometry is implied here.
pub struct IdSourceAdapter;

impl SourceAdapter for IdSourceAdapter {
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
        if !radars.iter().any(|radar| radar == station) {
            return false;
        }
        let Some(valid_time) = parse_frame_time(&frame.valid_time) else {
            return false;
        };
        let expected_revision = format!("{station}-{}", valid_time.timestamp());
        let Some((safe_url, _, _)) = safe_artifact_url(url.as_str()) else {
            return false;
        };
        let Some(locator_url) = frame.locator.get("url").and_then(Value::as_str) else {
            return false;
        };
        frame.revision.as_deref() == Some(expected_revision.as_str())
            && frame.locator.get("station").and_then(Value::as_str) == Some(station)
            && frame.locator.get("revision").and_then(Value::as_str)
                == Some(expected_revision.as_str())
            && frame.locator.get("artifacts").and_then(Value::as_array).is_some_and(Vec::is_empty)
            && safe_url == locator_url
            && locator_url == url.as_str()
            && logical_id(frame).ok().as_deref() == Some(frame.logical_id.as_str())
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            let radars = radar_inventory()?;
            validate_target_and_query(&target, &context.query, &radars)?;
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source id discovery requires network access".into(),
                ));
            }

            let token = token_from_options(&context.source_options)?;
            let selected = selected_radar_ids(&radars, context.source_options.get("radar_ids"))?
                .into_iter()
                .filter(|station| {
                    target.station.as_deref().is_none_or(|requested| requested == station)
                        && (context.query.stations.is_empty()
                            || context.query.stations.iter().any(|requested| requested == station))
                })
                .collect::<Vec<_>>();
            if selected.is_empty() {
                return Ok(Vec::new());
            }

            let request_context = context.clone();
            let request_token = Arc::<str>::from(token);
            let payloads = stream::iter(selected)
                .map(move |station| {
                    let context = request_context.clone();
                    let token = request_token.clone();
                    async move {
                        let address = api_url(&station, &token)?;
                        let payload = context
                            .http_transport
                            .get_bytes_coalesced(
                                &address,
                                &REQUEST_HEADERS,
                                &context.request_coalescer,
                            )
                            .await
                            .map_err(sanitize_request_error)?;
                        parse_response(&station, &payload)
                    }
                })
                .buffer_unordered(context.discovery_workers.max(1))
                .try_collect::<Vec<_>>()
                .await?;

            let entries = payloads.into_iter().flatten().collect();
            let entries = select_entries(entries, &context.query.selector)?;
            let mut frames =
                entries.into_iter().map(frame_from_entry).collect::<CoreResult<Vec<_>>>()?;
            frames.sort_by(|left, right| {
                left.valid_time
                    .cmp(&right.valid_time)
                    .then_with(|| left.station.cmp(&right.station))
                    .then_with(|| left.logical_id.cmp(&right.logical_id))
            });
            Ok(frames)
        })
    }
}

#[derive(Debug, Deserialize)]
struct RadarResource {
    schema_version: u32,
    source: String,
    radar_ids: Vec<String>,
}

#[derive(Clone, Debug)]
struct Entry {
    station: String,
    valid_time: DateTime<Utc>,
    revision: String,
    url: String,
}

fn validate_target_and_query(
    target: &DiscoveryTarget,
    query: &Query,
    radars: &[String],
) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source id received a mismatched source query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source id only supports the composite product".into()));
    }
    if target.station.as_deref().is_some_and(|station| !radars.iter().any(|radar| radar == station))
        || query.stations.iter().any(|station| !radars.iter().any(|radar| radar == station))
    {
        return Err(CoreError::Transport("source id requires a supported radar".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source id does not expose base times".into()));
    }
    validate_selector(&query.selector)
}

fn validate_selector(selector: &TimeSelector) -> CoreResult<()> {
    match selector {
        TimeSelector::Latest => Ok(()),
        TimeSelector::At { time } => parse_selector_time(time).map(|_| ()),
        TimeSelector::Range { start, end } => {
            let start = parse_selector_time(start)?;
            let end = parse_selector_time(end)?;
            if start < end {
                Ok(())
            } else {
                Err(CoreError::Transport("source id query range start must precede end".into()))
            }
        }
    }
}

fn radar_inventory() -> CoreResult<Vec<String>> {
    radar_inventory_from_json(RADAR_RESOURCE)
}

fn radar_inventory_from_json(payload: &str) -> CoreResult<Vec<String>> {
    let resource: RadarResource = serde_json::from_str(payload)
        .map_err(|_| CoreError::Transport("source id radar resource is invalid".into()))?;
    if resource.schema_version != 1 || resource.source != SOURCE || resource.radar_ids.is_empty() {
        return Err(CoreError::Transport("source id radar resource is invalid".into()));
    }
    let mut seen = BTreeSet::new();
    for radar in &resource.radar_ids {
        if !valid_radar_id(radar) || !seen.insert(radar.as_str()) {
            return Err(CoreError::Transport("source id radar resource is invalid".into()));
        }
    }
    Ok(resource.radar_ids)
}

fn valid_radar_id(value: &str) -> bool {
    (2..=8).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_uppercase())
}

fn token_from_options(options: &BTreeMap<String, serde_yaml_ng::Value>) -> CoreResult<String> {
    options
        .get("token")
        .and_then(serde_yaml_ng::Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| CoreError::Transport("source id requires sources.id.token".into()))
}

fn selected_radar_ids(
    radars: &[String],
    configured: Option<&serde_yaml_ng::Value>,
) -> CoreResult<Vec<String>> {
    let Some(values) = configured else {
        return Ok(radars.to_vec());
    };
    let Some(values) = values.as_sequence() else {
        return Err(CoreError::Transport("sources.id.radar_ids must be a list".into()));
    };
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for value in values {
        let Some(value) = value.as_str() else {
            return Err(CoreError::Transport("sources.id.radar_ids must be a list".into()));
        };
        let station = value.trim().to_ascii_uppercase();
        if !radars.iter().any(|radar| radar == &station) {
            return Err(CoreError::Transport("source id has an unknown radar id".into()));
        }
        if seen.insert(station.clone()) {
            selected.push(station);
        }
    }
    Ok(selected)
}

fn api_url(station: &str, token: &str) -> CoreResult<String> {
    if !valid_radar_id(station) {
        return Err(CoreError::Transport("source id has an invalid radar id".into()));
    }
    let mut url = Url::parse(API_URL)
        .map_err(|_| CoreError::Transport("source id has an invalid API endpoint".into()))?;
    if url.scheme() != "https"
        || url.host_str() != Some(API_HOST)
        || url.port() != Some(8090)
        || url.path() != API_PATH
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CoreError::Transport("source id has an invalid API endpoint".into()));
    }
    url.query_pairs_mut().append_pair("token", token).append_pair("radar", station);
    Ok(url.into())
}

fn sanitize_request_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source id discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source id metadata response exceeds configured limits".into())
        }
        _ => CoreError::Transport("source id metadata request failed".into()),
    }
}

fn parse_response(station: &str, payload: &[u8]) -> CoreResult<Vec<Entry>> {
    let document: Value = serde_json::from_slice(payload)
        .map_err(|_| CoreError::Transport("source id returned invalid discovery JSON".into()))?;
    let Some(bucket) = document.get("LastOneHour").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    let (Some(files), Some(times)) = (
        bucket.get("file").and_then(Value::as_array),
        bucket.get("timeUTC").and_then(Value::as_array),
    ) else {
        return Ok(Vec::new());
    };

    let mut entries = Vec::new();
    for (file, time) in files.iter().zip(times) {
        let (Some(file), Some(time)) = (file.as_str(), time.as_str()) else {
            continue;
        };
        let Some(valid_time) = parse_provider_time(time) else {
            continue;
        };
        let Some((url, _, _)) = safe_artifact_url(file) else {
            continue;
        };
        entries.push(Entry {
            station: station.to_owned(),
            revision: format!("{station}-{}", valid_time.timestamp()),
            valid_time,
            url,
        });
    }
    Ok(entries)
}

fn parse_provider_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("no data") {
        return None;
    }
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
    const NAIVE_FORMATS: [&str; 5] = [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d",
    ];
    if let Some(time) = NAIVE_FORMATS
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(&normalized, format).ok())
    {
        return Some(time.and_utc());
    }
    NaiveDate::parse_from_str(&normalized, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)
        .map(|time| time.and_utc())
}

fn safe_artifact_url(value: &str) -> Option<(String, String, String)> {
    if value.chars().any(char::is_control) || value.contains('\\') {
        return None;
    }
    let (_, authority_and_path) = value.split_once("://")?;
    let authority = authority_and_path.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains(['@', ':', '%']) {
        return None;
    }
    let url = Url::parse(value).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if url.scheme() != "https"
        || !is_bmkg_host(&host)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = raw_path(value)?;
    if !safe_path(path) || path != url.path() {
        return None;
    }
    Some((url.to_string(), host, path.to_owned()))
}

fn raw_path(value: &str) -> Option<&str> {
    let (_, authority_and_path) = value.split_once("://")?;
    let path_start = authority_and_path.find('/')?;
    let path = &authority_and_path[path_start..];
    let end = path.find(['?', '#']).unwrap_or(path.len());
    Some(&path[..end])
}

fn safe_path(path: &str) -> bool {
    if !path.starts_with('/') || path.len() < 2 || path.ends_with('/') || path.contains("//") {
        return false;
    }
    path[1..].split('/').all(|segment| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
            })
    })
}

fn is_bmkg_host(host: &str) -> bool {
    host.eq_ignore_ascii_case(ARTIFACT_DOMAIN)
        || host
            .to_ascii_lowercase()
            .strip_suffix(ARTIFACT_DOMAIN)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn select_entries(entries: Vec<Entry>, selector: &TimeSelector) -> CoreResult<Vec<Entry>> {
    let mut selected = match selector {
        TimeSelector::Latest => {
            let mut latest = BTreeMap::<String, DateTime<Utc>>::new();
            for entry in &entries {
                latest
                    .entry(entry.station.clone())
                    .and_modify(|time| *time = (*time).max(entry.valid_time))
                    .or_insert(entry.valid_time);
            }
            entries
                .into_iter()
                .filter(|entry| latest.get(&entry.station) == Some(&entry.valid_time))
                .collect()
        }
        TimeSelector::At { time } => {
            let requested = parse_selector_time(time)?;
            entries.into_iter().filter(|entry| entry.valid_time == requested).collect()
        }
        TimeSelector::Range { start, end } => {
            let start = parse_selector_time(start)?;
            let end = parse_selector_time(end)?;
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
    selected.sort_by(|left, right| {
        left.valid_time
            .cmp(&right.valid_time)
            .then_with(|| left.station.cmp(&right.station))
            .then_with(|| left.revision.cmp(&right.revision))
            .then_with(|| left.url.cmp(&right.url))
    });
    Ok(selected)
}

fn parse_selector_time(value: &str) -> CoreResult<DateTime<Utc>> {
    parse_utc_time(value)
        .map_err(|_| CoreError::Transport("source id query contains an invalid timestamp".into()))
}

fn parse_frame_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value).ok().map(|time| time.with_timezone(&Utc))
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
        CoreError::Transport("source id frame identity could not be computed".into())
    })?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;

    const SAMPLE_URL: &str = "https://radar.bmkg.go.id/radar/ACE/frame_20260918.png";
    const SYNTHETIC_TOKEN: &str = "unit-test-token-only";

    fn entry(station: &str, time: &str, name: &str) -> Entry {
        let valid_time = parse_provider_time(time).expect("valid fixture timestamp");
        Entry {
            station: station.into(),
            revision: format!("{station}-{}", valid_time.timestamp()),
            valid_time,
            url: format!("https://radar.bmkg.go.id/radar/{name}.png"),
        }
    }

    #[test]
    fn embedded_inventory_is_valid_and_complete() {
        let radars = radar_inventory().unwrap();
        assert_eq!(radars.len(), 47);
        assert!(radars.iter().any(|radar| radar == "ACE"));
        assert!(radars.iter().any(|radar| radar == "NGW"));

        assert!(
            radar_inventory_from_json(
                r#"{"schema_version":1,"source":"id","radar_ids":["ACE","NGW"]}"#
            )
            .is_ok()
        );
        for invalid in [
            r#"{"schema_version":2,"source":"id","radar_ids":["ACE"]}"#,
            r#"{"schema_version":1,"source":"id_sidarma","radar_ids":["ACE"]}"#,
            r#"{"schema_version":1,"source":"id","radar_ids":[]}"#,
            r#"{"schema_version":1,"source":"id","radar_ids":["ACE","ACE"]}"#,
            r#"{"schema_version":1,"source":"id","radar_ids":["ace"]}"#,
            "not-json",
        ] {
            assert!(radar_inventory_from_json(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn source_options_require_external_token_and_validate_configured_radars() {
        let radars = radar_inventory().unwrap();
        let options = BTreeMap::from([
            ("token".to_owned(), serde_yaml_ng::Value::String(format!(" {SYNTHETIC_TOKEN} "))),
            (
                "radar_ids".to_owned(),
                serde_yaml_ng::Value::Sequence(vec![
                    serde_yaml_ng::Value::String(" ace ".into()),
                    serde_yaml_ng::Value::String("NGW".into()),
                    serde_yaml_ng::Value::String("ACE".into()),
                ]),
            ),
        ]);
        assert_eq!(token_from_options(&options).unwrap(), SYNTHETIC_TOKEN);
        assert_eq!(selected_radar_ids(&radars, None).unwrap().len(), 47);
        assert_eq!(
            selected_radar_ids(&radars, options.get("radar_ids")).unwrap(),
            vec!["ACE", "NGW"]
        );
        assert!(token_from_options(&BTreeMap::new()).is_err());
        assert!(
            token_from_options(&BTreeMap::from([(
                "token".into(),
                serde_yaml_ng::Value::String("  ".into())
            )]))
            .is_err()
        );
        assert!(
            selected_radar_ids(&radars, Some(&serde_yaml_ng::Value::String("ACE".into()))).is_err()
        );
        assert!(
            selected_radar_ids(
                &radars,
                Some(&serde_yaml_ng::Value::Sequence(vec![serde_yaml_ng::Value::Number(7.into())]))
            )
            .is_err()
        );
        assert!(
            selected_radar_ids(
                &radars,
                Some(&serde_yaml_ng::Value::Sequence(vec![serde_yaml_ng::Value::String(
                    "UNKNOWN".into()
                )]))
            )
            .is_err()
        );
    }

    #[test]
    fn api_requests_use_the_fixed_https_endpoint_and_encode_token_at_request_time() {
        let address = api_url("ACE", "token with & reserved chars").unwrap();
        let url = Url::parse(&address).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some(API_HOST));
        assert_eq!(url.port(), Some(8090));
        assert_eq!(url.path(), API_PATH);
        let query = url.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(
            query.get("token").map(|value| value.as_ref()),
            Some("token with & reserved chars")
        );
        assert_eq!(query.get("radar").map(|value| value.as_ref()), Some("ACE"));
        assert!(api_url("bad/id", SYNTHETIC_TOKEN).is_err());
    }

    #[test]
    fn artifact_urls_require_https_bmkg_hosts_and_safe_paths() {
        assert!(safe_artifact_url(SAMPLE_URL).is_some());
        assert!(safe_artifact_url("https://bmkg.go.id/a/b.png").is_some());
        assert!(safe_artifact_url("https://sub.radar.bmkg.go.id/a/b.png").is_some());
        for unsafe_url in [
            "http://radar.bmkg.go.id/a.png",
            "https://bmkg.go.id.attacker.example/a.png",
            "https://attackerbmkg.go.id/a.png",
            "https://radar.bmkg.go.id:8443/a.png",
            "https://user@radar.bmkg.go.id/a.png",
            "https://radar.bmkg.go.id/a/../b.png",
            "https://radar.bmkg.go.id/a/%2e%2e/b.png",
            "https://radar.bmkg.go.id/a%2fb.png",
            "https://radar.bmkg.go.id/a%252fb.png",
            "https://radar.bmkg.go.id/a\\b.png",
            "https://radar.bmkg.go.id//a.png",
            "https://radar.bmkg.go.id/a.png?token=hidden",
            "https://radar.bmkg.go.id/a.png#fragment",
            "https://radar.bmkg.go.id/",
        ] {
            assert!(safe_artifact_url(unsafe_url).is_none(), "accepted {unsafe_url}");
        }
    }

    #[test]
    fn parses_last_hour_url_and_time_arrays_and_skips_bad_pairs() {
        let payload = br#"{
          "LastOneHour": {
            "file": [
              "https://radar.bmkg.go.id/radar/ACE/one.png",
              "https://sub.bmkg.go.id/radar/ACE/two.png",
              "https://example.invalid/unsafe.png",
              "https://radar.bmkg.go.id/radar/ACE/bad-time.png",
              12
            ],
            "timeUTC": [
              "2026-09-18 04:00:00 UTC",
              "2026-09-18T07:10:00+03:00",
              "2026-09-18 04:20:00 UTC",
              "not-a-time",
              "2026-09-18 04:40:00 UTC"
            ]
          }
        }"#;
        let entries = parse_response("ACE", payload).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].valid_time.to_rfc3339(), "2026-09-18T04:00:00+00:00");
        assert_eq!(entries[1].valid_time.to_rfc3339(), "2026-09-18T04:10:00+00:00");
        assert_eq!(entries[0].revision, format!("ACE-{}", entries[0].valid_time.timestamp()));
        assert!(parse_response("ACE", b"{").is_err());
        assert!(
            parse_response("ACE", br#"{"LastOneHour":{"file":"x","timeUTC":"y"}}"#)
                .unwrap()
                .is_empty()
        );
        assert!(parse_response("ACE", br#"{"Latest":{}}"#).unwrap().is_empty());
    }

    #[test]
    fn provider_times_normalize_offsets_and_naive_values_to_utc() {
        assert_eq!(
            parse_provider_time(" 2026-09-18 04:00:00 UTC ").unwrap().to_rfc3339(),
            "2026-09-18T04:00:00+00:00"
        );
        assert_eq!(
            parse_provider_time("2026-09-18T07:00:00+03:00").unwrap().to_rfc3339(),
            "2026-09-18T04:00:00+00:00"
        );
        assert_eq!(
            parse_provider_time("2026-09-18 04:00:00.125").unwrap().timestamp_subsec_millis(),
            125
        );
        assert_eq!(
            parse_provider_time("2026-09-18").unwrap().to_rfc3339(),
            "2026-09-18T00:00:00+00:00"
        );
        assert!(parse_provider_time("No Data").is_none());
        assert!(parse_provider_time("invalid").is_none());
    }

    #[test]
    fn historical_selectors_and_latest_are_applied_per_station() {
        let entries = vec![
            entry("ACE", "2026-09-18 04:00:00 UTC", "ace-old"),
            entry("ACE", "2026-09-18 04:10:00 UTC", "ace-new"),
            entry("NGW", "2026-09-18 04:05:00 UTC", "ngw-latest"),
        ];
        let latest = select_entries(entries.clone(), &TimeSelector::Latest).unwrap();
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[0].station, "NGW");
        assert_eq!(latest[1].station, "ACE");

        let at = select_entries(
            entries.clone(),
            &TimeSelector::At { time: "2026-09-18T07:10:00+03:00".into() },
        )
        .unwrap();
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].revision, entries[1].revision);

        let range = select_entries(
            entries,
            &TimeSelector::Range {
                start: "2026-09-18T04:00:00Z".into(),
                end: "2026-09-18T04:10:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 2);
        assert!(range.iter().all(
            |entry| entry.valid_time < parse_provider_time("2026-09-18 04:10:00 UTC").unwrap()
        ));
        assert!(select_entries(Vec::new(), &TimeSelector::At { time: "invalid".into() }).is_err());
    }

    #[test]
    fn target_contract_rejects_wrong_source_product_base_time_and_stations() {
        let radars = radar_inventory().unwrap();
        let target = DiscoveryTarget {
            source: SOURCE.into(),
            product: Some(PRODUCT.into()),
            station: Some("ACE".into()),
        };
        assert!(validate_target_and_query(&target, &Query::default(), &radars).is_ok());
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { source: "id_sidarma".into(), ..target.clone() },
                &Query::default(),
                &radars
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { product: Some("cmax".into()), ..target.clone() },
                &Query::default(),
                &radars
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target,
                &Query { base_time: Some("2026-09-18T00:00:00Z".into()), ..Query::default() },
                &radars
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { station: Some("ZZZ".into()), ..target },
                &Query::default(),
                &radars
            )
            .is_err()
        );
    }

    #[test]
    fn legacy_locator_identity_is_stable_and_artifact_substitution_is_rejected() {
        let frame =
            frame_from_entry(entry("ACE", "2026-09-18 04:00:00 UTC", "frame_20260918")).unwrap();
        assert_eq!(frame.product, PRODUCT);
        assert_eq!(frame.station.as_deref(), Some("ACE"));
        assert_eq!(frame.locator_version, "id-legacy-v1");
        assert_eq!(frame.revision.as_deref(), Some("ACE-1789704000"));
        assert_eq!(frame.locator["url"], "https://radar.bmkg.go.id/radar/frame_20260918.png");
        assert_eq!(frame.locator["artifacts"], json!([]));
        assert_eq!(frame.locator["station"], "ACE");
        assert_eq!(frame.locator["revision"], "ACE-1789704000");
        assert_eq!(frame.logical_id, logical_id(&frame).unwrap());
        assert_eq!(
            frame.logical_id,
            "f1da65790d52405ec502965027c601315e256adfc5d9c7cb15b246f18cff113c"
        );
        assert!(frame_identity(&frame).unwrap()["locator"].get("url").is_none());

        let adapter = IdSourceAdapter;
        assert!(adapter.allows_artifact_url(
            &frame,
            &Url::parse(&frame.locator["url"].as_str().unwrap()).unwrap()
        ));
        assert!(!adapter.allows_artifact_url(
            &frame,
            &Url::parse("https://radar.bmkg.go.id/radar/other.png").unwrap()
        ));
        let mut changed = frame.clone();
        changed.locator["revision"] = json!("ACE-1");
        assert!(!adapter.allows_artifact_url(
            &changed,
            &Url::parse(&frame.locator["url"].as_str().unwrap()).unwrap()
        ));
    }

    #[test]
    fn transport_errors_are_sanitized_before_they_can_expose_request_tokens() {
        let errors = [
            CoreError::Transport(format!("failed URL contains {SYNTHETIC_TOKEN}")),
            CoreError::NetworkDisabled(format!("request URL contains {SYNTHETIC_TOKEN}")),
            CoreError::ResourceLimit(format!("request URL contains {SYNTHETIC_TOKEN}")),
        ];
        for error in errors {
            let public = sanitize_request_error(error);
            assert!(!public.to_string().contains(SYNTHETIC_TOKEN));
        }
        assert!(matches!(sanitize_request_error(CoreError::Cancelled), CoreError::Cancelled));
    }
}
