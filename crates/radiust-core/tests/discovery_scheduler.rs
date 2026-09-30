use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::engine::Engine;
use radiust_core::errors::{CoreError, CoreResult};
use radiust_core::identity::logical_id;
use radiust_core::model::{DiscoveryStatus, DiscoveryTarget, FrameRef, Query};
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};

#[derive(Clone, Copy)]
enum ResultKind {
    Frame,
    NoData,
    Failure,
}

enum FixtureBehavior {
    Immediate(ResultKind),
    Delayed(Duration, ResultKind),
    Gated(Arc<Semaphore>, ResultKind),
    Pending,
    HeadMetadata(String),
    CoalescedGet(String),
}

#[derive(Clone, Debug)]
enum Event {
    Started,
    Completed(String),
}

struct Tracker {
    active: AtomicUsize,
    maximum: AtomicUsize,
    start_order: parking_lot::Mutex<Vec<String>>,
    events: mpsc::UnboundedSender<Event>,
}

struct ActiveGuard(Arc<Tracker>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn tracker() -> (Arc<Tracker>, mpsc::UnboundedReceiver<Event>) {
    let (events, receiver) = mpsc::unbounded_channel();
    (
        Arc::new(Tracker {
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            start_order: parking_lot::Mutex::new(Vec::new()),
            events,
        }),
        receiver,
    )
}

impl Tracker {
    fn begin(self: &Arc<Self>, source: &str) -> ActiveGuard {
        let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(current, Ordering::SeqCst);
        self.start_order.lock().push(source.to_owned());
        let _ = self.events.send(Event::Started);
        ActiveGuard(self.clone())
    }

    fn complete(&self, source: &str) {
        let _ = self.events.send(Event::Completed(source.to_owned()));
    }
}

struct FixtureAdapter {
    id: &'static str,
    behavior: FixtureBehavior,
    tracker: Arc<Tracker>,
}

impl FixtureAdapter {
    fn new(id: &'static str, behavior: FixtureBehavior, tracker: Arc<Tracker>) -> Self {
        Self { id, behavior, tracker }
    }
}

impl SourceAdapter for FixtureAdapter {
    fn source_id(&self) -> &'static str {
        self.id
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            let _active = self.tracker.begin(&target.source);
            let result = match &self.behavior {
                FixtureBehavior::Immediate(kind) => fixture_result(target.clone(), *kind),
                FixtureBehavior::Delayed(delay, kind) => {
                    tokio::time::sleep(*delay).await;
                    fixture_result(target.clone(), *kind)
                }
                FixtureBehavior::Gated(gate, kind) => {
                    let _permit = gate.clone().acquire_owned().await.map_err(|_| {
                        CoreError::Transport("fixture gate closed unexpectedly".into())
                    })?;
                    fixture_result(target.clone(), *kind)
                }
                FixtureBehavior::Pending => std::future::pending().await,
                FixtureBehavior::HeadMetadata(address) => {
                    context.http_transport.head_metadata(address).await?;
                    Ok(Vec::new())
                }
                FixtureBehavior::CoalescedGet(address) => {
                    let bytes = context
                        .http_transport
                        .get_bytes_coalesced(
                            address,
                            &[("accept", "application/json")],
                            &context.request_coalescer,
                        )
                        .await?;
                    if bytes.as_ref() == b"metadata" {
                        Ok(Vec::new())
                    } else {
                        Err(CoreError::Transport("unexpected fixture metadata".into()))
                    }
                }
            };
            self.tracker.complete(&target.source);
            result
        })
    }
}

fn fixture_result(target: DiscoveryTarget, kind: ResultKind) -> CoreResult<Vec<FrameRef>> {
    match kind {
        ResultKind::NoData => Ok(Vec::new()),
        ResultKind::Failure => Err(CoreError::Transport("fixture upstream failure".into())),
        ResultKind::Frame => {
            let mut frame = FrameRef {
                source: target.source,
                product: target.product.unwrap_or_else(|| "composite".into()),
                station: target.station,
                valid_time: "2026-09-24T00:00:00Z".into(),
                base_time: None,
                logical_id: String::new(),
                revision: None,
                locator_version: "fixture-v1".into(),
                locator: json!({"fixture": true}),
            };
            frame.logical_id = logical_id(&frame).expect("fixture frame has a valid identity");
            Ok(vec![frame])
        }
    }
}

fn fixture_engine(
    adapters: Vec<FixtureAdapter>,
    workers: usize,
    deadline: Duration,
    request_limit: usize,
    host_limit: usize,
) -> Engine {
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.discovery_workers = workers;
    config.runtime.discovery_deadline = deadline.as_secs_f64();
    config.runtime.request_concurrency = request_limit;
    config.runtime.host_concurrency = host_limit;

    let mut registry = SourceRegistry::default();
    for adapter in adapters {
        registry.register(Arc::new(adapter)).expect("fixture source IDs are unique");
    }
    Engine::new(config, registry).expect("fixture Engine configuration is valid")
}

fn multi_query(sources: &[&str]) -> Query {
    Query {
        sources: sources.iter().map(|source| (*source).to_owned()).collect(),
        ..Query::default()
    }
}

async fn receive_until(
    receiver: &mut mpsc::UnboundedReceiver<Event>,
    observed: &mut Vec<Event>,
    predicate: impl Fn(&[Event]) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !predicate(observed) {
            observed.push(receiver.recv().await.expect("fixture event channel remains open"));
        }
    })
    .await
    .expect("expected fixture events before the test timeout");
}

fn completed_sources(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Completed(source) => Some(source.clone()),
            Event::Started => None,
        })
        .collect()
}

#[tokio::test]
async fn four_delayed_sources_respect_the_worker_bound_and_keep_stable_terminal_reports() {
    let (tracker, _events) = tracker();
    let engine = fixture_engine(
        vec![
            FixtureAdapter::new(
                "ca",
                FixtureBehavior::Delayed(Duration::from_millis(35), ResultKind::Frame),
                tracker.clone(),
            ),
            FixtureAdapter::new(
                "es",
                FixtureBehavior::Delayed(Duration::from_millis(5), ResultKind::NoData),
                tracker.clone(),
            ),
            FixtureAdapter::new(
                "fr",
                FixtureBehavior::Delayed(Duration::from_millis(25), ResultKind::Failure),
                tracker.clone(),
            ),
            FixtureAdapter::new(
                "nz",
                FixtureBehavior::Delayed(Duration::from_millis(12), ResultKind::Frame),
                tracker.clone(),
            ),
        ],
        2,
        Duration::from_secs(3),
        4,
        4,
    );

    let first = engine.discover_seeded(multi_query(&["ca", "es", "fr", "nz"]), 17).await.unwrap();
    let second = engine.discover_seeded(multi_query(&["ca", "es", "fr", "nz"]), 91).await.unwrap();

    assert_eq!(tracker.maximum.load(Ordering::SeqCst), 2);
    assert_eq!(
        first.items, second.items,
        "seeded scheduling must not affect report order or contents"
    );
    assert_eq!(
        first.items.iter().map(|item| item.target.source.as_str()).collect::<Vec<_>>(),
        vec!["ca", "es", "fr", "nz"]
    );
    assert_eq!(first.counts.total, 4);
    assert_eq!(first.counts.success, 2);
    assert_eq!(first.counts.no_data, 1);
    assert_eq!(first.counts.upstream_failed, 1);
    assert_eq!(first.counts.timeout, 0);
    assert_eq!(first.counts.cancelled, 0);
    assert_eq!(first.counts.not_started, 0);
    assert_eq!(
        first.counts.success
            + first.counts.no_data
            + first.counts.stale
            + first.counts.missing_credentials
            + first.counts.retired
            + first.counts.network_restricted
            + first.counts.upstream_failed
            + first.counts.ambiguous
            + first.counts.timeout
            + first.counts.cancelled
            + first.counts.not_started,
        first.counts.total
    );
    assert!(first.items.iter().all(|item| item.status.is_terminal()));
    assert!(!first.interrupted);
}

#[tokio::test]
async fn slow_and_failed_targets_do_not_prevent_other_targets_from_completing() {
    let (tracker, mut receiver) = tracker();
    let slow_gate = Arc::new(Semaphore::new(0));
    let engine = Arc::new(fixture_engine(
        vec![
            FixtureAdapter::new(
                "ca",
                FixtureBehavior::Gated(slow_gate.clone(), ResultKind::Frame),
                tracker.clone(),
            ),
            FixtureAdapter::new(
                "es",
                FixtureBehavior::Immediate(ResultKind::Failure),
                tracker.clone(),
            ),
            FixtureAdapter::new(
                "fr",
                FixtureBehavior::Immediate(ResultKind::Frame),
                tracker.clone(),
            ),
        ],
        3,
        Duration::from_secs(3),
        4,
        4,
    ));
    let task = tokio::spawn({
        let engine = engine.clone();
        async move { engine.discover_seeded(multi_query(&["ca", "es", "fr"]), 4).await.unwrap() }
    });

    let mut observed = Vec::new();
    receive_until(&mut receiver, &mut observed, |events| {
        events.iter().filter(|event| matches!(event, Event::Started)).count() == 3
    })
    .await;
    receive_until(&mut receiver, &mut observed, |events| {
        events.iter().filter(|event| matches!(event, Event::Completed(_))).count() == 2
    })
    .await;
    let completed = completed_sources(&observed);
    assert!(completed.contains(&"es".to_owned()));
    assert!(completed.contains(&"fr".to_owned()));
    assert!(!completed.contains(&"ca".to_owned()), "the gated target must still be active");

    slow_gate.add_permits(1);
    let report = task.await.unwrap();
    assert_eq!(report.counts.success, 2);
    assert_eq!(report.counts.upstream_failed, 1);
    assert_eq!(report.counts.total, 3);
}

#[tokio::test]
async fn batch_deadline_marks_active_targets_timeout_and_queued_targets_not_started() {
    let (tracker, mut receiver) = tracker();
    let engine = Arc::new(fixture_engine(
        ["ca", "es", "fr"]
            .into_iter()
            .map(|source| FixtureAdapter::new(source, FixtureBehavior::Pending, tracker.clone()))
            .collect(),
        2,
        Duration::from_millis(400),
        4,
        4,
    ));
    let task = tokio::spawn({
        let engine = engine.clone();
        async move { engine.discover_seeded(multi_query(&["ca", "es", "fr"]), 55).await.unwrap() }
    });
    let mut observed = Vec::new();
    receive_until(&mut receiver, &mut observed, |events| {
        events.iter().filter(|event| matches!(event, Event::Started)).count() == 2
    })
    .await;

    let report = task.await.unwrap();
    let statuses = report.items.iter().map(|item| item.status).collect::<Vec<_>>();
    assert_eq!(statuses.iter().filter(|status| **status == DiscoveryStatus::Timeout).count(), 2);
    assert_eq!(statuses.iter().filter(|status| **status == DiscoveryStatus::NotStarted).count(), 1);
    assert_eq!(report.counts.timeout, 2);
    assert_eq!(report.counts.not_started, 1);
    assert_eq!(report.counts.total, 3);
    assert_eq!(tracker.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_marks_active_targets_cancelled_and_queued_targets_not_started() {
    let (tracker, mut receiver) = tracker();
    let engine = Arc::new(fixture_engine(
        ["ca", "es", "fr"]
            .into_iter()
            .map(|source| FixtureAdapter::new(source, FixtureBehavior::Pending, tracker.clone()))
            .collect(),
        2,
        Duration::from_secs(4),
        4,
        4,
    ));
    let task = tokio::spawn({
        let engine = engine.clone();
        async move { engine.discover_seeded(multi_query(&["ca", "es", "fr"]), 73).await.unwrap() }
    });
    let mut observed = Vec::new();
    receive_until(&mut receiver, &mut observed, |events| {
        events.iter().filter(|event| matches!(event, Event::Started)).count() == 2
    })
    .await;
    engine.cancel();

    let report = task.await.unwrap();
    assert_eq!(report.counts.cancelled, 2);
    assert_eq!(report.counts.not_started, 1);
    assert_eq!(report.counts.total, 3);
    assert!(report.interrupted);
    assert!(report.items.iter().all(|item| item.status.is_terminal()));
    assert_eq!(tracker.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn source_groups_are_rotated_before_a_source_gets_its_second_target() {
    let (tracker, _events) = tracker();
    let engine = fixture_engine(
        vec![
            FixtureAdapter::new(
                "ca",
                FixtureBehavior::Immediate(ResultKind::NoData),
                tracker.clone(),
            ),
            FixtureAdapter::new(
                "fr",
                FixtureBehavior::Immediate(ResultKind::NoData),
                tracker.clone(),
            ),
            FixtureAdapter::new(
                "my",
                FixtureBehavior::Immediate(ResultKind::NoData),
                tracker.clone(),
            ),
        ],
        1,
        Duration::from_secs(3),
        4,
        4,
    );

    let report = engine.discover_seeded(multi_query(&["my", "ca", "fr"]), 103).await.unwrap();
    let started = tracker.start_order.lock().clone();
    assert_eq!(report.counts.total, 4, "my expands to two station targets");
    assert_eq!(report.counts.no_data, 4);
    assert_eq!(started.len(), 4);
    let first_cycle = started[..3].iter().cloned().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        first_cycle,
        ["ca".to_owned(), "fr".to_owned(), "my".to_owned()].into_iter().collect()
    );
}

async fn read_request_headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 512];
    loop {
        let count = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut buffer))
            .await
            .expect("local fixture request arrives")
            .expect("local TCP read succeeds");
        assert_ne!(count, 0, "request ended before its headers were complete");
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).expect("HTTP request is UTF-8");
        }
    }
}

async fn serve_head_requests(
    listener: TcpListener,
    expected: usize,
    response_delay: Duration,
) -> (usize, usize) {
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..expected {
        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(4), listener.accept())
            .await
            .expect("local HTTP request arrives")
            .expect("local TCP accept succeeds");
        let active = active.clone();
        let maximum = maximum.clone();
        tasks.spawn(async move {
            let _request = read_request_headers(&mut stream).await;
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(response_delay).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("write local HEAD response");
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.expect("local HTTP fixture task succeeds");
    }
    (maximum.load(Ordering::SeqCst), expected)
}

async fn same_host_request_peak(request_limit: usize, host_limit: usize) -> (usize, usize) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_head_requests(listener, 4, Duration::from_millis(100)));
    let (tracker, _events) = tracker();
    let adapters = ["ca", "es", "fr", "nz"]
        .into_iter()
        .map(|source| {
            FixtureAdapter::new(
                source,
                FixtureBehavior::HeadMetadata(format!("http://{address}/{source}")),
                tracker.clone(),
            )
        })
        .collect();
    let engine = fixture_engine(adapters, 4, Duration::from_secs(4), request_limit, host_limit);
    let report = engine.discover_seeded(multi_query(&["ca", "es", "fr", "nz"]), 9).await.unwrap();
    let (maximum, requests) = server.await.unwrap();
    assert_eq!(report.counts.no_data, 4);
    assert_eq!(report.counts.upstream_failed, 0);
    (maximum, requests)
}

#[tokio::test]
async fn same_host_discovery_requests_share_global_and_per_host_limits() {
    let (global_peak, global_requests) = same_host_request_peak(2, 4).await;
    assert_eq!(global_peak, 2, "the Engine request budget is shared across source adapters");
    assert_eq!(global_requests, 4);

    let (host_peak, host_requests) = same_host_request_peak(4, 2).await;
    assert_eq!(host_peak, 2, "the host budget is shared across source adapters");
    assert_eq!(host_requests, 4);
}

async fn serve_coalesced_metadata(listener: TcpListener, observation_window: Duration) -> usize {
    let deadline = tokio::time::Instant::now() + observation_window;
    let mut tasks = tokio::task::JoinSet::new();
    let mut accepted = 0;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let accepted_stream = tokio::time::timeout(remaining, listener.accept()).await;
        let Ok(Ok((mut stream, _))) = accepted_stream else {
            break;
        };
        accepted += 1;
        tasks.spawn(async move {
            let request = read_request_headers(&mut stream).await;
            assert!(request.starts_with("GET /metadata HTTP/1.1\r\n"));
            tokio::time::sleep(Duration::from_millis(60)).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nmetadata",
                )
                .await
                .expect("write local metadata response");
        });
    }
    while let Some(result) = tasks.join_next().await {
        result.expect("local metadata fixture task succeeds");
    }
    accepted
}

#[tokio::test]
async fn duplicate_upstream_metadata_is_coalesced_across_source_adapters() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_coalesced_metadata(listener, Duration::from_millis(250)));
    let (tracker, _events) = tracker();
    let url = format!("http://{address}/metadata");
    let adapters = ["ca", "es", "fr", "nz"]
        .into_iter()
        .map(|source| {
            FixtureAdapter::new(source, FixtureBehavior::CoalescedGet(url.clone()), tracker.clone())
        })
        .collect();
    let engine = fixture_engine(adapters, 4, Duration::from_secs(3), 4, 4);

    let report = engine.discover_seeded(multi_query(&["ca", "es", "fr", "nz"]), 22).await.unwrap();
    let accepted = server.await.unwrap();
    assert_eq!(report.counts.no_data, 4);
    assert_eq!(report.counts.upstream_failed, 0);
    assert_eq!(accepted, 1, "identical metadata GETs should share one upstream request");
}

fn percentile_ms(values: &[f64], percentile: f64) -> f64 {
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let position = (ordered.len() - 1) as f64 * percentile;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower as f64)
}

/// Manual scheduler acceptance run; it reports data but does not impose a
/// machine-sensitive timing threshold on the regular integration suite.
#[tokio::test]
#[ignore = "manual performance acceptance; run explicitly with --ignored --nocapture"]
async fn four_delayed_sources_emit_paired_scheduler_measurements() {
    let delays = [
        ("ca", Duration::from_millis(35), ResultKind::Frame),
        ("es", Duration::from_millis(5), ResultKind::NoData),
        ("fr", Duration::from_millis(25), ResultKind::Failure),
        ("nz", Duration::from_millis(12), ResultKind::Frame),
    ];
    let (tracker, _events) = tracker();
    let engine = fixture_engine(
        delays
            .iter()
            .map(|(id, delay, result)| {
                FixtureAdapter::new(id, FixtureBehavior::Delayed(*delay, *result), tracker.clone())
            })
            .collect(),
        4,
        Duration::from_secs(3),
        4,
        4,
    );
    let source_ids = delays.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
    let query = multi_query(&source_ids);
    let mut serial_ms = Vec::with_capacity(30);
    let mut parallel_ms = Vec::with_capacity(30);
    let mut expected_items = None;

    for seed in 0..30 {
        let started = tokio::time::Instant::now();
        for (_, delay, _) in &delays {
            tokio::time::sleep(*delay).await;
        }
        serial_ms.push(started.elapsed().as_secs_f64() * 1000.0);

        let started = tokio::time::Instant::now();
        let report = engine.discover_seeded(query.clone(), seed).await.expect("fixture discovery");
        parallel_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(report.counts.total, 4);
        assert_eq!(report.counts.success, 2);
        assert_eq!(report.counts.no_data, 1);
        assert_eq!(report.counts.upstream_failed, 1);
        assert!(report.items.iter().all(|item| item.status.is_terminal()));
        if let Some(expected) = &expected_items {
            assert_eq!(&report.items, expected, "seed must not change semantic results");
        } else {
            expected_items = Some(report.items);
        }
    }

    let maximum_active_targets = tracker.maximum.load(Ordering::SeqCst);
    assert_eq!(maximum_active_targets, 4, "all configured workers should run a delayed target");
    println!(
        "{}",
        json!({
            "schema_version": 1,
            "scenario": "four_delayed_fixture_sources",
            "samples_per_path": serial_ms.len(),
            "configured_workers": 4,
            "serial_baseline": {
                "p50_ms": percentile_ms(&serial_ms, 0.50),
                "p95_ms": percentile_ms(&serial_ms, 0.95),
                "method": "same four fixture delays awaited one at a time"
            },
            "native_scheduler": {
                "p50_ms": percentile_ms(&parallel_ms, 0.50),
                "p95_ms": percentile_ms(&parallel_ms, 0.95),
                "maximum_active_targets": maximum_active_targets,
                "requests": 0,
                "method": "Engine discovery with four in-process delay adapters"
            },
            "semantic_results_stable": true,
            "all_targets_terminal": true,
            "provider_requests": "not represented by this in-process fixture"
        })
    );
}
