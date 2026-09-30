use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::download::{DownloadStatus, FetchErrorPolicy, FetchStatus};
use radiust_core::engine::Engine;
use radiust_core::error_contract::{ErrorCode, ErrorStage};
use radiust_core::errors::{CoreError, CoreResult};
use radiust_core::identity::logical_id;
use radiust_core::model::{
    ArtifactReceipt, DiscoveryStatus, DiscoveryTarget, FrameRef, Query, RawArtifact, RawFrame,
};
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct FixtureAdapter;

impl SourceAdapter for FixtureAdapter {
    fn source_id(&self) -> &'static str {
        "au"
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case("127.0.0.1")
    }

    fn discover(
        self: Arc<Self>,
        _target: DiscoveryTarget,
        _context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

struct LatestIdentityFixtureAdapter {
    candidates: Vec<FrameRef>,
    completed_stations: Option<Arc<Mutex<Vec<String>>>>,
}

impl SourceAdapter for LatestIdentityFixtureAdapter {
    fn source_id(&self) -> &'static str {
        "au"
    }

    fn discover(
        self: Arc<Self>,
        _target: DiscoveryTarget,
        _context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        let candidates = self.candidates.clone();
        Box::pin(async move { Ok(candidates) })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        _context: SourceContext,
        temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        let completed_stations = self.completed_stations.clone();
        Some(Box::pin(async move {
            let station = frame.station.clone().unwrap_or_default();
            if station == "b" {
                tokio::time::sleep(Duration::from_millis(80)).await;
            } else {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            let bytes = frame
                .locator
                .get("payload")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .as_bytes()
                .to_vec();
            std::fs::create_dir_all(&temp_root)
                .map_err(|error| CoreError::Temporary(error.to_string()))?;
            let path = temp_root.join(format!("{}.fixture", frame.logical_id));
            std::fs::write(&path, &bytes)
                .map_err(|error| CoreError::Temporary(error.to_string()))?;
            let path = tempfile::TempPath::try_from_path(path)
                .map_err(|_| CoreError::Temporary("fixture temp path failed".into()))?;
            if let Some(completed_stations) = completed_stations {
                completed_stations.lock().unwrap().push(station);
            }
            Ok(RawFrame {
                frame,
                artifacts: vec![RawArtifact {
                    receipt: ArtifactReceipt {
                        name: "frame.bin".into(),
                        media_type: "application/octet-stream".into(),
                        size_bytes: bytes.len() as u64,
                        sha256: hex::encode(Sha256::digest(&bytes)),
                    },
                    path,
                }],
                private_locator: None,
            })
        }))
    }
}

fn latest_identity_frame(
    station: &str,
    valid_time: &str,
    revision: Option<&str>,
    locator: serde_json::Value,
) -> FrameRef {
    let mut frame = FrameRef {
        source: "au".into(),
        product: "composite".into(),
        station: Some(station.into()),
        valid_time: valid_time.into(),
        base_time: None,
        logical_id: String::new(),
        revision: revision.map(str::to_owned),
        locator_version: "fixture-v1".into(),
        locator,
    };
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

fn latest_identity_engine(temp_root: &Path, adapter: LatestIdentityFixtureAdapter) -> Engine {
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.frame_concurrency = 2;
    config.runtime.request_concurrency = 2;
    config.runtime.temp_root = Some(temp_root.to_path_buf());
    config.cache.enabled = false;
    let mut registry = SourceRegistry::default();
    registry.register(Arc::new(adapter)).unwrap();
    Engine::new(config, registry).unwrap()
}

#[tokio::test]
async fn latest_identity_collision_keeps_equal_time_revisions_ambiguous() {
    let timestamp = "2026-09-22T00:00:00Z";
    let locator = json!({"name": "frame.png"});
    let candidates = vec![
        latest_identity_frame("a", timestamp, Some("r1"), locator.clone()),
        latest_identity_frame("a", timestamp, Some("r2"), locator),
    ];
    assert_eq!(candidates[0].logical_id, candidates[1].logical_id);
    assert_ne!(candidates[0].revision, candidates[1].revision);

    let temp = tempfile::tempdir().unwrap();
    let engine = latest_identity_engine(
        temp.path(),
        LatestIdentityFixtureAdapter { candidates, completed_stations: None },
    );
    // Exercise Engine's explicit source-list ambiguity path: equal-time
    // revisions with one logical identity must remain unresolved.
    let report = engine
        .discover_seeded(Query { sources: vec!["au".into()], ..Query::default() }, 17)
        .await
        .unwrap();

    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].status, DiscoveryStatus::Ambiguous);
    assert!(report.items[0].frame.is_none());
    assert_eq!(
        report.items[0].error.as_ref().unwrap().message,
        "2 candidates for source/product/station; select a single source"
    );
}

#[tokio::test]
async fn latest_discovery_selects_the_newest_frame_for_each_station() {
    let old = "2026-09-20T00:00:00Z";
    let latest = "2026-09-22T00:00:00Z";
    let candidates = ["a", "b"]
        .into_iter()
        .flat_map(|station| {
            [old, latest].into_iter().map(move |time| {
                latest_identity_frame(station, time, None, json!({"name": "frame.png"}))
            })
        })
        .collect();
    let temp = tempfile::tempdir().unwrap();
    let engine = latest_identity_engine(
        temp.path(),
        LatestIdentityFixtureAdapter { candidates, completed_stations: None },
    );

    let report = engine
        .discover_seeded(
            Query {
                source: Some("au".into()),
                product: Some("composite".into()),
                ..Query::default()
            },
            23,
        )
        .await
        .unwrap();

    assert_eq!(report.counts.success, 2);
    let selected = report
        .items
        .iter()
        .map(|item| {
            assert_eq!(item.status, DiscoveryStatus::Success);
            let frame = item.frame.as_ref().unwrap();
            (frame.station.as_deref().unwrap(), frame.valid_time.as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(selected, vec![("a", latest), ("b", latest)]);
}

#[tokio::test]
async fn raw_only_outputs_follow_input_order_when_fixture_fetches_finish_out_of_order() {
    let completed_stations = Arc::new(Mutex::new(Vec::new()));
    let adapter = LatestIdentityFixtureAdapter {
        candidates: Vec::new(),
        completed_stations: Some(completed_stations.clone()),
    };
    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join("output");
    let engine = latest_identity_engine(&temp.path().join("temp"), adapter);
    let frames = vec![
        latest_identity_frame(
            "b",
            "2026-09-22T00:00:00Z",
            Some("revision-b"),
            json!({"payload": "bytes-for-b"}),
        ),
        latest_identity_frame(
            "a",
            "2026-09-22T00:00:00Z",
            Some("revision-a"),
            json!({"payload": "bytes-for-a"}),
        ),
    ];

    let report = engine
        .download_raw_only_to(frames, FetchErrorPolicy::Collect, false, false, output.clone())
        .await;

    assert_eq!(*completed_stations.lock().unwrap(), vec!["a", "b"]);
    assert_eq!(report.written, 2);
    assert_eq!(report.items.iter().map(|item| item.input_index).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(
        report.items.iter().map(|item| item.frame.station.as_deref().unwrap()).collect::<Vec<_>>(),
        vec!["b", "a"]
    );
    let output_uris =
        report.items.iter().map(|item| item.output_uri.as_ref().unwrap()).collect::<Vec<_>>();
    assert_eq!(output_uris.len(), 2);
    assert_ne!(output_uris[0], output_uris[1]);
    for (item, expected) in
        report.items.iter().zip([b"bytes-for-b".as_slice(), b"bytes-for-a".as_slice()])
    {
        let frame_root = output.join("frames").join(&item.frame.logical_id);
        assert!(Path::new(item.output_uri.as_ref().unwrap()).is_file());
        assert!(frame_root.join("raw-manifest.json.manifest.json").is_file());
        assert_eq!(std::fs::read(frame_root.join("raw/frame.bin")).unwrap(), expected);
    }
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 512];
    loop {
        let count = stream.read(&mut buffer).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).unwrap();
        }
    }
}

async fn serve_requests(
    listener: TcpListener,
    expected: usize,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..expected {
        let (mut stream, _) = listener.accept().await.unwrap();
        let active = active.clone();
        let maximum = maximum.clone();
        tasks.spawn(async move {
            let request = read_request(&mut stream).await;
            let path = request.split_whitespace().nth(1).unwrap_or_default();
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(current, Ordering::SeqCst);
            if path == "/slow" {
                tokio::time::sleep(Duration::from_millis(250)).await;
            } else {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let response = if path == "/fail" {
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec()
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nframe".to_vec()
            };
            let _ = stream.write_all(&response).await;
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
    while tasks.join_next().await.is_some() {}
}

fn frame(url: String, station: &str) -> FrameRef {
    let mut frame = FrameRef {
        source: "au".into(),
        product: "composite".into(),
        station: Some(station.into()),
        valid_time: "2026-09-24T00:00:00Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: None,
        locator_version: "fixture-v1".into(),
        locator: json!({"url": url, "name": format!("{station}.bin"), "artifacts": []}),
    };
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

fn engine(cache: &std::path::Path, concurrency: usize) -> Engine {
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.frame_concurrency = concurrency;
    config.runtime.request_concurrency = concurrency;
    config.runtime.temp_root = Some(cache.to_path_buf());
    let mut registry = SourceRegistry::default();
    registry.register(Arc::new(FixtureAdapter)).unwrap();
    Engine::new(config, registry).unwrap()
}

#[derive(Default)]
struct ScriptedState {
    started: std::sync::Mutex<Vec<String>>,
    cancelled_active: AtomicUsize,
    prefix_release: tokio::sync::Notify,
    active_ready: tokio::sync::Notify,
    active_started: tokio::sync::Notify,
    active_cancelled: tokio::sync::Notify,
}

struct ScriptedAdapter {
    state: Arc<ScriptedState>,
}

struct ActiveFetchGuard(Arc<ScriptedState>);

impl Drop for ActiveFetchGuard {
    fn drop(&mut self) {
        self.0.cancelled_active.fetch_add(1, Ordering::SeqCst);
        self.0.active_cancelled.notify_one();
    }
}

fn temporary_raw_frame(
    frame: FrameRef,
    temp_root: &std::path::Path,
    bytes: &[u8],
) -> CoreResult<RawFrame> {
    std::fs::create_dir_all(temp_root).map_err(|error| CoreError::Temporary(error.to_string()))?;
    let path = temp_root.join(format!("{}.bin", frame.logical_id));
    std::fs::write(&path, bytes).map_err(|error| CoreError::Temporary(error.to_string()))?;
    let path = tempfile::TempPath::try_from_path(path)
        .map_err(|_| CoreError::Temporary("fixture temp path failed".into()))?;
    Ok(RawFrame {
        frame,
        artifacts: vec![RawArtifact {
            receipt: ArtifactReceipt {
                name: "fixture.bin".into(),
                media_type: "application/octet-stream".into(),
                size_bytes: bytes.len() as u64,
                sha256: hex::encode(Sha256::digest(bytes)),
            },
            path,
        }],
        private_locator: None,
    })
}

impl SourceAdapter for ScriptedAdapter {
    fn source_id(&self) -> &'static str {
        "au"
    }

    fn discover(
        self: Arc<Self>,
        _target: DiscoveryTarget,
        _context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        _context: SourceContext,
        temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        let state = self.state.clone();
        Some(Box::pin(async move {
            let station = frame.station.clone().unwrap_or_default();
            state.started.lock().unwrap().push(station.clone());
            match station.as_str() {
                "prefix" => {
                    state.prefix_release.notified().await;
                    temporary_raw_frame(frame, &temp_root, b"completed prefix")
                }
                "failure" => {
                    state.active_ready.notified().await;
                    Err(CoreError::Transport("scripted acquisition failure".into()))
                }
                "active" => {
                    let raw = temporary_raw_frame(frame, &temp_root, b"active stage")?;
                    let _guard = ActiveFetchGuard(state);
                    _guard.0.active_started.notify_one();
                    _guard.0.active_ready.notify_one();
                    std::future::pending::<()>().await;
                    drop(raw);
                    unreachable!("pending fixture fetch only exits by cancellation")
                }
                _ => temporary_raw_frame(frame, &temp_root, b"unexpected queued fetch"),
            }
        }))
    }
}

#[tokio::test]
async fn stop_download_keeps_slow_prefix_cancels_active_later_frame_and_never_starts_queue() {
    let root = tempfile::tempdir().unwrap();
    let temp_root = root.path().join("temp");
    let output_root = root.path().join("output");
    let state = Arc::new(ScriptedState::default());
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.frame_concurrency = 3;
    config.runtime.request_concurrency = 3;
    config.runtime.temp_root = Some(temp_root.clone());
    config.storage.output = output_root.clone();
    let mut registry = SourceRegistry::default();
    registry.register(Arc::new(ScriptedAdapter { state: state.clone() })).unwrap();
    let engine = Engine::new(config, registry).unwrap();
    let frames = ["prefix", "failure", "active", "queued-1", "queued-2"]
        .into_iter()
        .map(|station| frame("http://fixture.invalid/unused".into(), station))
        .collect::<Vec<_>>();

    let active_started = state.active_started.notified();
    let active_cancelled = state.active_cancelled.notified();
    let download_frames = frames.clone();
    let download = tokio::spawn(async move {
        engine.download_raw_only(download_frames, FetchErrorPolicy::Stop, false, false).await
    });
    tokio::time::timeout(Duration::from_secs(2), active_started).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), active_cancelled).await.unwrap();
    state.prefix_release.notify_one();
    let report = download.await.unwrap();

    assert_eq!(report.items.len(), 5);
    assert_eq!(
        report.items.iter().map(|item| item.input_index).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(
        report.items.iter().map(|item| item.status).collect::<Vec<_>>(),
        vec![
            DownloadStatus::Written,
            DownloadStatus::Failed,
            DownloadStatus::Cancelled,
            DownloadStatus::NotStarted,
            DownloadStatus::NotStarted,
        ]
    );
    assert_eq!(report.written, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(report.cancelled, 1);
    assert_eq!(report.not_started, 2);
    assert!(!report.interrupted);

    let failure = report.items[1].error.as_ref().unwrap();
    assert_eq!(failure.code, ErrorCode::Transport);
    assert_eq!(failure.stage, ErrorStage::Acquire);
    assert!(failure.retryable);
    assert_eq!(failure.message, "transport request failed");

    let committed = std::path::Path::new(report.items[0].output_uri.as_ref().unwrap());
    assert!(committed.is_file());
    assert!(
        output_root
            .join(format!("frames/{}/raw-manifest.json.manifest.json", frames[0].logical_id))
            .is_file()
    );
    for (index, frame) in frames.iter().enumerate().skip(1) {
        assert!(report.items[index].output_uri.is_none());
        assert!(!output_root.join(format!("frames/{}", frame.logical_id)).exists());
    }
    assert_eq!(std::fs::read_dir(output_root.join("frames")).unwrap().count(), 1);
    assert!(
        std::fs::read_dir(&output_root)
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(".radiust-stage-"))
    );
    assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);

    let mut started = state.started.lock().unwrap().clone();
    started.sort();
    assert_eq!(started, vec!["active", "failure", "prefix"]);
    assert_eq!(state.cancelled_active.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn collect_policy_is_bounded_and_keeps_input_order_across_partial_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let server = tokio::spawn(serve_requests(listener, 4, active, maximum.clone()));
    let cache = tempfile::tempdir().unwrap();
    let engine = engine(cache.path(), 2);
    let frames = ["/0", "/1", "/fail", "/3"]
        .into_iter()
        .enumerate()
        .map(|(index, path)| frame(format!("http://{address}{path}"), &format!("S{index}")))
        .collect::<Vec<_>>();
    let expected_ids = frames.iter().map(|frame| frame.logical_id.clone()).collect::<Vec<_>>();

    let report = engine.fetch_many_raw(frames, FetchErrorPolicy::Collect, false).await;
    server.await.unwrap();

    assert_eq!(report.success, 3);
    assert_eq!(report.failed, 1);
    assert_eq!(
        report.items.iter().map(|item| item.frame.logical_id.clone()).collect::<Vec<_>>(),
        expected_ids
    );
    assert_eq!(
        report.items.iter().map(|item| item.input_index).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(report.items[2].status, FetchStatus::Failed);
    assert!(maximum.load(Ordering::SeqCst) <= 2);
}

#[tokio::test]
async fn stop_policy_cancels_active_and_marks_queued_frames_not_started() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_requests(
        listener,
        2,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    ));
    let cache = tempfile::tempdir().unwrap();
    let engine = engine(cache.path(), 2);
    let frames = ["/fail", "/slow", "/queued-1", "/queued-2"]
        .into_iter()
        .enumerate()
        .map(|(index, path)| frame(format!("http://{address}{path}"), &format!("S{index}")))
        .collect::<Vec<_>>();

    let report = engine.fetch_many_raw(frames, FetchErrorPolicy::Stop, false).await;
    server.await.unwrap();

    assert_eq!(report.failed, 1);
    assert_eq!(report.cancelled, 1);
    assert_eq!(report.not_started, 2);
    assert_eq!(report.items[0].status, FetchStatus::Failed);
    assert_eq!(report.items[1].status, FetchStatus::Cancelled);
    assert_eq!(report.items[2].status, FetchStatus::NotStarted);
    assert_eq!(report.items[3].status, FetchStatus::NotStarted);
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn dry_run_marks_every_input_planned_without_network_access() {
    let cache = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::default();
    config.runtime.allow_network = false;
    config.runtime.frame_concurrency = 1;
    config.runtime.temp_root = Some(cache.path().to_path_buf());
    let mut registry = SourceRegistry::default();
    registry.register(Arc::new(FixtureAdapter)).unwrap();
    let engine = Engine::new(config, registry).unwrap();
    let frames = vec![frame("http://127.0.0.1/not-requested".into(), "S0")];

    let report = engine.fetch_many_raw(frames, FetchErrorPolicy::Collect, true).await;

    assert_eq!(report.planned, 1);
    assert_eq!(report.items[0].status, FetchStatus::Planned);
    assert!(report.items[0].raw.is_none());
}

#[tokio::test]
async fn external_cancellation_marks_active_frames_cancelled_and_queued_frames_not_started() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_requests(
        listener,
        2,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    ));
    let cache = tempfile::tempdir().unwrap();
    let engine = engine(cache.path(), 2);
    let frames = (0..4)
        .map(|index| frame(format!("http://{address}/slow"), &format!("S{index}")))
        .collect::<Vec<_>>();

    let report_future = engine.fetch_many_raw(frames, FetchErrorPolicy::Collect, false);
    tokio::pin!(report_future);
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut report_future).await.is_err());
    engine.cancel();
    let report = report_future.await;
    server.await.unwrap();

    assert_eq!(report.cancelled, 2);
    assert_eq!(report.not_started, 2);
    assert_eq!(report.failed, 0);
    assert!(report.items[..2].iter().all(|item| item.status == FetchStatus::Cancelled));
    assert!(report.items[2..].iter().all(|item| item.status == FetchStatus::NotStarted));
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn legacy_batch_policy_aliases_map_to_bounded_execution_policies() {
    assert_eq!(FetchErrorPolicy::parse("collect"), Some(FetchErrorPolicy::Collect));
    assert_eq!(FetchErrorPolicy::parse("continue"), Some(FetchErrorPolicy::Collect));
    assert_eq!(FetchErrorPolicy::parse("stop"), Some(FetchErrorPolicy::Stop));
    assert_eq!(FetchErrorPolicy::parse("raise"), Some(FetchErrorPolicy::Stop));
    assert_eq!(FetchErrorPolicy::parse("ignore"), None);
}

const BATCH_BENCHMARK_ROUNDS: usize = 30;
const BATCH_BENCHMARK_FRAMES: usize = 16;
const BATCH_BENCHMARK_CONCURRENCY: usize = 4;
const BATCH_BENCHMARK_BODY_BYTES: usize = 64 * 1024;

fn batch_benchmark_payload(index: usize) -> Vec<u8> {
    vec![(index as u8).wrapping_add(17); BATCH_BENCHMARK_BODY_BYTES]
}

async fn serve_batch_benchmark_requests(
    listener: TcpListener,
    expected: usize,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    paths: Arc<Mutex<Vec<String>>>,
) {
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..expected {
        let (mut stream, peer) = listener.accept().await.unwrap();
        assert!(peer.ip().is_loopback(), "benchmark peer escaped loopback: {peer}");
        let active = active.clone();
        let maximum = maximum.clone();
        let paths = paths.clone();
        tasks.spawn(async move {
            let request = read_request(&mut stream).await;
            let path = request.split_whitespace().nth(1).unwrap_or_default().to_owned();
            paths.lock().unwrap().push(path.clone());
            let index = path
                .rsplit('/')
                .next()
                .and_then(|name| name.strip_prefix("asset-"))
                .and_then(|value| value.parse::<usize>().ok())
                .expect("benchmark request path includes an asset index");
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(8)).await;
            let payload = batch_benchmark_payload(index);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.write_all(&payload).await.unwrap();
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
    while tasks.join_next().await.is_some() {}
}

/// Manual-only acceptance benchmark. Run through scripts/validation/benchmark_rust_batch.py.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual loopback-only 30-round batch download benchmark"]
async fn batch_download_benchmark_loopback_raw_only_30_rounds() {
    let mut round_results = Vec::with_capacity(BATCH_BENCHMARK_ROUNDS);
    for round in 0..BATCH_BENCHMARK_ROUNDS {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let request_paths = Arc::new(Mutex::new(Vec::with_capacity(BATCH_BENCHMARK_FRAMES)));
        let server = tokio::spawn(serve_batch_benchmark_requests(
            listener,
            BATCH_BENCHMARK_FRAMES,
            active,
            maximum.clone(),
            request_paths.clone(),
        ));

        let round_root = tempfile::tempdir().unwrap();
        let temp_root = round_root.path().join("temp");
        let output_root = round_root.path().join("output");
        std::fs::create_dir_all(&temp_root).unwrap();
        let mut config = CoreConfig::default();
        config.runtime.allow_network = true;
        config.runtime.frame_concurrency = BATCH_BENCHMARK_CONCURRENCY;
        config.runtime.request_concurrency = BATCH_BENCHMARK_CONCURRENCY;
        config.runtime.temp_root = Some(temp_root.clone());
        config.cache.enabled = false;
        let mut registry = SourceRegistry::default();
        registry.register(Arc::new(FixtureAdapter)).unwrap();
        let engine = Engine::new(config, registry).unwrap();
        let frames = (0..BATCH_BENCHMARK_FRAMES)
            .map(|index| {
                frame(
                    format!("http://{address}/round-{round}/asset-{index}"),
                    &format!("B{index:02}"),
                )
            })
            .collect::<Vec<_>>();
        let expected_ids = frames.iter().map(|item| item.logical_id.clone()).collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let report = engine
            .download_raw_only_to(
                frames,
                FetchErrorPolicy::Collect,
                false,
                false,
                output_root.clone(),
            )
            .await;
        let elapsed_seconds = started.elapsed().as_secs_f64();
        tokio::time::timeout(Duration::from_secs(10), server).await.unwrap().unwrap();

        assert_eq!(report.written, BATCH_BENCHMARK_FRAMES);
        assert_eq!(report.failed, 0);
        assert_eq!(report.items.len(), BATCH_BENCHMARK_FRAMES);
        assert_eq!(
            report.items.iter().map(|item| item.input_index).collect::<Vec<_>>(),
            (0..BATCH_BENCHMARK_FRAMES).collect::<Vec<_>>()
        );
        assert_eq!(
            report.items.iter().map(|item| item.frame.logical_id.clone()).collect::<Vec<_>>(),
            expected_ids
        );

        let received_paths = request_paths.lock().unwrap().clone();
        let expected_paths = (0..BATCH_BENCHMARK_FRAMES)
            .map(|index| format!("/round-{round}/asset-{index}"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(received_paths.len(), BATCH_BENCHMARK_FRAMES);
        assert_eq!(
            received_paths.iter().cloned().collect::<std::collections::BTreeSet<_>>(),
            expected_paths
        );
        let observed_concurrency = maximum.load(Ordering::SeqCst);
        assert!(observed_concurrency > 1, "loopback fixture did not exercise concurrency");
        assert!(observed_concurrency <= BATCH_BENCHMARK_CONCURRENCY);

        for (index, item) in report.items.iter().enumerate() {
            assert_eq!(item.status, DownloadStatus::Written);
            let frame_root = output_root.join("frames").join(&item.frame.logical_id);
            let raw_manifest_path = frame_root.join("raw-manifest.json");
            assert_eq!(
                Path::new(item.output_uri.as_ref().unwrap()).canonicalize().unwrap(),
                raw_manifest_path.canonicalize().unwrap()
            );
            let commit_manifest_path = frame_root.join("raw-manifest.json.manifest.json");
            let commit_manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&commit_manifest_path).unwrap()).unwrap();
            let committed_artifacts = commit_manifest["artifacts"].as_array().unwrap();
            assert_eq!(committed_artifacts.len(), 2);
            for artifact in committed_artifacts {
                let relative_uri = artifact["relative_uri"].as_str().unwrap();
                let bytes = std::fs::read(output_root.join(relative_uri)).unwrap();
                assert_eq!(artifact["size_bytes"].as_u64().unwrap(), bytes.len() as u64);
                assert_eq!(
                    artifact["sha256"].as_str().unwrap(),
                    hex::encode(Sha256::digest(&bytes))
                );
            }

            let raw_manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&raw_manifest_path).unwrap()).unwrap();
            assert_eq!(raw_manifest["raw_complete"], true);
            let raw_artifacts = raw_manifest["artifacts"].as_array().unwrap();
            assert_eq!(raw_artifacts.len(), 1);
            let artifact = &raw_artifacts[0];
            let name = artifact["name"].as_str().unwrap();
            assert_eq!(name, format!("B{index:02}.bin"));
            let payload = batch_benchmark_payload(index);
            let payload_path = frame_root.join("raw").join(name);
            let readback = std::fs::read(&payload_path).unwrap();
            assert_eq!(readback, payload);
            assert_eq!(artifact["size_bytes"].as_u64().unwrap(), readback.len() as u64);
            assert_eq!(
                artifact["sha256"].as_str().unwrap(),
                hex::encode(Sha256::digest(&readback))
            );
        }

        let temp_residue_count = std::fs::read_dir(&temp_root).unwrap().count();
        assert_eq!(temp_residue_count, 0, "round {round} left files in the temp root");
        assert!(
            std::fs::read_dir(&output_root)
                .unwrap()
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().starts_with(".radiust-stage-")),
            "round {round} left an output staging directory"
        );

        round_results.push(json!({
            "round": round + 1,
            "elapsed_seconds": elapsed_seconds,
            "request_count": received_paths.len(),
            "observed_server_concurrency": observed_concurrency,
            "configured_concurrency_limit": BATCH_BENCHMARK_CONCURRENCY,
            "input_order_verified": true,
            "manifest_and_artifact_sha256_readback_verified": true,
            "temp_residue_count": temp_residue_count,
        }));
    }

    println!(
        "BATCH_BENCHMARK_JSON={}",
        serde_json::to_string(&json!({
            "rounds": round_results,
            "frames_per_round": BATCH_BENCHMARK_FRAMES,
            "body_bytes_per_artifact": BATCH_BENCHMARK_BODY_BYTES,
            "configured_concurrency_limit": BATCH_BENCHMARK_CONCURRENCY,
            "listener_host": "127.0.0.1",
            "public_network_used": false,
        }))
        .unwrap()
    );
}
