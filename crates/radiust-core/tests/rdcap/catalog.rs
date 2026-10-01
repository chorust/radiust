use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::engine::{Engine, EngineError};
use radiust_core::errors::{CoreError, CoreResult, ProviderError};
use radiust_core::model::{DiscoveryStatus, DiscoveryTarget, FrameRef, Query};
use radiust_core::source::catalog::SourceCatalog;
use radiust_core::source::catalog::{CatalogStation, StationCatalogUpdate};
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[test]
fn builtin_catalog_has_a_deduplicated_48_station_rdcap_snapshot() {
    let catalog = SourceCatalog::builtin().unwrap();
    assert_eq!(catalog.sources.len(), 25);
    let old_sources = catalog.sources.iter().filter(|source| source.id != "rdcap").count();
    assert_eq!(old_sources, 24);

    let source = catalog.source("rdcap").unwrap();
    assert_eq!(catalog.default_product("rdcap"), Some("reflectivity"));
    assert_eq!(source.stations.len(), 48);
    assert_eq!(source.products.len(), 1);
    assert_eq!(source.metadata.as_ref().unwrap().extensions["snapshot_record_count"], 49);

    for (country, expected) in [("TWN", 13), ("JPN", 20), ("PHL", 15)] {
        let count = source
            .stations
            .iter()
            .filter(|station| station.id.starts_with(&format!("{country}/")))
            .count();
        assert_eq!(count, expected, "{country}");
        assert_eq!(
            source.metadata.as_ref().unwrap().country_capabilities[country].raw_acquisition,
            radiust_core::source::catalog::CatalogCapabilityStatus::Unverified
        );
    }

    let ids = vec!["rdcap".to_owned()];
    let targets = catalog.expand_targets(Some(&ids)).unwrap();
    assert_eq!(targets.len(), 48);
    assert!(targets.iter().all(|target| target.product.as_deref() == Some("reflectivity")));
    assert!(targets.iter().all(|target| {
        target.station.as_deref().is_some_and(|station| {
            station.starts_with("TWN/")
                || station.starts_with("JPN/")
                || station.starts_with("PHL/")
        })
    }));
}

#[test]
fn bale_status_conflict_and_unverified_science_limits_are_preserved() {
    let catalog = SourceCatalog::builtin().unwrap();
    let source = catalog.source("rdcap").unwrap();
    let bale = source.stations.iter().find(|station| station.id == "PHL/BALE").unwrap();
    assert_eq!(bale.name, "Baler");
    let metadata = bale.metadata.as_ref().unwrap();
    assert_eq!(metadata.extensions["directory_record_ids"], serde_json::json!(["3004", "5031"]));
    assert_eq!(
        metadata.extensions["directory_statuses"],
        serde_json::json!(["Inactive", "Active"])
    );
    let conflict =
        metadata.directory_conflicts.iter().find(|conflict| conflict.field == "Status").unwrap();
    assert_eq!(conflict.station_id, "PHL/BALE");
    assert_eq!(conflict.values, vec![serde_json::json!("Inactive"), serde_json::json!("Active")]);

    let product = &source.products[0];
    assert!(!product.historical);
    assert_eq!(product.metadata.as_ref().unwrap().extensions["scan_height"], "unknown");
    assert_eq!(product.metadata.as_ref().unwrap().extensions["upstream_qc"], "unknown");
}

#[derive(Clone)]
struct DirectoryAdapter {
    update: Option<StationCatalogUpdate>,
    refresh_error: bool,
    refresh_delay: Duration,
    wait_for_cancel: bool,
    refresh_count: Arc<AtomicUsize>,
    discovered: Arc<Mutex<Vec<DiscoveryTarget>>>,
}

impl SourceAdapter for DirectoryAdapter {
    fn source_id(&self) -> &'static str {
        "rdcap"
    }

    fn refresh_station_catalog(
        self: Arc<Self>,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Option<StationCatalogUpdate>>> {
        Box::pin(async move {
            self.refresh_count.fetch_add(1, Ordering::SeqCst);
            if self.wait_for_cancel {
                context.request_budget.cancellation.cancelled().await;
                return Err(CoreError::Cancelled);
            }
            if !self.refresh_delay.is_zero() {
                tokio::time::sleep(self.refresh_delay).await;
            }
            if self.refresh_error {
                return Err(CoreError::Provider(ProviderError::CatalogUnavailable));
            }
            Ok(self.update.clone())
        })
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        _context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            self.discovered.lock().unwrap().push(target);
            Ok(Vec::new())
        })
    }
}

fn directory_adapter(stations: Vec<CatalogStation>) -> DirectoryAdapter {
    DirectoryAdapter {
        update: Some(StationCatalogUpdate { source_id: "rdcap".into(), stations, metadata: None }),
        refresh_error: false,
        refresh_delay: Duration::ZERO,
        wait_for_cancel: false,
        refresh_count: Arc::new(AtomicUsize::new(0)),
        discovered: Arc::new(Mutex::new(Vec::new())),
    }
}

fn live_station(id: &str, name: &str) -> CatalogStation {
    CatalogStation {
        id: id.into(),
        name: name.into(),
        longitude: None,
        latitude: None,
        product_ids: vec!["reflectivity".into()],
        metadata: None,
    }
}

fn engine_with_adapter(mut config: CoreConfig, adapter: DirectoryAdapter) -> Engine {
    config.runtime.allow_network = true;
    let mut overrides = SourceRegistry::default();
    overrides.register(Arc::new(adapter)).unwrap();
    Engine::new(config, overrides).unwrap()
}

#[tokio::test]
async fn engine_refreshes_once_merges_new_and_snapshot_stations_and_normalizes_short_codes() {
    let adapter = directory_adapter(vec![
        live_station("PHL/NEW1", "New Radar"),
        live_station("TWN/RCHL", "RCHL"),
    ]);
    let refresh_count = adapter.refresh_count.clone();
    let discovered = adapter.discovered.clone();
    let engine = engine_with_adapter(CoreConfig::default(), adapter);

    let report = engine
        .discover_seeded(
            Query {
                source: Some("rdcap".into()),
                stations: vec!["NEW1".into(), "RCHL".into()],
                ..Query::default()
            },
            19,
        )
        .await
        .unwrap();

    assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(report.counts.total, 2);
    assert!(report.items.iter().all(|item| item.status == DiscoveryStatus::NoData));
    let mut station_ids = discovered
        .lock()
        .unwrap()
        .iter()
        .filter_map(|target| target.station.clone())
        .collect::<Vec<_>>();
    station_ids.sort();
    assert_eq!(station_ids, ["PHL/NEW1", "TWN/RCHL"]);
}

#[tokio::test]
async fn short_code_ambiguity_is_reported_without_dispatch() {
    let adapter = directory_adapter(vec![
        live_station("TWN/DUP1", "Taiwan"),
        live_station("PHL/DUP1", "Philippines"),
    ]);
    let refresh_count = adapter.refresh_count.clone();
    let discovered = adapter.discovered.clone();
    let engine = engine_with_adapter(CoreConfig::default(), adapter);

    let report = engine
        .discover_seeded(
            Query {
                source: Some("rdcap".into()),
                stations: vec!["DUP1".into()],
                ..Query::default()
            },
            23,
        )
        .await
        .unwrap();

    assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(report.counts.ambiguous, 1);
    assert_eq!(report.items[0].error.as_ref().unwrap().code, "ambiguous_index");
    assert!(discovered.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refresh_failure_keeps_snapshot_targets_and_marks_unknown_selectors_unavailable() {
    let mut adapter = directory_adapter(Vec::new());
    adapter.update = None;
    adapter.refresh_error = true;
    let refresh_count = adapter.refresh_count.clone();
    let discovered = adapter.discovered.clone();
    let engine = engine_with_adapter(CoreConfig::default(), adapter);

    let report = engine
        .discover_seeded(
            Query {
                source: Some("rdcap".into()),
                stations: vec!["TWN/RCHL".into(), "NOPE".into()],
                ..Query::default()
            },
            29,
        )
        .await
        .unwrap();

    assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(report.counts.total, 2);
    assert_eq!(report.counts.no_data, 1);
    assert_eq!(report.items[0].error.as_ref().unwrap().code, "catalog_unavailable");
    assert_eq!(
        discovered.lock().unwrap().as_slice(),
        [DiscoveryTarget {
            source: "rdcap".into(),
            product: Some("reflectivity".into()),
            station: Some("TWN/RCHL".into()),
        }]
    );
}

#[tokio::test]
async fn normalized_duplicate_station_selection_is_rejected() {
    let adapter = directory_adapter(vec![live_station("TWN/RCHL", "Hua-Lien")]);
    let engine = engine_with_adapter(CoreConfig::default(), adapter);

    let result = engine
        .discover_seeded(
            Query {
                source: Some("rdcap".into()),
                stations: vec!["TWN/RCHL".into(), "RCHL".into()],
                ..Query::default()
            },
            31,
        )
        .await;

    assert!(matches!(result, Err(EngineError::InvalidQuery(_))));
}

#[tokio::test]
async fn catalog_refresh_consumes_the_shared_discovery_deadline() {
    let mut adapter = directory_adapter(vec![live_station("TWN/RCHL", "Hua-Lien")]);
    adapter.refresh_delay = Duration::from_millis(80);
    let refresh_count = adapter.refresh_count.clone();
    let discovered = adapter.discovered.clone();
    let mut config = CoreConfig::default();
    config.runtime.discovery_deadline = 0.02;
    let engine = engine_with_adapter(config, adapter);

    let report = engine
        .discover_seeded(
            Query {
                source: Some("rdcap".into()),
                stations: vec!["TWN/RCHL".into()],
                ..Query::default()
            },
            37,
        )
        .await
        .unwrap();

    assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
    assert_eq!(report.counts.timeout, 1);
    assert!(discovered.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_interrupts_the_catalog_refresh_and_prevents_station_dispatch() {
    let mut adapter = directory_adapter(vec![live_station("TWN/RCHL", "Hua-Lien")]);
    adapter.wait_for_cancel = true;
    let refresh_count = adapter.refresh_count.clone();
    let discovered = adapter.discovered.clone();
    let engine = Arc::new(engine_with_adapter(CoreConfig::default(), adapter));
    let task_engine = engine.clone();
    let task = tokio::spawn(async move {
        task_engine
            .discover_seeded(
                Query {
                    source: Some("rdcap".into()),
                    stations: vec!["TWN/RCHL".into()],
                    ..Query::default()
                },
                41,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    engine.cancel();
    let report = task.await.unwrap().unwrap();

    assert_eq!(refresh_count.load(Ordering::SeqCst), 1);
    assert!(report.interrupted);
    assert_eq!(report.counts.cancelled, 1);
    assert!(discovered.lock().unwrap().is_empty());
}
