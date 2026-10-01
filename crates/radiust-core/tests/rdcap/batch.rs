//! RDCAP batch scheduling, cancellation, and partial-result contract tests.

use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::download::{DownloadStatus, FetchErrorPolicy, FetchStatus};
use radiust_core::engine::Engine;
use radiust_core::error_contract::{ErrorCode, ErrorStage};
use radiust_core::errors::{CoreError, CoreResult};
use radiust_core::model::{DiscoveryTarget, FrameRef, RawFrame};
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::support;

struct BatchAdapter {
    fetches: AtomicUsize,
    completion_order: Mutex<Vec<String>>,
}

#[derive(Default)]
struct CancellationState {
    started: AtomicUsize,
    active: AtomicUsize,
    maximum_active: AtomicUsize,
    cancelled_active: AtomicUsize,
    two_started: tokio::sync::Notify,
}

struct CancellationAdapter {
    state: Arc<CancellationState>,
}

struct ActiveCancellationGuard(Arc<CancellationState>);

impl Drop for ActiveCancellationGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
        self.0.cancelled_active.fetch_add(1, Ordering::SeqCst);
    }
}

impl SourceAdapter for CancellationAdapter {
    fn source_id(&self) -> &'static str {
        "rdcap"
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
        _frame: FrameRef,
        context: SourceContext,
        _temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        let state = self.state.clone();
        Some(Box::pin(async move {
            let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
            state.maximum_active.fetch_max(active, Ordering::SeqCst);
            if state.started.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                state.two_started.notify_one();
            }
            let _guard = ActiveCancellationGuard(state);
            tokio::select! {
                _ = context.request_budget.cancellation.cancelled() => Err(CoreError::Cancelled),
                () = std::future::pending::<()>() => unreachable!("fixture only exits on cancellation"),
            }
        }))
    }
}

impl SourceAdapter for BatchAdapter {
    fn source_id(&self) -> &'static str {
        "rdcap"
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
        Some(Box::pin(async move {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            let station = frame.station.clone().expect("RDCAP frame has a station");
            let delay = match station.as_str() {
                "TWN/RCHL" => Duration::from_millis(60),
                "JPN/ISHI" => Duration::from_millis(5),
                "PHL/SUBI" => Duration::from_millis(20),
                _ => panic!("unexpected fixture station: {station}"),
            };
            tokio::time::sleep(delay).await;

            if station == "JPN/ISHI" {
                self.completion_order.lock().unwrap().push(station);
                return Err(CoreError::Transport("simulated ticket expiry".into()));
            }

            let key = frame.locator["key"].as_str().expect("RDCAP frame has epoch key");
            let response = support::reconstructed_file_response(&station);
            let raw = support::fixture_raw_frame(&station, key, &response, &temp_root);
            assert_eq!(raw.frame.logical_id, frame.logical_id);
            self.completion_order.lock().unwrap().push(station);
            Ok(raw)
        }))
    }
}

fn selected_fixture_frame(station_id: &str, temp_root: &std::path::Path) -> FrameRef {
    let manifest: Value = serde_json::from_str(support::FIXTURE_MANIFEST).unwrap();
    let station = manifest["stations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|station| station["station_id"] == station_id)
        .unwrap();
    let key = station["selected_key_epoch_ms"].as_u64().unwrap().to_string();
    support::fixture_raw_frame(
        station_id,
        &key,
        &support::reconstructed_file_response(station_id),
        temp_root,
    )
    .frame
}

#[tokio::test]
async fn decoded_batch_keeps_order_and_successful_country_fields_when_one_fetch_fails() {
    let root = tempfile::tempdir().unwrap();
    let temp_root = root.path().join("temporary");
    let frames = ["TWN/RCHL", "JPN/ISHI", "PHL/SUBI"]
        .into_iter()
        .map(|station| selected_fixture_frame(station, &temp_root))
        .collect::<Vec<_>>();
    let expected_ids = frames.iter().map(|frame| frame.logical_id.clone()).collect::<Vec<_>>();

    let adapter = Arc::new(BatchAdapter {
        fetches: AtomicUsize::new(0),
        completion_order: Mutex::new(Vec::new()),
    });
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.frame_concurrency = 3;
    config.runtime.temp_root = Some(temp_root);
    config.cache.enabled = false;
    let mut registry = SourceRegistry::default();
    registry.register(adapter.clone()).unwrap();
    let engine = Engine::new(config, registry).unwrap();

    let report = engine.fetch_many_decoded(frames, FetchErrorPolicy::Collect, false).await;

    assert_eq!((report.success, report.failed, report.cancelled, report.not_started), (2, 1, 0, 0));
    assert_eq!(adapter.fetches.load(Ordering::SeqCst), 3);
    assert_eq!(*adapter.completion_order.lock().unwrap(), ["JPN/ISHI", "PHL/SUBI", "TWN/RCHL"]);
    assert_eq!(report.items.iter().map(|item| item.input_index).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(
        report.items.iter().map(|item| item.frame.logical_id.clone()).collect::<Vec<_>>(),
        expected_ids
    );
    assert_eq!(
        report.items.iter().map(|item| item.status).collect::<Vec<_>>(),
        [FetchStatus::Success, FetchStatus::Failed, FetchStatus::Success]
    );
    for (index, item) in report.items.iter().enumerate() {
        if index == 1 {
            assert!(item.data.is_none());
            let error = item.error_details.as_ref().expect("failed item retains typed error");
            assert_eq!(error.code, ErrorCode::Transport);
            assert_eq!(error.stage, ErrorStage::Acquire);
        } else {
            let field = item.data.as_ref().expect("successful frame is decoded");
            assert_eq!(field.grid.crs.as_deref(), Some("EPSG:4326"));
            assert_eq!(field.units.as_deref(), Some("dBZ"));
            assert!(field.quality.contains(&65), "annotation quality survives batch decode");
        }
    }
}

#[tokio::test]
async fn stop_policy_does_not_fetch_or_commit_frames_after_the_first_failure() {
    let root = tempfile::tempdir().unwrap();
    let temp_root = root.path().join("temporary");
    let output_root = root.path().join("output");
    let frames = ["JPN/ISHI", "TWN/RCHL", "PHL/SUBI"]
        .into_iter()
        .map(|station| selected_fixture_frame(station, &temp_root))
        .collect::<Vec<_>>();
    let adapter = Arc::new(BatchAdapter {
        fetches: AtomicUsize::new(0),
        completion_order: Mutex::new(Vec::new()),
    });
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.frame_concurrency = 1;
    config.runtime.temp_root = Some(temp_root);
    config.cache.enabled = false;
    config.storage.output = output_root.clone();
    let mut registry = SourceRegistry::default();
    registry.register(adapter.clone()).unwrap();
    let engine = Engine::new(config, registry).unwrap();

    let report = engine
        .download_decoded_to_with_template(
            frames,
            FetchErrorPolicy::Stop,
            false,
            false,
            output_root.clone(),
            "netcdf",
            None,
        )
        .await
        .unwrap();

    assert_eq!(report.failed, 1);
    assert_eq!(adapter.fetches.load(Ordering::SeqCst), 1);
    assert_eq!(report.items[0].status, DownloadStatus::Failed);
    assert!(report.items[1..].iter().all(|item| {
        matches!(item.status, DownloadStatus::Cancelled | DownloadStatus::NotStarted)
    }));
    assert!(!output_root.join("frames").exists(), "failed/late frames must not be committed");
}

#[tokio::test]
async fn rdcap_stop_cancels_late_frames_before_raw_manifests_are_committed() {
    let root = tempfile::tempdir().unwrap();
    let temp_root = root.path().join("temporary");
    let output_root = root.path().join("output");
    let frames = ["JPN/ISHI", "PHL/SUBI", "TWN/RCHL"]
        .into_iter()
        .map(|station| selected_fixture_frame(station, &temp_root))
        .collect::<Vec<_>>();
    let adapter = Arc::new(BatchAdapter {
        fetches: AtomicUsize::new(0),
        completion_order: Mutex::new(Vec::new()),
    });
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.frame_concurrency = frames.len();
    config.runtime.request_concurrency = frames.len();
    config.runtime.temp_root = Some(temp_root.clone());
    config.cache.enabled = false;
    config.storage.output = output_root.clone();
    let mut registry = SourceRegistry::default();
    registry.register(adapter.clone()).unwrap();
    let engine = Engine::new(config, registry).unwrap();

    let report = engine
        .download_raw_only_to(frames, FetchErrorPolicy::Stop, false, false, output_root.clone())
        .await;

    assert_eq!((report.written, report.failed, report.cancelled, report.not_started), (0, 1, 2, 0));
    assert_eq!(adapter.fetches.load(Ordering::SeqCst), 3);
    assert_eq!(
        report.items.iter().map(|item| item.status).collect::<Vec<_>>(),
        [DownloadStatus::Failed, DownloadStatus::Cancelled, DownloadStatus::Cancelled]
    );
    assert!(report.items.iter().all(|item| item.output_uri.is_none()));
    assert!(
        !output_root.join("frames").exists(),
        "late RDCAP frames must not publish raw manifests after stop"
    );
    assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
}

#[tokio::test]
async fn external_cancel_stops_active_rdcap_frames_and_leaves_queued_frame_unstarted() {
    let root = tempfile::tempdir().unwrap();
    let temp_root = root.path().join("temporary");
    let output_root = root.path().join("output");
    let frames = ["TWN/RCHL", "JPN/ISHI", "PHL/SUBI"]
        .into_iter()
        .map(|station| selected_fixture_frame(station, &temp_root))
        .collect::<Vec<_>>();
    let state = Arc::new(CancellationState::default());
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.frame_concurrency = 2;
    config.runtime.request_concurrency = 2;
    config.runtime.temp_root = Some(temp_root.clone());
    config.cache.enabled = false;
    config.storage.output = output_root.clone();
    let mut registry = SourceRegistry::default();
    registry.register(Arc::new(CancellationAdapter { state: state.clone() })).unwrap();
    let engine = Engine::new(config, registry).unwrap();

    let fetch = engine.fetch_many_decoded(frames, FetchErrorPolicy::Collect, false);
    tokio::pin!(fetch);
    let two_started = state.two_started.notified();
    tokio::pin!(two_started);
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            report = &mut fetch => panic!("batch finished before cancellation: {report:?}"),
            () = &mut two_started => (),
        }
    })
    .await
    .unwrap();
    assert_eq!(state.active.load(Ordering::SeqCst), 2);
    engine.cancel();
    let report = tokio::time::timeout(Duration::from_secs(2), &mut fetch).await.unwrap();

    assert_eq!((report.success, report.failed, report.cancelled, report.not_started), (0, 0, 2, 1));
    assert_eq!(report.items.iter().map(|item| item.input_index).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(
        report.items.iter().map(|item| item.status).collect::<Vec<_>>(),
        [FetchStatus::Cancelled, FetchStatus::Cancelled, FetchStatus::NotStarted]
    );
    assert!(report.items.iter().all(|item| item.data.is_none()));
    assert_eq!(state.started.load(Ordering::SeqCst), 2);
    assert_eq!(state.maximum_active.load(Ordering::SeqCst), 2);
    assert_eq!(state.cancelled_active.load(Ordering::SeqCst), 2);
    assert_eq!(std::fs::read_dir(&temp_root).unwrap().count(), 0);
    assert!(!output_root.exists(), "cancelled frames must not create output commits");
}
