use chrono::{DateTime, SecondsFormat};
use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::engine::Engine;
use radiust_core::errors::{CoreError, CoreResult};
use radiust_core::model::{DiscoveryTarget, FrameRef, Query, RawFrame, TimeSelector};
use radiust_core::source::SourceRegistry;
use radiust_core::source::{SourceAdapter, SourceContext};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::support;

struct FixtureAdapter {
    timelines: Arc<BTreeMap<String, Vec<FrameRef>>>,
    discovery_calls: Arc<AtomicUsize>,
    file_gets: Arc<AtomicUsize>,
    wait_for_cancel: bool,
}

impl SourceAdapter for FixtureAdapter {
    fn source_id(&self) -> &'static str {
        "rdcap"
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        let calls = self.discovery_calls.clone();
        let station = target.station.unwrap_or_default();
        let frames = self.timelines.get(&station).cloned().unwrap_or_default();
        let token = context.request_budget.cancellation.clone();
        let wait_for_cancel = self.wait_for_cancel;
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if wait_for_cancel {
                tokio::select! {
                    _ = token.cancelled() => Err(CoreError::Cancelled),
                    _ = tokio::time::sleep(Duration::from_secs(30)) => Ok(frames),
                }
            } else {
                Ok(frames)
            }
        })
    }

    fn fetch_raw(
        self: Arc<Self>,
        _frame: FrameRef,
        _context: SourceContext,
        _temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        let file_gets = self.file_gets.clone();
        Some(Box::pin(async move {
            file_gets.fetch_add(1, Ordering::SeqCst);
            Err(CoreError::Cancelled)
        }))
    }
}

fn fixture_adapter(stations: &[&str], wait_for_cancel: bool) -> Arc<FixtureAdapter> {
    let manifest: serde_json::Value = serde_json::from_str(support::FIXTURE_MANIFEST).unwrap();
    let mut timelines = BTreeMap::new();
    for entry in manifest["stations"].as_array().unwrap() {
        let station = entry["station_id"].as_str().unwrap();
        if !stations.contains(&station) {
            continue;
        }
        let relative = entry["files"]["index"]["path"].as_str().unwrap();
        let index: serde_json::Value = serde_json::from_slice(
            &std::fs::read(support::repository_root().join(relative)).unwrap(),
        )
        .unwrap();
        let mut frames = Vec::new();
        for item in index["list"].as_array().unwrap() {
            let key = item["key"].as_u64().unwrap();
            let valid_time = DateTime::from_timestamp_millis(key as i64)
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Micros, true);
            let mut frame = FrameRef {
                source: "rdcap".into(),
                product: "reflectivity".into(),
                station: Some(station.into()),
                valid_time,
                base_time: None,
                logical_id: String::new(),
                revision: None,
                locator_version: "rdcap-v1".into(),
                locator: json!({
                    "country": entry["country"],
                    "station_code": entry["station_code"],
                    "key": key,
                }),
            };
            frame.logical_id = radiust_core::identity::logical_id(&frame).unwrap();
            frames.push(frame);
        }
        timelines.insert(station.to_owned(), frames);
    }
    Arc::new(FixtureAdapter {
        timelines: Arc::new(timelines),
        discovery_calls: Arc::new(AtomicUsize::new(0)),
        file_gets: Arc::new(AtomicUsize::new(0)),
        wait_for_cancel,
    })
}

fn fixture_engine(adapter: Arc<FixtureAdapter>) -> Engine {
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    let mut registry = SourceRegistry::default();
    registry.register(adapter).unwrap();
    Engine::new(config, registry).unwrap()
}

fn query(station: &str, selector: TimeSelector) -> Query {
    Query {
        source: Some("rdcap".into()),
        product: Some("reflectivity".into()),
        stations: vec![station.into()],
        selector,
        ..Query::default()
    }
}

#[tokio::test]
async fn latest_and_exact_at_use_the_observed_epoch_millisecond_keys_without_file_gets() {
    let adapter = fixture_adapter(&["TWRCHL"], false);
    let frames = adapter.timelines["TWRCHL"].clone();
    let engine = fixture_engine(adapter.clone());

    let latest = engine.discover(query("TWRCHL", TimeSelector::Latest)).await.unwrap();
    assert_eq!(latest.items.len(), 1);
    assert_eq!(latest.items[0].status, radiust_core::model::DiscoveryStatus::Success);
    assert_eq!(
        latest.items[0].frame.as_ref().unwrap().logical_id,
        frames.last().unwrap().logical_id
    );

    let exact_time = frames[3].valid_time.clone();
    let exact = engine
        .discover(query("TWRCHL", TimeSelector::At { time: exact_time.clone() }))
        .await
        .unwrap();
    assert_eq!(exact.items.len(), 1);
    assert_eq!(exact.items[0].valid_time.as_deref(), Some(exact_time.as_str()));
    assert_eq!(adapter.discovery_calls.load(Ordering::SeqCst), 2);
    assert_eq!(adapter.file_gets.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn range_selection_is_half_open_and_a_missing_time_does_not_invent_a_frame() {
    let adapter = fixture_adapter(&["JPISHI"], false);
    let frames = adapter.timelines["JPISHI"].clone();
    let engine = fixture_engine(adapter.clone());
    let range = engine
        .discover(query(
            "JPISHI",
            TimeSelector::Range {
                start: frames[2].valid_time.clone(),
                end: frames[5].valid_time.clone(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(range.items.len(), 3);
    assert_eq!(range.counts.total, 3);
    assert_eq!(range.items[0].valid_time.as_deref(), Some(frames[2].valid_time.as_str()));
    assert_eq!(range.items[2].valid_time.as_deref(), Some(frames[4].valid_time.as_str()));

    let missing_time = DateTime::from_timestamp_millis(1_790_000_000_123)
        .unwrap()
        .to_rfc3339_opts(SecondsFormat::Micros, true);
    let missing =
        engine.discover(query("JPISHI", TimeSelector::At { time: missing_time })).await.unwrap();
    assert_eq!(missing.items.len(), 1);
    assert_eq!(missing.items[0].status, radiust_core::model::DiscoveryStatus::NoData);
    assert_eq!(missing.items[0].error.as_ref().unwrap().code, "no_matching_time");
    assert!(missing.items[0].valid_time.is_none());
    assert_eq!(adapter.file_gets.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn empty_index_for_an_active_station_is_no_data_and_stale_latest_is_marked_stale() {
    let catalog = radiust_core::source::catalog::SourceCatalog::builtin().unwrap();
    let aparri = catalog
        .source("rdcap")
        .unwrap()
        .stations
        .iter()
        .find(|station| station.id == "PHAPAR")
        .unwrap();
    assert_eq!(aparri.metadata.as_ref().unwrap().extensions["directory_statuses"][0], "Active");

    let empty_adapter = fixture_adapter(&["PHAPAR"], false);
    let empty_engine = fixture_engine(empty_adapter.clone());
    let empty = empty_engine.discover(query("PHAPAR", TimeSelector::Latest)).await.unwrap();
    assert_eq!(empty.counts.no_data, 1);
    assert_eq!(empty.items[0].error.as_ref().unwrap().code, "no_data");

    let mut stale_frame = FrameRef {
        source: "rdcap".into(),
        product: "reflectivity".into(),
        station: Some("TWRCHL".into()),
        valid_time: "2000-01-01T00:00:00Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: None,
        locator_version: "rdcap-csr-v1".into(),
        locator: json!({"country":"TWN","station_code":"RCHL","key":"946684800000"}),
    };
    stale_frame.logical_id = radiust_core::identity::logical_id(&stale_frame).unwrap();
    let stale_adapter = Arc::new(FixtureAdapter {
        timelines: Arc::new(BTreeMap::from([("TWRCHL".into(), vec![stale_frame])])),
        discovery_calls: Arc::new(AtomicUsize::new(0)),
        file_gets: Arc::new(AtomicUsize::new(0)),
        wait_for_cancel: false,
    });
    let stale_engine = fixture_engine(stale_adapter);
    let stale = stale_engine
        .discover(Query { max_age_secs: Some(3600.0), ..query("TWRCHL", TimeSelector::Latest) })
        .await
        .unwrap();
    assert_eq!(stale.counts.stale, 1);
    assert_eq!(stale.items[0].error.as_ref().unwrap().code, "stale");
}

#[tokio::test]
async fn discovery_cancellation_finishes_without_starting_raw_file_requests() {
    let adapter = fixture_adapter(&["PHSUBI"], true);
    let engine = Arc::new(fixture_engine(adapter.clone()));
    let task_engine = engine.clone();
    let task =
        tokio::spawn(
            async move { task_engine.discover(query("PHSUBI", TimeSelector::Latest)).await },
        );
    tokio::time::sleep(Duration::from_millis(20)).await;
    engine.cancel();
    let report = task.await.unwrap().unwrap();
    assert!(report.interrupted);
    assert!(report.items.iter().any(|item| {
        item.status == radiust_core::model::DiscoveryStatus::Cancelled
            || item.status == radiust_core::model::DiscoveryStatus::NotStarted
    }));
    assert_eq!(adapter.file_gets.load(Ordering::SeqCst), 0);
}
