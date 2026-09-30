//! Native discovery adapter for Cambodia's timestamped radar slideshow.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector, parse_utc_time};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "cam";
const PRODUCT: &str = "composite";
const PAGE_URL: &str = "http://www.cambodiameteo.com/slideshow?menu=117&lang=en&domain=CAMBODIA";
const BASE_URL: &str = "http://www.cambodiameteo.com";
const HOST: &str = "www.cambodiameteo.com";
const LOCATOR_VERSION: &str = "cam-legacy-v1";
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;

/// Discovers image frames from the provider page. Scientific decoding remains
/// evidence-gated; the adapter only returns and acquires the original image.
pub struct CamSourceAdapter;

impl SourceAdapter for CamSourceAdapter {
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
            || logical_id(frame).ok().as_deref() != Some(frame.logical_id.as_str())
        {
            return false;
        }
        let Some(expected) = frame.locator.get("url").and_then(Value::as_str) else {
            return false;
        };
        let Some(revision) = frame.revision.as_deref() else {
            return false;
        };
        let Ok(expected_url) = Url::parse(expected) else {
            return false;
        };
        let Ok(valid_time) = parse_utc_time(&frame.valid_time) else {
            return false;
        };
        let filename = expected_url.path().rsplit('/').next().unwrap_or_default();
        url == &expected_url
            && url.scheme() == "http"
            && url.port().is_none()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && self.allows_artifact_host(url.host_str().unwrap_or_default())
            && frame.locator.get("station").and_then(Value::as_str) == frame.station.as_deref()
            && frame.locator.get("revision").and_then(Value::as_str) == Some(revision)
            && filename.rsplit_once('.').is_some_and(|(stem, _)| stem == revision)
            && timestamp_from_filename(filename) == Some(valid_time)
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
                    "source cam discovery requires network access".into(),
                ));
            }
            let headers = [
                ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
                ("Accept-Language", "zh,en;q=0.9"),
                (
                    "User-Agent",
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
                ),
            ];
            let payload = context
                .http_transport
                .get_bytes_coalesced(PAGE_URL, &headers, &context.request_coalescer)
                .await
                .map_err(sanitize_page_error)?;
            let entries = parse_page(&payload)?;
            select_frames(entries, &context.query)
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    url: String,
    station: String,
    valid_time: DateTime<Utc>,
    revision: String,
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source cam received a mismatched source query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source cam only supports the composite product".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source cam does not expose base times".into()));
    }
    Ok(())
}

fn sanitize_page_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source cam discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => {
            CoreError::ResourceLimit("source cam page exceeds configured limits".into())
        }
        _ => CoreError::Transport("source cam slideshow request failed".into()),
    }
}

fn parse_page(payload: &[u8]) -> CoreResult<Vec<Entry>> {
    if payload.len() > MAX_PAGE_BYTES {
        return Err(CoreError::ResourceLimit("source cam page exceeds the parser limit".into()));
    }
    let text = String::from_utf8_lossy(payload);
    let bytes = text.as_bytes();
    let mut urls = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let start = if bytes[position..].starts_with(b"https://") {
            Some(position)
        } else if bytes[position..].starts_with(b"http://") {
            Some(position)
        } else if bytes[position] == b'/' {
            Some(position)
        } else {
            None
        };
        let Some(start) = start else {
            position += 1;
            continue;
        };
        let mut end = start;
        while end < bytes.len()
            && !matches!(bytes[end], b'"' | b'\'' | b'<' | b'>' | b' ' | b'\t' | b'\r' | b'\n')
        {
            end += 1;
        }
        if end > start {
            let candidate = text[start..end].trim_end_matches([')', ',', ';']);
            if has_supported_image_suffix(candidate) {
                urls.push(candidate.to_owned());
            }
        }
        position = end.max(start + 1);
    }

    let base = Url::parse(BASE_URL)
        .map_err(|_| CoreError::Transport("source cam base URL is invalid".into()))?;
    let mut unique = BTreeMap::new();
    for candidate in urls {
        let Some(url) = base.join(&candidate).ok().filter(safe_image_url) else {
            continue;
        };
        let filename = url.path().rsplit('/').next().unwrap_or_default();
        let Some(valid_time) = timestamp_from_filename(filename) else {
            continue;
        };
        let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
        let pieces = filename.split('_').collect::<Vec<_>>();
        // Match the public Python adapter's station extraction exactly so that
        // logical IDs remain stable during migration.
        let station = if pieces.len() > 2 { pieces[1] } else { "CAMCOMP" };
        let entry = Entry {
            url: url.to_string(),
            station: station.to_owned(),
            valid_time,
            revision: stem.to_owned(),
        };
        unique
            .entry((entry.station.clone(), entry.valid_time, entry.revision.clone()))
            .or_insert(entry);
    }
    Ok(unique.into_values().collect())
}

fn has_supported_image_suffix(value: &str) -> bool {
    let path = value.split_once('?').map_or(value, |(path, _)| path);
    [".png", ".gif", ".jpg", ".jpeg"]
        .iter()
        .any(|suffix| path.to_ascii_lowercase().ends_with(suffix))
}

fn safe_image_url(url: &Url) -> bool {
    url.scheme() == "http"
        && url.host_str().is_some_and(|host| host.eq_ignore_ascii_case(HOST))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && has_supported_image_suffix(url.path())
}

fn timestamp_from_filename(filename: &str) -> Option<DateTime<Utc>> {
    let marker = "_cambodia_";
    let lower = filename.to_ascii_lowercase();
    let marker_position = lower.find(marker)?;
    let timestamp = filename.get(marker_position.checked_sub(14)?..marker_position)?;
    if timestamp.len() != 14 || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let naive = NaiveDateTime::parse_from_str(timestamp, "%Y%m%d%H%M%S").ok()?;
    Some(DateTime::from_naive_utc_and_offset(naive, Utc))
}

fn select_frames(entries: Vec<Entry>, query: &Query) -> CoreResult<Vec<FrameRef>> {
    select_frames_at(entries, query, Utc::now())
}

fn select_frames_at(
    mut entries: Vec<Entry>,
    query: &Query,
    now: DateTime<Utc>,
) -> CoreResult<Vec<FrameRef>> {
    let selected = match &query.selector {
        TimeSelector::Latest => {
            let mut latest = BTreeMap::<String, Entry>::new();
            for entry in entries {
                let replace = latest
                    .get(&entry.station)
                    .is_none_or(|current| entry.valid_time > current.valid_time);
                if replace {
                    latest.insert(entry.station.clone(), entry);
                }
            }
            latest
                .into_values()
                .filter(|entry| {
                    query.max_age_secs.is_none_or(|max_age| {
                        let age = (now - entry.valid_time).num_microseconds().unwrap_or(0) as f64
                            / 1_000_000.0;
                        age < 0.0 || age <= max_age
                    })
                })
                .collect()
        }
        TimeSelector::At { time } => {
            let selected_time = parse_utc_time(time).map_err(|_| {
                CoreError::Transport("source cam received an invalid at time".into())
            })?;
            entries.retain(|entry| entry.valid_time == selected_time);
            entries
        }
        TimeSelector::Range { start, end } => {
            let start = parse_utc_time(start).map_err(|_| {
                CoreError::Transport("source cam received an invalid time range".into())
            })?;
            let end = parse_utc_time(end).map_err(|_| {
                CoreError::Transport("source cam received an invalid time range".into())
            })?;
            entries.retain(|entry| entry.valid_time >= start && entry.valid_time < end);
            entries
        }
    };

    let mut frames = selected
        .into_iter()
        .filter(|entry| {
            query.stations.is_empty()
                || query.stations.iter().any(|station| station == &entry.station)
        })
        .map(frame_from_entry)
        .collect::<CoreResult<Vec<_>>>()?;
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
            "station": entry.station,
            "revision": entry.revision,
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source cam frame identity could not be computed".into())
    })?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
        <img src="/images/20260925093000_cambodia_COMP.png">
        <script>slides.push("/images/20260925093500_cambodia_COMP.jpg")</script>
        <img src="https://www.cambodiameteo.com/images/20260925093000_cambodia_COMP.png">
        <img src="https://evil.example/20260925094500_cambodia_COMP.png">
        <img src="/images/20260925invalid_cambodia_COMP.png">
    "#;

    fn frame_at(revision: &str) -> FrameRef {
        let valid_time = timestamp_from_filename(&format!("{revision}.png")).unwrap();
        frame_from_entry(Entry {
            url: format!("{BASE_URL}/images/{revision}.png"),
            station: "cambodia".into(),
            valid_time,
            revision: revision.into(),
        })
        .unwrap()
    }

    #[test]
    fn parses_timestamped_same_host_images_and_deduplicates_urls() {
        let entries = parse_page(PAGE.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].station, "cambodia");
        assert_eq!(entries[0].valid_time.to_rfc3339(), "2026-09-25T09:30:00+00:00");
        assert!(entries.iter().all(|entry| entry.url.starts_with(BASE_URL)));
    }

    #[test]
    fn rejects_oversized_pages() {
        let page = vec![b'a'; MAX_PAGE_BYTES + 1];
        assert!(matches!(parse_page(&page), Err(CoreError::ResourceLimit(_))));
    }

    #[test]
    fn latest_is_selected_per_station_and_max_age_is_applied_after_selection() {
        let entries = parse_page(PAGE.as_bytes()).unwrap();
        let latest = select_frames_at(
            entries.clone(),
            &Query::default(),
            parse_utc_time("2026-09-25T10:00:00Z").unwrap(),
        )
        .unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].revision.as_deref(), Some("20260925093500_cambodia_COMP"));
        let stale = Query { max_age_secs: Some(1.0), ..Query::default() };
        assert!(
            select_frames_at(entries, &stale, parse_utc_time("2026-09-25T10:00:00Z").unwrap(),)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn at_range_and_station_filters_match_the_query() {
        let entries = parse_page(PAGE.as_bytes()).unwrap();
        let at = Query {
            selector: TimeSelector::At { time: "2026-09-25T09:30:00Z".into() },
            ..Query::default()
        };
        assert_eq!(select_frames(entries.clone(), &at).unwrap().len(), 1);
        let range = Query {
            selector: TimeSelector::Range {
                start: "2026-09-25T09:30:00Z".into(),
                end: "2026-09-25T09:35:00Z".into(),
            },
            ..Query::default()
        };
        assert_eq!(select_frames(entries.clone(), &range).unwrap().len(), 1);
        let no_match = Query { stations: vec!["other".into()], ..Query::default() };
        assert!(select_frames(entries, &no_match).unwrap().is_empty());
    }

    #[test]
    fn rejects_mismatched_targets_and_query_filters() {
        let target =
            DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None };
        assert!(validate_target_and_query(&target, &Query::default()).is_ok());
        assert!(
            validate_target_and_query(
                &DiscoveryTarget { source: "vn".into(), ..target.clone() },
                &Query::default()
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target,
                &Query { product: Some("rain".into()), ..Query::default() }
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target,
                &Query { base_time: Some("2026-09-25T09:00:00Z".into()), ..Query::default() }
            )
            .is_err()
        );
    }

    #[test]
    fn artifact_url_is_bound_to_the_frame_and_legacy_identity_is_stable() {
        let frame = frame_at("20260925093000_cambodia_COMP");
        let adapter = CamSourceAdapter;
        let expected =
            Url::parse("http://www.cambodiameteo.com/images/20260925093000_cambodia_COMP.png")
                .unwrap();
        assert!(adapter.allows_artifact_host(HOST));
        assert!(adapter.allows_artifact_url(&frame, &expected));
        assert!(
            !adapter
                .allows_artifact_url(&frame, &Url::parse("http://evil.example/image.png").unwrap())
        );
        assert!(
            !adapter.allows_artifact_url(
                &frame,
                &Url::parse(
                    "https://www.cambodiameteo.com/images/20260925093000_cambodia_COMP.png"
                )
                .unwrap()
            )
        );
        assert!(
            !adapter.allows_artifact_url(
                &frame,
                &Url::parse("http://www.cambodiameteo.com/images/20260925093500_cambodia_COMP.png")
                    .unwrap()
            )
        );
        assert_eq!(
            frame.logical_id,
            "4cedf954ca220692327e1f8585d68d78f36f8f0177fa4671949a3a4d604e761f"
        );
    }
}
