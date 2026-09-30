//! Temporary Cargo helper source used by benchmark_rust_migration.py.
//!
//! All adapter URLs are constructed from a benchmark-owned IPv4 loopback
//! server. The fixture adapters shadow four catalogue entries in this helper
//! process only; no product adapter or product source is changed.

use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::download::{FetchErrorPolicy, FetchStatus, fetch_many_raw};
use radiust_core::engine::Engine;
use radiust_core::errors::{CoreError, CoreResult};
use radiust_core::identity::logical_id;
use radiust_core::model::{DiscoveryTarget, FrameRef, Query};
use radiust_core::source::catalog::SourceCatalog;
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use serde_json::{Value, json};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const SOURCES: [&str; 4] = ["au", "es", "fr", "vn"];

struct ReplayAdapter {
    id: &'static str,
    server: String,
    token: String,
    phase: Arc<AtomicUsize>,
}

impl ReplayAdapter {
    fn url(&self, phase: &str, kind: &str) -> String {
        format!("{}/{}/{}/{}/{}/{}", self.server, self.token, "native", phase, kind, self.id)
    }
}

impl SourceAdapter for ReplayAdapter {
    fn source_id(&self) -> &'static str {
        self.id
    }

    fn allows_artifact_host(&self, host: &str) -> bool {
        host == "127.0.0.1"
    }

    fn discover(
        self: Arc<Self>,
        target: DiscoveryTarget,
        context: SourceContext,
    ) -> BoxFuture<'static, CoreResult<Vec<FrameRef>>> {
        Box::pin(async move {
            let phase = match self.phase.load(Ordering::SeqCst) {
                0 => "cold",
                _ => "warm",
            };
            let body = context.http_transport.get_bytes(&self.url(phase, "discover")).await?;
            let row: Value = serde_json::from_slice(&body)
                .map_err(|_| CoreError::Transport("fixture response was invalid JSON".into()))?;
            let source = row.get("source").and_then(Value::as_str).unwrap_or(self.id);
            let product = row
                .get("product")
                .and_then(Value::as_str)
                .or(target.product.as_deref())
                .ok_or_else(|| CoreError::Transport("fixture product was missing".into()))?;
            let valid_time = row
                .get("valid_time")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Transport("fixture time was missing".into()))?;
            let revision = row
                .get("revision")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::Transport("fixture revision was missing".into()))?;
            let artifact_url = self.url("batch", "artifact");
            let mut frame = FrameRef {
                source: source.to_owned(),
                product: product.to_owned(),
                station: row
                    .get("station")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or(target.station),
                valid_time: valid_time.to_owned(),
                base_time: None,
                logical_id: String::new(),
                revision: Some(revision.to_owned()),
                locator_version: "1".into(),
                locator: json!({
                    "url": artifact_url,
                    "name": "fixed-frame.png",
                    "media_type": "image/png"
                }),
            };
            frame.logical_id = logical_id(&frame)
                .map_err(|_| CoreError::Transport("fixture identity was invalid".into()))?;
            Ok(vec![frame])
        })
    }
}

fn source_semantic(catalog: &SourceCatalog) -> Value {
    let mut rows = catalog
        .sources
        .iter()
        .map(|source| {
            let mut products = source.products.iter().map(|product| product.id.clone()).collect::<Vec<_>>();
            products.sort();
            json!({
                "id": source.id,
                "description": source.description,
                "availability": source.availability,
                "products": products,
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left["id"].as_str().cmp(&right["id"].as_str()));
    json!({"sources": rows})
}

fn discovery_query_all() -> Query {
    Query { source: Some("all".into()), ..Query::default() }
}

fn fixture_query() -> Query {
    Query { sources: SOURCES.iter().map(|source| (*source).to_owned()).collect(), ..Query::default() }
}

fn fixture_semantic(report: &radiust_core::model::DiscoveryReport) -> Value {
    let mut rows = report
        .items
        .iter()
        .map(|item| {
            let frame = item.frame.as_ref();
            json!({
                "source": item.target.source,
                "product": item.target.product,
                "station": item.target.station,
                "status": format!("{:?}", item.status).to_ascii_lowercase(),
                "valid_time": frame.map(|frame| frame.valid_time.clone()),
                "logical_id": frame.map(|frame| frame.logical_id.clone()),
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left["source"].as_str().cmp(&right["source"].as_str()));
    json!({"items": rows})
}

async fn run(server: String, token: String, runtime_root: PathBuf) -> Result<Value, Box<dyn std::error::Error>> {
    let setup_started = Instant::now();
    let mut offline_config = CoreConfig::default();
    offline_config.cache.enabled = false;
    offline_config.cache.dir = runtime_root.join("cache-offline");
    offline_config.runtime.temp_root = Some(runtime_root.join("tmp-offline"));
    let offline_engine = Engine::new(offline_config, SourceRegistry::default())?;

    let mut override_registry = SourceRegistry::default();
    let phase = Arc::new(AtomicUsize::new(0));
    for id in SOURCES {
        override_registry.register(Arc::new(ReplayAdapter {
            id,
            server: server.clone(),
            token: token.clone(),
            phase: phase.clone(),
        }))?;
    }
    let mut fixture_config = CoreConfig::default();
    fixture_config.runtime.allow_network = true;
    fixture_config.runtime.discovery_workers = 4;
    fixture_config.runtime.frame_concurrency = 4;
    fixture_config.runtime.request_concurrency = 8;
    fixture_config.runtime.host_concurrency = 4;
    fixture_config.runtime.temp_root = Some(runtime_root.join("tmp-loopback"));
    fixture_config.cache.enabled = false;
    fixture_config.cache.dir = runtime_root.join("cache-loopback");
    fixture_config.storage.output = runtime_root.join("output");
    let fixture_engine = Engine::new(fixture_config, override_registry)?;
    let setup_seconds = setup_started.elapsed().as_secs_f64();

    let mut operations = serde_json::Map::new();
    let mut list_rows = Vec::new();
    let mut cached_catalog: Option<SourceCatalog> = None;
    for mode in ["cold", "warm"] {
        let started = Instant::now();
        if cached_catalog.is_none() {
            cached_catalog = Some(SourceCatalog::builtin()?);
        }
        let semantic = source_semantic(cached_catalog.as_ref().expect("catalog initialized"));
        list_rows.push(json!({
            "mode": mode,
            "elapsed_seconds": started.elapsed().as_secs_f64(),
            "semantic": semantic,
        }));
    }
    operations.insert("list_sources".into(), Value::Array(list_rows));

    let mut offline_rows = Vec::new();
    for (index, mode) in ["cold", "warm"].into_iter().enumerate() {
        let started = Instant::now();
        let report = offline_engine.discover_seeded(discovery_query_all(), 71 + index as u64).await?;
        let mut semantic = serde_json::to_value(&report)?;
        semantic["command"] = json!("discover");
        offline_rows.push(json!({
            "mode": mode,
            "elapsed_seconds": started.elapsed().as_secs_f64(),
            "semantic": semantic,
        }));
    }
    operations.insert("discover_all".into(), Value::Array(offline_rows));

    let mut refs = Vec::new();
    let mut delayed_rows = Vec::new();
    for (index, mode) in ["cold", "warm"].into_iter().enumerate() {
        phase.store(index, Ordering::SeqCst);
        let started = Instant::now();
        let report = fixture_engine.discover_seeded(fixture_query(), 81 + index as u64).await?;
        let elapsed = started.elapsed().as_secs_f64();
        if index == 1 {
            refs = report.items.iter().filter_map(|item| item.frame.clone()).collect();
        }
        delayed_rows.push(json!({
            "mode": mode,
            "elapsed_seconds": elapsed,
            "semantic": fixture_semantic(&report),
        }));
    }
    operations.insert("four_delayed_sources".into(), Value::Array(delayed_rows));

    phase.store(2, Ordering::SeqCst);
    let started = Instant::now();
    let batch = fetch_many_raw(&fixture_engine, refs, 4, FetchErrorPolicy::Collect, false).await;
    let mut batch_rows = batch
        .items
        .iter()
        .map(|item| {
            let artifacts = item
                .raw
                .as_ref()
                .map(|raw| {
                    raw.public_receipt()
                        .artifacts
                        .into_iter()
                        .map(|artifact| {
                            json!({
                                "name": artifact.name,
                                "size_bytes": artifact.size_bytes,
                                "sha256": artifact.sha256,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({
                "source": item.frame.source,
                "logical_id": item.frame.logical_id,
                "status": match item.status {
                    FetchStatus::Success => "success",
                    FetchStatus::Failed => "failed",
                    FetchStatus::Cancelled => "cancelled",
                    FetchStatus::NotStarted => "not_started",
                    FetchStatus::Planned => "planned",
                },
                "artifacts": artifacts,
            })
        })
        .collect::<Vec<_>>();
    batch_rows.sort_by(|left, right| left["source"].as_str().cmp(&right["source"].as_str()));
    operations.insert(
        "batch_acquisition".into(),
        json!({
            "mode": "warm_engine",
            "elapsed_seconds": started.elapsed().as_secs_f64(),
            "semantic": {"items": batch_rows},
        }),
    );

    Ok(json!({
        "runtime": "native",
        "setup_seconds": setup_seconds,
        "operations": operations,
    }))
}

fn arg(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        if key == name {
            return args.next().ok_or_else(|| format!("missing value for {name}").into());
        }
    }
    Err(format!("missing argument {name}").into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = arg("--server")?;
    let token = arg("--token")?;
    let runtime_root = PathBuf::from(arg("--runtime-root")?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;
    let result = runtime.block_on(run(server, token, runtime_root))?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
