//! Native discovery adapter for Canada's ECCC CAPPI rainfall GIF directories.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, TimeSelector, parse_utc_time};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use url::Url;

const SOURCE: &str = "ca";
const PRODUCT: &str = "rain";
const HOST: &str = "dd.meteo.gc.ca";
const BASE_PATH_SUFFIX: &str = "/WXO-DD/radar/CAPPI/GIF/";
const LOCATOR_VERSION: &str = "ca-legacy-v1";
const ACCEPT_HTML: &str = "text/html,*/*;q=0.8";

/// Discovers today's ECCC CAPPI rainfall GIF frames, grouped by radar station.
pub struct CaSourceAdapter;

impl SourceAdapter for CaSourceAdapter {
    fn source_id(&self) -> &'static str {
        SOURCE
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(HOST)
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            if target.source != SOURCE {
                return Err(CoreError::Transport("source ca received an invalid target".into()));
            }
            if target.product.as_deref().is_some_and(|product| product != PRODUCT)
                || context.query.product.as_deref().is_some_and(|product| product != PRODUCT)
            {
                return Err(CoreError::Transport(
                    "source ca only supports the rain product".into(),
                ));
            }
            if context.query.base_time.is_some() {
                return Err(CoreError::Transport("source ca does not expose base times".into()));
            }
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source ca discovery requires network access".into(),
                ));
            }

            let root = today_root_url()?;
            let entries = discover_entries(
                &context,
                &root,
                target.station.as_deref(),
                &context.query.stations,
            )
            .await?;
            select_entries(entries, &context.query.selector)
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

fn today_root_url() -> CoreResult<Url> {
    let day = Utc::now().format("%Y%m%d").to_string();
    directory_root_url(&day)
}

fn directory_root_url(day: &str) -> CoreResult<Url> {
    if day.len() != 8 || !day.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CoreError::Transport("source ca directory date is invalid".into()));
    }
    NaiveDate::parse_from_str(day, "%Y%m%d")
        .map_err(|_| CoreError::Transport("source ca directory date is invalid".into()))?;
    Url::parse(&format!("https://{HOST}/{day}{BASE_PATH_SUFFIX}"))
        .map_err(|_| CoreError::Transport("source ca directory URL is invalid".into()))
}

async fn discover_entries(
    context: &SourceContext,
    root: &Url,
    target_station: Option<&str>,
    requested_stations: &[String],
) -> CoreResult<Vec<Entry>> {
    let root_links = fetch_links(context, root).await?;
    let mut station_directories = BTreeMap::<String, Url>::new();
    for href in root_links {
        if !href.ends_with('/') {
            continue;
        }
        let station = href.trim_matches('/');
        if !station.starts_with("CA") || station.contains('/') || !is_safe_path_component(station) {
            continue;
        }
        if !station_is_requested(station, target_station, requested_stations) {
            continue;
        }
        if let Some(url) = safe_join(root, root, &href) {
            station_directories.entry(station.to_owned()).or_insert(url);
        }
    }

    let workers = context.discovery_workers.max(1);
    let results = stream::iter(station_directories)
        .map(|(station, station_url)| async move {
            let station_links = fetch_links(context, &station_url).await?;
            let subdirectories = station_links
                .iter()
                .filter_map(|href| {
                    if !href.ends_with('/') {
                        return None;
                    }
                    let name = href.trim_matches('/');
                    if name.is_empty()
                        || matches!(name, "." | "..")
                        || name.contains('/')
                        || !is_safe_path_component(name)
                    {
                        return None;
                    }
                    safe_join(&station_url, root, href)
                })
                .collect::<Vec<_>>();

            let targets = if subdirectories.is_empty() {
                vec![(station_url.clone(), station_links)]
            } else {
                let mut targets = Vec::with_capacity(subdirectories.len());
                for url in subdirectories {
                    targets.push((url.clone(), fetch_links(context, &url).await?));
                }
                targets
            };

            let mut entries = Vec::new();
            for (base, links) in targets {
                for href in links {
                    let Some(url) = safe_join(&base, root, &href) else {
                        continue;
                    };
                    let Some((name, valid_time)) = parse_rain_gif_href(&href) else {
                        continue;
                    };
                    entries.push(Entry {
                        station: station.clone(),
                        valid_time,
                        revision: name.to_owned(),
                        url: url.to_string(),
                    });
                }
            }
            Ok::<_, CoreError>(entries)
        })
        .buffer_unordered(workers)
        .try_collect::<Vec<_>>()
        .await?;
    Ok(results.into_iter().flatten().collect())
}

async fn fetch_links(context: &SourceContext, url: &Url) -> CoreResult<Vec<String>> {
    let payload = context
        .http_transport
        .get_bytes_coalesced(url.as_str(), &[("Accept", ACCEPT_HTML)], &context.request_coalescer)
        .await
        .map_err(sanitize_request_error)?;
    let html = String::from_utf8_lossy(&payload);
    Ok(extract_hrefs(&html).into_iter().map(str::to_owned).collect())
}

fn sanitize_request_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source ca discovery requires network access".into())
        }
        CoreError::ResourceLimit(_) => CoreError::ResourceLimit(
            "source ca directory response exceeds configured limits".into(),
        ),
        _ => CoreError::Transport("source ca directory request failed".into()),
    }
}

fn extract_hrefs(html: &str) -> Vec<&str> {
    let bytes = html.as_bytes();
    let mut hrefs = Vec::new();
    let mut cursor = 0;
    while cursor + 5 < bytes.len() {
        let Some(relative) =
            bytes[cursor..].windows(4).position(|window| window.eq_ignore_ascii_case(b"href"))
        else {
            break;
        };
        let start = cursor + relative;
        let quote_position = start + 5;
        if bytes.get(start + 4) != Some(&b'=')
            || !matches!(bytes.get(quote_position), Some(b'\'' | b'"'))
        {
            cursor = start + 4;
            continue;
        }
        let quote = bytes[quote_position];
        let value_start = quote_position + 1;
        let Some(value_end) = bytes[value_start..].iter().position(|byte| *byte == quote) else {
            break;
        };
        let value_end = value_start + value_end;
        if value_end > value_start {
            // Both bounds follow ASCII quote bytes and are UTF-8 boundaries.
            hrefs.push(&html[value_start..value_end]);
        }
        cursor = value_end.saturating_add(1);
    }
    hrefs
}

fn is_safe_path_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn station_is_requested(
    station: &str,
    target_station: Option<&str>,
    requested_stations: &[String],
) -> bool {
    target_station.is_none_or(|requested| requested == station)
        && (requested_stations.is_empty()
            || requested_stations.iter().any(|requested| requested == station))
}

/// Resolve a listing href only when it stays on the provider origin and under
/// both the current directory and the current day's CAPPI directory.
fn safe_join(base: &Url, root: &Url, href: &str) -> Option<Url> {
    if href.is_empty() || href.chars().any(char::is_control) || href.contains('\\') {
        return None;
    }
    let joined = base.join(href).ok()?;
    if joined.scheme() != "https"
        || joined.host_str() != Some(HOST)
        || joined.origin() != root.origin()
        || !joined.username().is_empty()
        || joined.password().is_some()
        || !joined.path().starts_with(base.path())
        || !joined.path().starts_with(root.path())
    {
        return None;
    }
    let lowercase_path = joined.path().to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "%00"].iter().any(|marker| lowercase_path.contains(marker)) {
        return None;
    }
    Some(joined)
}

fn parse_rain_gif_href(href: &str) -> Option<(&str, DateTime<Utc>)> {
    let without_query = href.split('?').next()?;
    let without_fragment = without_query.split('#').next()?;
    let path = without_fragment.trim_end_matches('/');
    let name = path.rsplit('/').next()?;
    if !name.to_ascii_uppercase().ends_with("RAIN.GIF") {
        return None;
    }
    let bytes = name.as_bytes();
    let digits_length = [14, 12, 10, 8].into_iter().find(|length| {
        bytes.len() > *length
            && bytes[..*length].iter().all(u8::is_ascii_digit)
            && bytes[*length] == b'_'
    })?;
    let digits = &name[..digits_length];
    let date = NaiveDate::parse_from_str(&digits[..8], "%Y%m%d").ok()?;
    let hour = if digits_length >= 10 { digits[8..10].parse().ok()? } else { 0 };
    let minute = if digits_length >= 12 { digits[10..12].parse().ok()? } else { 0 };
    let second = if digits_length >= 14 { digits[12..14].parse().ok()? } else { 0 };
    let valid_time = date.and_hms_opt(hour, minute, second)?.and_utc();
    Some((name, valid_time))
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
                CoreError::Transport("source ca query contains an invalid timestamp".into())
            })?;
            entries.into_iter().filter(|entry| entry.valid_time == requested).collect::<Vec<_>>()
        }
        TimeSelector::Range { start, end } => {
            let start = parse_utc_time(start).map_err(|_| {
                CoreError::Transport("source ca query contains an invalid timestamp".into())
            })?;
            let end = parse_utc_time(end).map_err(|_| {
                CoreError::Transport("source ca query contains an invalid timestamp".into())
            })?;
            entries
                .into_iter()
                .filter(|entry| entry.valid_time >= start && entry.valid_time < end)
                .collect::<Vec<_>>()
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
        revision: Some(entry.revision),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": entry.url,
            "artifacts": [],
            "station": entry.station,
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source ca frame identity could not be computed".into())
    })?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(station: &str, revision: &str) -> Entry {
        let valid_time = parse_rain_gif_href(revision)
            .map(|(_, valid_time)| valid_time)
            .expect("test revision timestamp");
        Entry {
            station: station.into(),
            valid_time,
            revision: revision.into(),
            url: format!("https://{HOST}/20260918{BASE_PATH_SUFFIX}{station}/1.5/{revision}"),
        }
    }

    #[test]
    fn parses_supported_filename_timestamp_precisions_and_revisions() {
        for (name, expected) in [
            ("20260918_X_RAIN.gif", "2026-09-18T00:00:00.000000Z"),
            ("2026091803_X_RAIN.gif", "2026-09-18T03:00:00.000000Z"),
            ("202609180354_X_RAIN.gif", "2026-09-18T03:54:00.000000Z"),
            ("20260918035427_X_RAIN.GIF", "2026-09-18T03:54:27.000000Z"),
        ] {
            let (revision, time) = parse_rain_gif_href(name).unwrap();
            assert_eq!(revision, name);
            assert_eq!(time.to_rfc3339_opts(SecondsFormat::Micros, true), expected);
        }

        assert!(parse_rain_gif_href("202609180354_X_CAPPI_RAIN.png").is_none());
        assert!(parse_rain_gif_href("202613010000_X_RAIN.gif").is_none());
        assert!(parse_rain_gif_href("202609180354X_RAIN.gif").is_none());
        assert!(parse_rain_gif_href("20260918035400_X_RAIN.gif?download=1#preview").is_some());
    }

    #[test]
    fn extracts_case_insensitive_html_hrefs_without_network_access() {
        let links = extract_hrefs(
            "<a HREF=\"CA1/\">station</a><a href='1.5/'>level</a><a href=\"\">empty</a>",
        );
        assert_eq!(links, ["CA1/", "1.5/"]);
    }

    #[test]
    fn station_discovery_requires_ca_directory_names_and_applies_requested_station_filters() {
        let root = directory_root_url("20260918").unwrap();
        let links = ["CA1/", "CA2/", "US1/", "CA1/sub/", "CA3/../"];
        let stations = links
            .into_iter()
            .filter(|href| href.ends_with('/'))
            .filter_map(|href| {
                let station = href.trim_matches('/');
                (station.starts_with("CA")
                    && !station.contains('/')
                    && is_safe_path_component(station)
                    && safe_join(&root, &root, href).is_some())
                .then_some(station)
            })
            .collect::<Vec<_>>();
        assert_eq!(stations, ["CA1", "CA2"]);

        let requested = ["CA2".to_owned()];
        let selected = stations
            .into_iter()
            .filter(|station| station_is_requested(station, None, &requested))
            .collect::<Vec<_>>();
        assert_eq!(selected, ["CA2"]);
        assert!(station_is_requested("CA2", Some("CA2"), &requested));
        assert!(!station_is_requested("CA1", Some("CA2"), &requested));
    }

    #[test]
    fn safe_url_join_stays_on_https_provider_host_and_inside_the_listing_tree() {
        let root = directory_root_url("20260918").unwrap();
        assert!(safe_join(&root, &root, "CA1/").is_some());
        assert!(
            safe_join(&root, &root, "https://dd.meteo.gc.ca/20260918/WXO-DD/radar/CAPPI/GIF/CA1/")
                .is_some()
        );
        for href in [
            "//example.com/CA1/",
            "https://dd.meteo.gc.ca.evil.example/CA1/",
            "http://dd.meteo.gc.ca/20260918/WXO-DD/radar/CAPPI/GIF/CA1/",
            "../../outside/",
            "%2e%2e/outside/",
            "https://user@dd.meteo.gc.ca/20260918/WXO-DD/radar/CAPPI/GIF/CA1/",
        ] {
            assert!(safe_join(&root, &root, href).is_none(), "accepted unsafe href {href}");
        }
        let station = safe_join(&root, &root, "CA1/").unwrap();
        assert!(safe_join(&station, &root, "../CA2/").is_none());
    }

    #[test]
    fn latest_is_per_station_at_is_exact_and_range_is_start_inclusive_end_exclusive() {
        let entries = vec![
            entry("CA1", "202609180300_A_RAIN.gif"),
            entry("CA1", "202609180354_A_RAIN.gif"),
            entry("CA1", "202609180354_B_RAIN.gif"),
            entry("CA2", "202609180300_C_RAIN.gif"),
        ];
        let latest = select_entries(entries.clone(), &TimeSelector::Latest).unwrap();
        assert_eq!(latest.len(), 3);
        assert!(
            latest
                .iter()
                .filter(|frame| frame.station.as_deref() == Some("CA1"))
                .all(|frame| frame.valid_time.starts_with("2026-09-18T03:54"))
        );
        assert!(
            latest
                .iter()
                .filter(|frame| frame.station.as_deref() == Some("CA2"))
                .all(|frame| frame.valid_time == "2026-09-18T03:00:00.000000Z")
        );

        let at = select_entries(
            entries.clone(),
            &TimeSelector::At { time: "2026-09-18T03:54:00Z".into() },
        )
        .unwrap();
        assert_eq!(at.len(), 2);

        let range = select_entries(
            entries,
            &TimeSelector::Range {
                start: "2026-09-18T03:00:00Z".into(),
                end: "2026-09-18T03:54:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 2);
        assert!(range.iter().all(|frame| frame.valid_time == "2026-09-18T03:00:00.000000Z"));
    }
}
