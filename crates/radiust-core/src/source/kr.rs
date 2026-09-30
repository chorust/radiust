//! Native discovery adapter for Korea Meteorological Administration station radar.

use crate::errors::{CoreError, CoreResult};
use crate::identity::logical_id;
use crate::model::{DiscoveryTarget, FrameRef, Query, TimeSelector};
use crate::source::{SourceAdapter, SourceContext};
use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use futures_util::future::BoxFuture;
use futures_util::stream::{self, StreamExt, TryStreamExt};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

const SOURCE: &str = "kr";
const PRODUCT: &str = "composite";
const HOST: &str = "radar.kma.go.kr";
const DISCOVERY_URL: &str = "https://radar.kma.go.kr/radar/fileChkAjax.do";
const IMAGE_URL: &str = "https://radar.kma.go.kr/cgi-bin/center/nph-rdr_stn1_img";
const REFERER: &str = "https://radar.kma.go.kr/eng/radar/individual.do";
const USER_AGENT: &str = "Mozilla/5.0 (compatible; radiust/1)";
const LOCATOR_VERSION: &str = "kr-legacy-v1";
const STATIONS: [&str; 10] = ["KWK", "BRI", "GDK", "GNG", "KSN", "JNI", "MYN", "PSN", "GSN", "SSP"];
const REQUEST_OFFSETS_MINUTES: [i64; 5] = [3, 8, 13, 18, 23];

/// Discovers the latest station frames advertised by KMA's station endpoint.
pub struct KrSourceAdapter;

impl SourceAdapter for KrSourceAdapter {
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
            validate_target_and_query(&target, &context.query)?;
            let stations = requested_stations(&target, &context.query)?;
            if stations.is_empty() {
                return Ok(Vec::new());
            }
            if !context.allow_network {
                return Err(CoreError::NetworkDisabled(
                    "source kr discovery requires network access".into(),
                ));
            }
            let entries = discover_entries(&context, &stations, DISCOVERY_URL, Utc::now()).await?;
            entries
                .into_iter()
                .map(frame_from_entry)
                .collect::<CoreResult<Vec<_>>>()
                .map(sort_frames)
        })
    }
}

#[derive(Clone, Debug)]
struct Entry {
    station: String,
    rec_date: String,
    valid_time: DateTime<Utc>,
}

fn validate_target_and_query(target: &DiscoveryTarget, query: &Query) -> CoreResult<()> {
    if target.source != SOURCE
        || query.source.as_deref().is_some_and(|source| source != SOURCE && source != "all")
        || (!query.sources.is_empty() && !query.sources.iter().any(|source| source == SOURCE))
    {
        return Err(CoreError::Transport("source kr received a mismatched source query".into()));
    }
    if target.product.as_deref().is_some_and(|product| product != PRODUCT)
        || query.product.as_deref().is_some_and(|product| product != PRODUCT)
    {
        return Err(CoreError::Transport("source kr only supports the composite product".into()));
    }
    if target.station.as_deref().is_some_and(|station| !STATIONS.contains(&station))
        || query.stations.iter().any(|station| !STATIONS.contains(&station.as_str()))
    {
        return Err(CoreError::Transport("source kr requires a supported station".into()));
    }
    if query.base_time.is_some() {
        return Err(CoreError::Transport("source kr does not expose base times".into()));
    }
    Ok(())
}

fn requested_stations(target: &DiscoveryTarget, query: &Query) -> CoreResult<Vec<String>> {
    let selected = if let Some(station) = target.station.as_deref() {
        vec![station]
    } else if query.stations.is_empty() {
        STATIONS.to_vec()
    } else {
        query.stations.iter().map(String::as_str).collect()
    };
    Ok(selected
        .into_iter()
        .filter(|station| query.stations.is_empty() || query.stations.iter().any(|v| v == station))
        .map(str::to_owned)
        .collect())
}

async fn discover_entries(
    context: &SourceContext,
    stations: &[String],
    discovery_url: &str,
    now: DateTime<Utc>,
) -> CoreResult<Vec<Entry>> {
    let tasks = REQUEST_OFFSETS_MINUTES
        .into_iter()
        .flat_map(|offset| {
            let request_time = candidate_time(now, offset);
            stations.iter().cloned().map(move |station| (station, request_time.clone()))
        })
        .collect::<Vec<_>>();
    let workers = context.discovery_workers.max(1);
    let request_context = context.clone();
    let discovery_url = discovery_url.to_owned();
    let entries = stream::iter(tasks)
        .map(move |(station, request_time)| {
            let context = request_context.clone();
            let address = discovery_url.clone();
            async move {
                let form = [
                    ("tm", request_time.as_str()),
                    ("cgiId", "STN"),
                    ("siteCd", station.as_str()),
                    ("prId", "gif"),
                ];
                let headers = [
                    ("User-Agent", USER_AGENT),
                    ("Accept", "application/json, text/javascript, */*; q=0.01"),
                    ("X-Requested-With", "XMLHttpRequest"),
                    ("Referer", REFERER),
                ];
                let payload = context
                    .http_transport
                    .post_form_bytes_with_headers(&address, &form, &headers)
                    .await
                    .map_err(sanitize_discovery_error)?;
                parse_response(&station, &payload)
            }
        })
        .buffer_unordered(workers)
        .try_collect::<Vec<_>>()
        .await?;

    let entries = entries.into_iter().flatten().collect::<Vec<_>>();
    let entries = deduplicate_entries(entries);
    select_entries(entries, &context.query.selector)
}

fn candidate_time(now: DateTime<Utc>, offset_minutes: i64) -> String {
    // Asia/Seoul is UTC+09:00 and does not observe daylight saving time.
    let seoul = FixedOffset::east_opt(9 * 60 * 60).expect("valid Seoul offset");
    now.with_timezone(&seoul)
        .checked_sub_signed(Duration::minutes(offset_minutes))
        .expect("candidate offsets are within the representable timestamp range")
        .format("%Y%m%d%H%M")
        .to_string()
}

fn parse_response(station: &str, payload: &[u8]) -> CoreResult<Option<Entry>> {
    if payload.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let document: Value = serde_json::from_slice(payload)
        .map_err(|_| CoreError::Transport("KMA returned invalid station discovery JSON".into()))?;
    let Some(item) = document.as_array().and_then(|items| items.first()).and_then(Value::as_object)
    else {
        return Ok(None);
    };
    if item.get("result").and_then(Value::as_i64) != Some(1) {
        return Ok(None);
    }
    let Some(rec_date) = item.get("recDate").and_then(Value::as_str) else {
        return Ok(None);
    };
    let Some(valid_time) = parse_rec_date(rec_date) else {
        return Ok(None);
    };
    Ok(Some(Entry { station: station.to_owned(), rec_date: rec_date.to_owned(), valid_time }))
}

fn parse_rec_date(value: &str) -> Option<DateTime<Utc>> {
    if value.len() != 12 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let local = NaiveDateTime::parse_from_str(value, "%Y%m%d%H%M").ok()?;
    let seoul = FixedOffset::east_opt(9 * 60 * 60)?;
    seoul.from_local_datetime(&local).single().map(|time| time.with_timezone(&Utc))
}

fn deduplicate_entries(entries: Vec<Entry>) -> Vec<Entry> {
    let unique = entries
        .into_iter()
        .map(|entry| ((entry.station.clone(), entry.rec_date.clone()), entry))
        .collect::<BTreeMap<_, _>>();
    unique.into_values().collect()
}

fn select_entries(entries: Vec<Entry>, selector: &TimeSelector) -> CoreResult<Vec<Entry>> {
    let mut selected = match selector {
        TimeSelector::Latest => {
            let mut latest = BTreeMap::<String, Entry>::new();
            for entry in entries {
                let replace = latest.get(&entry.station).is_none_or(|current| {
                    (entry.valid_time, entry.rec_date.as_str())
                        > (current.valid_time, current.rec_date.as_str())
                });
                if replace {
                    latest.insert(entry.station.clone(), entry);
                }
            }
            latest.into_values().collect()
        }
        TimeSelector::At { time } => {
            let requested = parse_query_time(time)?;
            entries.into_iter().filter(|entry| entry.valid_time == requested).collect()
        }
        TimeSelector::Range { start, end } => {
            let start = parse_query_time(start)?;
            let end = parse_query_time(end)?;
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
            .then_with(|| left.rec_date.cmp(&right.rec_date))
    });
    Ok(selected)
}

fn parse_query_time(value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| CoreError::Transport("source kr query contains an invalid timestamp".into()))
}

fn frame_from_entry(entry: Entry) -> CoreResult<FrameRef> {
    let url = image_url(&entry.station, &entry.rec_date).ok_or_else(|| {
        CoreError::Transport("source kr produced an invalid image locator".into())
    })?;
    let revision = format!("{}-{}", entry.station, entry.rec_date);
    let mut frame = FrameRef {
        source: SOURCE.into(),
        product: PRODUCT.into(),
        station: Some(entry.station.clone()),
        valid_time: entry.valid_time.to_rfc3339_opts(SecondsFormat::Micros, true),
        base_time: None,
        logical_id: String::new(),
        revision: Some(revision.clone()),
        locator_version: LOCATOR_VERSION.into(),
        locator: json!({
            "url": url,
            "artifacts": [],
            "station": entry.station,
            "revision": revision,
            "headers": {
                "User-Agent": USER_AGENT,
                "Referer": REFERER,
            },
        }),
    };
    frame.logical_id = logical_id(&frame).map_err(|_| {
        CoreError::Transport("source kr frame identity could not be computed".into())
    })?;
    Ok(frame)
}

fn image_url(station: &str, rec_date: &str) -> Option<String> {
    if !STATIONS.contains(&station) || parse_rec_date(rec_date).is_none() {
        return None;
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query
        .append_pair("rdr", "HSR")
        .append_pair("vol", "RN")
        .append_pair("cpi", "CPP")
        .append_pair("cdf", "1")
        .append_pair("sms", "5")
        .append_pair("swpn", "0")
        .append_pair("ht", "1.5i")
        .append_pair("aws", "0")
        .append_pair("map", "")
        .append_pair("color", "")
        .append_pair("area", "1")
        .append_pair("ang", "")
        .append_pair("size", "720")
        .append_pair("stn", station)
        .append_pair("tm", rec_date)
        .append_pair("zoom_level", "0")
        .append_pair("zoom_x", "0000000")
        .append_pair("zoom_y", "0000000")
        .append_pair("xp", "undefined")
        .append_pair("yp", "undefined")
        .append_pair("zoom", "1");
    Some(format!("{IMAGE_URL}?{}", query.finish()))
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

fn sanitize_discovery_error(error: CoreError) -> CoreError {
    match error {
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::NetworkDisabled(_) => {
            CoreError::NetworkDisabled("source kr discovery requires public network opt-in".into())
        }
        _ => CoreError::Transport("source kr station discovery request failed".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::frame_identity;
    use crate::limits::{Limits, RequestBudget};
    use crate::model::TimeSelector;
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Mutex;
    use tokio::task::JoinSet;

    const KR_FIXTURE: &str = include_str!("../../../../tests/fixtures/sources/kr/fixture.json");

    fn context(query: Query, workers: usize, allow_network: bool) -> SourceContext {
        let mut limits = Limits::default();
        limits.request_concurrency = workers.max(1);
        let request_budget = Arc::new(RequestBudget::new(&limits));
        let http_transport = Arc::new(
            HttpTransport::with_budget(limits.clone(), allow_network, request_budget.clone())
                .expect("HTTP transport"),
        );
        SourceContext {
            query,
            allow_network,
            discovery_workers: workers,
            source_options: Arc::new(Default::default()),
            request_budget,
            ftp_transport: Arc::new(FtpTransport::new(limits.clone(), allow_network)),
            limits,
            http_transport,
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        }
    }

    fn target() -> DiscoveryTarget {
        DiscoveryTarget { source: SOURCE.into(), product: Some(PRODUCT.into()), station: None }
    }

    fn query() -> Query {
        Query { source: Some(SOURCE.into()), ..Query::default() }
    }

    #[test]
    fn preserves_kma_station_inventory_and_candidate_times_in_seoul() {
        assert_eq!(
            STATIONS,
            ["KWK", "BRI", "GDK", "GNG", "KSN", "JNI", "MYN", "PSN", "GSN", "SSP"]
        );
        let now = DateTime::parse_from_rfc3339("2026-09-18T03:53:00Z").unwrap().to_utc();
        assert_eq!(
            REQUEST_OFFSETS_MINUTES.map(|offset| candidate_time(now, offset)),
            ["202609181250", "202609181245", "202609181240", "202609181235", "202609181230"]
        );
        assert_eq!(
            parse_rec_date("202609181250").unwrap().to_rfc3339(),
            "2026-09-18T03:50:00+00:00"
        );
        assert!(parse_rec_date("202602301250").is_none());
        assert!(parse_rec_date("20260918125").is_none());
    }

    #[test]
    fn builds_fixture_image_url_revision_and_python_frame_identity() {
        let frame = frame_from_entry(Entry {
            station: "KWK".into(),
            rec_date: "202609181250".into(),
            valid_time: parse_rec_date("202609181250").unwrap(),
        })
        .unwrap();
        let expected_url = concat!(
            "https://radar.kma.go.kr/cgi-bin/center/nph-rdr_stn1_img?",
            "rdr=HSR&vol=RN&cpi=CPP&cdf=1&sms=5&swpn=0&ht=1.5i&aws=0&map=&color=",
            "&area=1&ang=&size=720&stn=KWK&tm=202609181250&zoom_level=0&zoom_x=0000000",
            "&zoom_y=0000000&xp=undefined&yp=undefined&zoom=1"
        );
        assert_eq!(frame.product, PRODUCT);
        assert_eq!(frame.station.as_deref(), Some("KWK"));
        assert_eq!(frame.valid_time, "2026-09-18T03:50:00.000000Z");
        assert_eq!(frame.revision.as_deref(), Some("KWK-202609181250"));
        assert_eq!(frame.locator_version, LOCATOR_VERSION);
        assert_eq!(frame.locator["url"], expected_url);
        assert_eq!(frame.locator["headers"]["Referer"], REFERER);
        assert_eq!(frame_identity(&frame).unwrap()["locator"]["artifacts"], json!([]));
        assert_eq!(
            frame.logical_id,
            "6579cf95fa3551b4cf4066e98df025de4b8e41c20b727673d2fd1db1da7605ee"
        );

        let fixture: Value = serde_json::from_str(KR_FIXTURE).unwrap();
        let row = &fixture["frames"][0];
        assert_eq!(row["product"], frame.product);
        assert_eq!(row["station"], frame.station.as_deref().unwrap());
        assert_eq!(row["revision"], frame.revision.as_deref().unwrap());
        assert_eq!(row["locator_version"], frame.locator_version);
        assert_eq!(row["valid_time"], "2026-09-18T03:50:00Z");
    }

    #[test]
    fn validates_source_product_station_and_base_time_filters() {
        assert!(validate_target_and_query(&target(), &query()).is_ok());
        for invalid in [
            DiscoveryTarget { source: "my".into(), ..target() },
            DiscoveryTarget { product: Some("rain".into()), ..target() },
            DiscoveryTarget { station: Some("UNKNOWN".into()), ..target() },
        ] {
            assert!(validate_target_and_query(&invalid, &query()).is_err());
        }
        assert!(
            validate_target_and_query(
                &target(),
                &Query { product: Some("rain".into()), ..query() }
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target(),
                &Query { stations: vec!["UNKNOWN".into()], ..query() }
            )
            .is_err()
        );
        assert!(
            validate_target_and_query(
                &target(),
                &Query { base_time: Some("2026-09-18T03:50:00Z".into()), ..query() }
            )
            .is_err()
        );
    }

    #[test]
    fn applies_exact_and_half_open_range_selectors_per_station() {
        let entries = ["202609181250", "202609181245", "202609181240"]
            .into_iter()
            .map(|rec_date| Entry {
                station: "KWK".into(),
                rec_date: rec_date.into(),
                valid_time: parse_rec_date(rec_date).unwrap(),
            })
            .collect::<Vec<_>>();
        let exact = select_entries(
            entries.clone(),
            &TimeSelector::At { time: "2026-09-18T12:50:00+09:00".into() },
        )
        .unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].rec_date, "202609181250");
        let range = select_entries(
            entries,
            &TimeSelector::Range {
                start: "2026-09-18T03:45:00Z".into(),
                end: "2026-09-18T03:50:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(range.len(), 1);
        assert_eq!(range[0].rec_date, "202609181245");
    }

    #[tokio::test]
    async fn posts_station_metadata_with_bounded_workers_and_network_opt_in() {
        let (url, server) = mock_kma_server(STATIONS.len() * REQUEST_OFFSETS_MINUTES.len()).await;
        let fixed_now = DateTime::parse_from_rfc3339("2026-09-18T03:53:00Z").unwrap().to_utc();
        let discovery_context = context(query(), 3, true);
        let frames =
            discover_entries(&discovery_context, &STATIONS.map(str::to_owned), &url, fixed_now)
                .await
                .unwrap()
                .into_iter()
                .map(frame_from_entry)
                .collect::<CoreResult<Vec<_>>>()
                .unwrap();
        let (requests, max_active) = server.await.unwrap();
        assert_eq!(requests.len(), 50);
        assert!(max_active <= 3, "observed {max_active} concurrent requests");
        assert_eq!(frames.len(), STATIONS.len());
        assert!(frames.iter().all(|frame| frame.valid_time == "2026-09-18T03:50:00.000000Z"));

        let expected_times =
            ["202609181250", "202609181245", "202609181240", "202609181235", "202609181230"];
        for request in requests {
            assert!(request.headers.starts_with("POST /radar/fileChkAjax.do HTTP/1.1"));
            let headers = request.headers.to_ascii_lowercase();
            assert!(headers.contains("content-type: application/x-www-form-urlencoded"));
            assert!(headers.contains("x-requested-with: xmlhttprequest"));
            assert!(headers.contains("referer: https://radar.kma.go.kr/eng/radar/individual.do"));
            assert!(headers.contains("user-agent: mozilla/5.0 (compatible; radiust/1)"));
            let form = url::form_urlencoded::parse(request.body.as_bytes())
                .into_owned()
                .collect::<BTreeMap<_, _>>();
            assert_eq!(form["cgiId"], "STN");
            assert_eq!(form["prId"], "gif");
            assert!(STATIONS.contains(&form["siteCd"].as_str()));
            assert!(expected_times.contains(&form["tm"].as_str()));
        }

        let restricted = context(query(), 2, false);
        assert!(matches!(
            discover_entries(
                &restricted,
                &["KWK".into()],
                "https://radar.kma.go.kr/radar/fileChkAjax.do",
                fixed_now,
            )
            .await,
            Err(CoreError::NetworkDisabled(_))
        ));
    }

    #[tokio::test]
    async fn station_and_time_filters_bound_local_requests_and_results() {
        let (url, server) = mock_kma_server(REQUEST_OFFSETS_MINUTES.len()).await;
        let selected_query = Query {
            stations: vec!["KWK".into()],
            selector: TimeSelector::Range {
                start: "2026-09-18T03:45:00Z".into(),
                end: "2026-09-18T03:55:00Z".into(),
            },
            ..query()
        };
        let discovery_context = context(selected_query, 2, true);
        let fixed_now = DateTime::parse_from_rfc3339("2026-09-18T03:53:00Z").unwrap().to_utc();
        let stations = requested_stations(&target(), &discovery_context.query).unwrap();
        let entries =
            discover_entries(&discovery_context, &stations, &url, fixed_now).await.unwrap();
        let frames =
            entries.into_iter().map(frame_from_entry).collect::<CoreResult<Vec<_>>>().unwrap();
        let (requests, max_active) = server.await.unwrap();
        assert_eq!(requests.len(), 5);
        assert!(max_active <= 2);
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|frame| frame.station.as_deref() == Some("KWK")));
        assert_eq!(frames[0].valid_time, "2026-09-18T03:45:00.000000Z");
        assert_eq!(frames[1].valid_time, "2026-09-18T03:50:00.000000Z");
    }

    #[derive(Debug)]
    struct CapturedRequest {
        headers: String,
        body: String,
    }

    async fn mock_kma_server(
        expected_requests: usize,
    ) -> (String, tokio::task::JoinHandle<(Vec<CapturedRequest>, usize)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let requests = Arc::new(Mutex::new(Vec::with_capacity(expected_requests)));
            let active = Arc::new(AtomicUsize::new(0));
            let max_active = Arc::new(AtomicUsize::new(0));
            let mut handlers = JoinSet::new();
            for _ in 0..expected_requests {
                let (stream, _) = listener.accept().await.unwrap();
                let requests = requests.clone();
                let active = active.clone();
                let max_active = max_active.clone();
                handlers.spawn(async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(current, Ordering::SeqCst);
                    let (mut stream, request) = read_request(stream).await;
                    let body = request.body.clone();
                    requests.lock().await.push(request);
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    let time = url::form_urlencoded::parse(body.as_bytes())
                        .find(|(key, _)| key == "tm")
                        .map(|(_, value)| value.into_owned())
                        .unwrap();
                    let response = format!("[{{\"result\":1,\"recDate\":\"{time}\"}}]");
                    let response_head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        response.len()
                    );
                    stream.write_all(response_head.as_bytes()).await.unwrap();
                    stream.write_all(response.as_bytes()).await.unwrap();
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
            while handlers.join_next().await.is_some() {}
            let captured = Arc::try_unwrap(requests).unwrap().into_inner();
            (captured, max_active.load(Ordering::SeqCst))
        });
        (format!("http://{address}/radar/fileChkAjax.do"), server)
    }

    async fn read_request(mut stream: TcpStream) -> (TcpStream, CapturedRequest) {
        let mut reader = BufReader::new(&mut stream);
        let mut headers = String::new();
        let mut content_length = 0_usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.trim().parse().unwrap();
                }
            }
            headers.push_str(&line);
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).await.unwrap();
        drop(reader);
        (stream, CapturedRequest { headers, body: String::from_utf8(body).unwrap() })
    }
}
