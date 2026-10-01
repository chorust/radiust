use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::download::{DownloadStatus, FetchErrorPolicy};
use radiust_core::engine::{Engine, EngineError};
use radiust_core::identity::{logical_id, normalize_time};
use radiust_core::limits::{Limits, RequestBudget};
use radiust_core::model::{
    ArtifactReceipt, DiscoveryStatus, FrameRef, Query, RawArtifact, RawFrame,
};
use radiust_core::source::catalog::SourceCatalog;
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use radiust_core::transport::ftp::FtpTransport;
use radiust_core::transport::http::{HttpRequestCoalescer, HttpTransport};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

const SOURCE_IDS: [&str; 25] = [
    "au",
    "bmkg",
    "ca",
    "cam",
    "es",
    "fr",
    "id",
    "id_sidarma",
    "kr",
    "my",
    "nz",
    "opensnow",
    "ph",
    "pt",
    "rainviewer",
    "rdcap",
    "sg",
    "th",
    "th_royalrain",
    "tw",
    "tw-http",
    "uk",
    "vn",
    "windy",
    "wunderground",
];

const NATIVE_ADAPTER_IDS: [&str; 25] = [
    "au",
    "bmkg",
    "ca",
    "cam",
    "es",
    "fr",
    "id",
    "id_sidarma",
    "kr",
    "my",
    "nz",
    "opensnow",
    "ph",
    "pt",
    "rainviewer",
    "rdcap",
    "sg",
    "th",
    "th_royalrain",
    "tw",
    "tw-http",
    "uk",
    "vn",
    "windy",
    "wunderground",
];

const DISCOVERY_TARGETS: [(&str, &str, Option<&str>); 74] = [
    ("au", "composite", None),
    ("bmkg", "composite", None),
    ("ca", "rain", None),
    ("cam", "composite", None),
    ("es", "composite", None),
    ("fr", "composite", None),
    ("id", "composite", None),
    ("id_sidarma", "cmax", None),
    ("kr", "composite", None),
    ("my", "composite", Some("east")),
    ("my", "composite", Some("peninsular")),
    ("nz", "rain", None),
    ("opensnow", "composite", None),
    ("ph", "composite", None),
    ("pt", "composite", None),
    ("rainviewer", "composite", None),
    ("rdcap", "reflectivity", Some("JPN/AKIT")),
    ("rdcap", "reflectivity", Some("JPN/FUNC")),
    ("rdcap", "reflectivity", Some("JPN/HAIG")),
    ("rdcap", "reflectivity", Some("JPN/HAKO")),
    ("rdcap", "reflectivity", Some("JPN/ISHI")),
    ("rdcap", "reflectivity", Some("JPN/ITOK")),
    ("rdcap", "reflectivity", Some("JPN/KASH")),
    ("rdcap", "reflectivity", Some("JPN/KURU")),
    ("rdcap", "reflectivity", Some("JPN/KUSH")),
    ("rdcap", "reflectivity", Some("JPN/MAKI")),
    ("rdcap", "reflectivity", Some("JPN/MISA")),
    ("rdcap", "reflectivity", Some("JPN/MURO")),
    ("rdcap", "reflectivity", Some("JPN/NAGO")),
    ("rdcap", "reflectivity", Some("JPN/SAPP")),
    ("rdcap", "reflectivity", Some("JPN/SEFU")),
    ("rdcap", "reflectivity", Some("JPN/SEND")),
    ("rdcap", "reflectivity", Some("JPN/TAKA")),
    ("rdcap", "reflectivity", Some("JPN/TANE")),
    ("rdcap", "reflectivity", Some("JPN/TOJI")),
    ("rdcap", "reflectivity", Some("JPN/YAHI")),
    ("rdcap", "reflectivity", Some("PHL/APAR")),
    ("rdcap", "reflectivity", Some("PHL/BAGU")),
    ("rdcap", "reflectivity", Some("PHL/BALE")),
    ("rdcap", "reflectivity", Some("PHL/BASC")),
    ("rdcap", "reflectivity", Some("PHL/BOHO")),
    ("rdcap", "reflectivity", Some("PHL/DAET")),
    ("rdcap", "reflectivity", Some("PHL/GUIU")),
    ("rdcap", "reflectivity", Some("PHL/HINA")),
    ("rdcap", "reflectivity", Some("PHL/ILOI")),
    ("rdcap", "reflectivity", Some("PHL/MACT")),
    ("rdcap", "reflectivity", Some("PHL/QUEZ")),
    ("rdcap", "reflectivity", Some("PHL/SUBI")),
    ("rdcap", "reflectivity", Some("PHL/TAGA")),
    ("rdcap", "reflectivity", Some("PHL/TAMP")),
    ("rdcap", "reflectivity", Some("PHL/VIRA")),
    ("rdcap", "reflectivity", Some("TWN/RCAA")),
    ("rdcap", "reflectivity", Some("TWN/RCCG")),
    ("rdcap", "reflectivity", Some("TWN/RCCK")),
    ("rdcap", "reflectivity", Some("TWN/RCCU")),
    ("rdcap", "reflectivity", Some("TWN/RCGI")),
    ("rdcap", "reflectivity", Some("TWN/RCHL")),
    ("rdcap", "reflectivity", Some("TWN/RCKT")),
    ("rdcap", "reflectivity", Some("TWN/RCLY")),
    ("rdcap", "reflectivity", Some("TWN/RCMD")),
    ("rdcap", "reflectivity", Some("TWN/RCMK")),
    ("rdcap", "reflectivity", Some("TWN/RCNT")),
    ("rdcap", "reflectivity", Some("TWN/RCSL")),
    ("rdcap", "reflectivity", Some("TWN/RCWF")),
    ("sg", "composite", None),
    ("th", "composite", None),
    ("th_royalrain", "cappi", None),
    ("tw", "grid", None),
    ("tw", "observation", None),
    ("tw-http", "observation", None),
    ("uk", "rain", None),
    ("vn", "cmax", None),
    ("windy", "reflectivity", None),
    ("wunderground", "composite", None),
];

const RAW_HOOK_SOURCES: [&str; 7] = ["au", "ph", "rainviewer", "rdcap", "th", "tw", "windy"];
const CREDENTIALS: [(&str, &str); 3] =
    [("id", "token"), ("id_sidarma", "api_key"), ("wunderground", "api_key")];

fn offline_engine(config: CoreConfig) -> Engine {
    assert!(!config.runtime.allow_network, "source matrix tests must stay offline");
    Engine::new(config, SourceRegistry::default()).expect("offline engine initializes")
}

fn raw_context() -> SourceContext {
    let limits = Limits::default();
    let budget = Arc::new(RequestBudget::new(&limits));
    SourceContext {
        query: Query::default(),
        allow_network: false,
        discovery_workers: 1,
        source_options: Arc::new(BTreeMap::new()),
        request_budget: budget.clone(),
        limits: limits.clone(),
        http_transport: Arc::new(
            HttpTransport::with_budget(limits.clone(), false, budget.clone())
                .expect("local HTTP transport initializes"),
        ),
        ftp_transport: Arc::new(FtpTransport::with_budget(limits, false, budget)),
        request_coalescer: Arc::new(HttpRequestCoalescer::default()),
    }
}

fn empty_frame(source: &str, product: &str) -> FrameRef {
    FrameRef {
        source: source.into(),
        product: product.into(),
        station: None,
        valid_time: "2026-09-18T02:00:00.000000Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: None,
        locator_version: "matrix-v1".into(),
        locator: json!({}),
    }
}

fn fixture_artifact(name: &str, media_type: &str, bytes: &[u8]) -> RawArtifact {
    let mut staged = tempfile::NamedTempFile::new().expect("fixture staging file");
    staged.write_all(bytes).expect("write fixture bytes");
    RawArtifact {
        receipt: ArtifactReceipt {
            name: name.into(),
            media_type: media_type.into(),
            size_bytes: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        },
        path: staged.into_temp_path(),
    }
}

fn rainviewer_fixture() -> RawFrame {
    let mut frame = FrameRef {
        source: "rainviewer".into(),
        product: "composite".into(),
        station: None,
        valid_time: "2026-09-18T02:00:00.000000Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some("7cc4a10f8d53".into()),
        locator_version: "rainviewer-v2".into(),
        locator: json!({
            "api_url": "https://api.rainviewer.com/public/weather-maps.json",
            "host": "https://tilecache.rainviewer.com",
            "path": "/v2/radar/7cc4a10f8d53",
            "tile_size": 512,
            "zoom": 1,
            "color": 2,
            "options": "0_0",
        }),
    };
    frame.logical_id = logical_id(&frame).expect("RainViewer fixture identity");
    let artifacts = vec![
        fixture_artifact(
            "tile-z1-x0-y0.png",
            "image/png",
            include_bytes!("../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y0.png"),
        ),
        fixture_artifact(
            "tile-z1-x1-y0.png",
            "image/png",
            include_bytes!("../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x1-y0.png"),
        ),
        fixture_artifact(
            "tile-z1-x0-y1.png",
            "image/png",
            include_bytes!("../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y1.png"),
        ),
        fixture_artifact(
            "tile-z1-x1-y1.png",
            "image/png",
            include_bytes!("../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x1-y1.png"),
        ),
    ];
    RawFrame { frame, artifacts, private_locator: None }
}

fn tw_grid_fixture() -> RawFrame {
    let bytes = include_bytes!("../../../tests/fixtures/sources/tw/raw/O-A0059-001.json");
    let revision = "O-A0059-001-1789878600";
    let mut frame = FrameRef {
        source: "tw".into(),
        product: "grid".into(),
        station: Some("CV1_3600".into()),
        valid_time: "2026-09-20T04:30:00.000000Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some(revision.into()),
        locator_version: "tw-cwa-v2".into(),
        locator: json!({
            "url": "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json",
            "name": "O-A0059-001.json",
            "media_type": "application/json",
            "artifacts": [],
            "station": "CV1_3600",
            "revision": revision,
            "time_semantics": "provider_grid_time",
            "geometry_status": "provider_native_twd67",
            "native_crs": "EPSG:3821",
            "grid_dimension": [881, 921],
            "grid_origin": [115.0, 18.0],
            "grid_resolution": 0.0125,
        }),
    };
    frame.logical_id = logical_id(&frame).expect("TW grid fixture identity");
    RawFrame {
        frame,
        artifacts: vec![fixture_artifact("O-A0059-001.json", "application/json", bytes)],
        private_locator: None,
    }
}

#[test]
fn catalog_lists_all_sources_and_expands_the_exact_discovery_target_matrix() {
    let catalog = SourceCatalog::builtin().expect("built-in source catalog");
    let source_ids = catalog.sources.iter().map(|source| source.id.as_str()).collect::<Vec<_>>();
    assert_eq!(source_ids, SOURCE_IDS);

    let targets = catalog.expand_targets(None).expect("expand all catalog targets");
    let actual = targets
        .iter()
        .map(|target| {
            (target.source.as_str(), target.product.as_deref().unwrap(), target.station.as_deref())
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, DISCOVERY_TARGETS);
    assert_eq!(targets.len(), 74);
}

#[test]
fn native_registry_matches_all_catalog_sources() {
    let registry = SourceRegistry::with_builtins();
    let registered = SOURCE_IDS
        .iter()
        .copied()
        .filter(|source| registry.get(source).is_some())
        .collect::<Vec<_>>();
    let missing = SOURCE_IDS
        .iter()
        .copied()
        .filter(|source| registry.get(source).is_none())
        .collect::<Vec<_>>();

    assert_eq!(registered, NATIVE_ADAPTER_IDS);
    assert!(missing.is_empty());
    for source in NATIVE_ADAPTER_IDS {
        assert_eq!(registry.get(source).unwrap().source_id(), source);
    }
}

#[test]
fn historical_sources_keep_fixture_provenance_but_remain_fail_closed() {
    let catalog = SourceCatalog::builtin().expect("built-in source catalog");
    let registry = SourceRegistry::with_builtins();
    let inventory: Value = serde_json::from_str(include_str!("../../../migration/inventory.json"))
        .expect("source inventory is valid JSON");
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let historical = inventory["historical"].as_array().expect("historical source list");
    assert_eq!(historical.len(), 2);

    for entry in historical {
        let source_id = entry["id"].as_str().expect("historical source ID");
        assert!(catalog.source(source_id).is_none(), "{source_id} must stay outside the catalog");
        assert!(registry.get(source_id).is_none(), "{source_id} must have no native adapter");
        assert!(matches!(
            catalog.expand_targets(Some(&[source_id.to_owned()])),
            Err(radiust_core::source::catalog::CatalogError::UnknownSource(id)) if id == source_id
        ));

        let history_path = repo_root.join(entry["evidence"].as_str().expect("history record path"));
        let history: Value = serde_json::from_slice(
            &fs::read(&history_path).expect("read historical source provenance"),
        )
        .expect("historical provenance is valid JSON");
        let fixture_path = repo_root.join(entry["fixture"].as_str().expect("fixture path"));
        let fixture: Value = serde_json::from_slice(
            &fs::read(&fixture_path).expect("read historical source fixture"),
        )
        .expect("historical source fixture is valid JSON");
        assert_eq!(history["source"], source_id);
        assert_eq!(fixture["source"], source_id);
        assert_eq!(history["fixture"], entry["fixture"]);
        assert_eq!(history["migration"], entry["migration"]);
        assert!(history["adapter"].as_str().is_some());
        assert!(history["test"].as_str().is_some());
        assert!(repo_root.join(history["migration"].as_str().unwrap()).is_file());
        assert!(repo_root.join(history["resource"].as_str().unwrap()).is_file());
        assert!(repo_root.join(history["test"].as_str().unwrap()).is_file());
        assert!(
            !repo_root.join(history["adapter"].as_str().unwrap()).exists(),
            "historical provenance must not keep a Python source adapter implementation"
        );

        let frames = fixture["frames"].as_array().expect("historical fixture frames");
        assert!(!frames.is_empty(), "{source_id} retains provider evidence");
        for frame in frames {
            for artifact in frame["artifacts"].as_array().expect("fixture artifacts") {
                let path = fixture_path
                    .parent()
                    .unwrap()
                    .join(artifact["path"].as_str().expect("artifact path"));
                let bytes = fs::read(path).expect("read retained historical raw bytes");
                assert_eq!(bytes.len() as u64, artifact["size_bytes"].as_u64().unwrap());
                assert_eq!(
                    hex::encode(Sha256::digest(bytes)),
                    artifact["sha256"].as_str().unwrap(),
                    "{source_id} historical raw digest"
                );
            }
            assert!(
                frame["metadata"]["scientific_reference_status"]
                    .as_str()
                    .is_some_and(|status| status.starts_with("blocked:"))
            );
        }
    }

    for entry in inventory["sources"].as_array().expect("current source list") {
        let fixture_path = repo_root.join(entry["fixture"].as_str().expect("fixture path"));
        let fixture: Value =
            serde_json::from_slice(&fs::read(fixture_path).expect("read current source fixture"))
                .expect("current source fixture is valid JSON");
        if fixture["status"] == "blocked" {
            assert!(fixture["frames"].as_array().unwrap().is_empty());
            assert!(!fixture["blockers"].as_array().unwrap().is_empty());
            assert!(!fixture["evidence"].as_array().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn raw_acquisition_hooks_and_scientific_decoders_match_the_implemented_capabilities() {
    let registry = SourceRegistry::with_builtins();
    let context = raw_context();
    let frame = empty_frame("capability-probe", "composite");
    let raw_hooks = SOURCE_IDS
        .iter()
        .filter_map(|source| {
            registry.get(source).and_then(|adapter| {
                adapter
                    .fetch_raw(frame.clone(), context.clone(), PathBuf::new())
                    .is_some()
                    .then_some(*source)
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(raw_hooks, RAW_HOOK_SOURCES);

    let engine = offline_engine(CoreConfig::default());
    let supported =
        BTreeSet::from([("rainviewer", "composite"), ("rdcap", "reflectivity"), ("tw", "grid")]);
    for (source, product, _) in DISCOVERY_TARGETS {
        if !supported.contains(&(source, product)) {
            let raw = RawFrame {
                frame: empty_frame(source, product),
                artifacts: Vec::new(),
                private_locator: None,
            };
            assert!(
                matches!(
                    engine.decode_science(Arc::new(raw)).await,
                    Err(EngineError::UnsupportedScience(_))
                ),
                "{source}/{product} must remain blocked from scientific decoding"
            );
        }
    }
}

#[tokio::test]
async fn validated_science_capabilities_decode_only_the_retained_local_samples() {
    let engine = offline_engine(CoreConfig::default());

    let rainviewer = engine.decode_science(Arc::new(rainviewer_fixture())).await.unwrap();
    assert_eq!(rainviewer.name, "reflectivity");
    assert_eq!(rainviewer.shape, [1024, 1024]);
    assert_eq!(rainviewer.units.as_deref(), Some("dBZ"));

    let tw_grid = engine.decode_science(Arc::new(tw_grid_fixture())).await.unwrap();
    assert_eq!(tw_grid.name, "reflectivity");
    assert_eq!(tw_grid.shape, [881, 921]);
    assert_eq!(tw_grid.grid.crs.as_deref(), Some("EPSG:3821"));
    assert_eq!(tw_grid.units.as_deref(), Some("dBZ"));
}

#[tokio::test]
async fn offline_discovery_preserves_retirement_credentials_and_network_preflight_order() {
    let mut config = CoreConfig::default();
    for (source, field) in CREDENTIALS {
        config.sources.insert(
            source.into(),
            BTreeMap::from([(field.into(), serde_yaml_ng::Value::String(" \t".into()))]),
        );
    }
    let report = offline_engine(config)
        .discover_seeded(Query { source: Some("all".into()), ..Query::default() }, 17)
        .await
        .expect("offline discover all");

    assert_eq!(report.counts.total, 74);
    for item in &report.items {
        let expected = if item.target.source == "uk" {
            DiscoveryStatus::Retired
        } else if CREDENTIALS.iter().any(|(source, _)| *source == item.target.source) {
            DiscoveryStatus::MissingCredentials
        } else {
            DiscoveryStatus::NetworkRestricted
        };
        assert_eq!(item.status, expected, "unexpected preflight for {:?}", item.target);
    }
    assert_eq!(report.counts.retired, 1);
    assert_eq!(report.counts.missing_credentials, 3);
    assert_eq!(report.counts.network_restricted, 70);

    let mut configured = CoreConfig::default();
    for (source, field) in CREDENTIALS {
        configured.sources.insert(
            source.into(),
            BTreeMap::from([(
                field.into(),
                serde_yaml_ng::Value::String(format!("offline-test-{source}-secret")),
            )]),
        );
    }
    let credentialed = offline_engine(configured)
        .discover_seeded(Query { source: Some("all".into()), ..Query::default() }, 17)
        .await
        .expect("credentialed offline discover all");
    assert_eq!(credentialed.counts.retired, 1);
    assert_eq!(credentialed.counts.network_restricted, 73);
    assert_eq!(credentialed.counts.missing_credentials, 0);
    let serialized = serde_json::to_string(&credentialed).unwrap();
    for (source, _) in CREDENTIALS {
        assert!(!serialized.contains(&format!("offline-test-{source}-secret")));
    }
}

#[tokio::test]
async fn raw_acquisition_preflight_is_offline_for_every_catalog_target() {
    let engine = offline_engine(CoreConfig::default());
    for (source, product, _) in DISCOVERY_TARGETS {
        let error = engine.fetch_raw(empty_frame(source, product)).await.unwrap_err();
        match source {
            "uk" => assert!(matches!(error, EngineError::InvalidFrame)),
            _ => assert!(matches!(
                error,
                EngineError::Core(radiust_core::errors::CoreError::NetworkDisabled(_))
            )),
        }
    }
}

#[tokio::test]
async fn retired_source_rejects_raw_acquisition_before_network_preflight() {
    let engine = offline_engine(CoreConfig::default());
    let retired = empty_frame("uk", "rain");
    let error = engine.fetch_raw(retired).await.unwrap_err();
    assert!(matches!(error, EngineError::InvalidFrame));
}

#[derive(Clone)]
struct MatrixFixtureArtifact {
    name: String,
    media_type: String,
    bytes: Vec<u8>,
}

struct MatrixFixtureAdapter {
    source: &'static str,
    expected_frame: FrameRef,
    artifacts: Vec<MatrixFixtureArtifact>,
}

struct SingleSourceE2eAdapter {
    frame: FrameRef,
    artifacts: Vec<MatrixFixtureArtifact>,
}

impl SourceAdapter for SingleSourceE2eAdapter {
    fn source_id(&self) -> &'static str {
        "rainviewer"
    }

    fn discover(
        self: Arc<Self>,
        target: radiust_core::model::DiscoveryTarget,
        _context: SourceContext,
    ) -> BoxFuture<'static, radiust_core::errors::CoreResult<Vec<FrameRef>>> {
        let frame = self.frame.clone();
        Box::pin(async move {
            if target.source == frame.source
                && target.product.as_deref().is_none_or(|product| product == frame.product)
                && target
                    .station
                    .as_deref()
                    .is_none_or(|station| frame.station.as_deref() == Some(station))
            {
                Ok(vec![frame])
            } else {
                Ok(Vec::new())
            }
        })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        _context: SourceContext,
        _temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, radiust_core::errors::CoreResult<RawFrame>>> {
        let expected_frame = self.frame.clone();
        let artifacts = self.artifacts.clone();
        Some(Box::pin(async move {
            if frame != expected_frame {
                return Err(radiust_core::errors::CoreError::Temporary(
                    "single-source fixture received a different frame identity".into(),
                ));
            }
            let artifacts = artifacts
                .iter()
                .map(|artifact| {
                    fixture_artifact(&artifact.name, &artifact.media_type, &artifact.bytes)
                })
                .collect();
            Ok(RawFrame { frame, artifacts, private_locator: None })
        }))
    }
}

impl SourceAdapter for MatrixFixtureAdapter {
    fn source_id(&self) -> &'static str {
        self.source
    }

    fn discover(
        self: Arc<Self>,
        _target: radiust_core::model::DiscoveryTarget,
        _context: SourceContext,
    ) -> BoxFuture<'static, radiust_core::errors::CoreResult<Vec<FrameRef>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        _context: SourceContext,
        _temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, radiust_core::errors::CoreResult<RawFrame>>> {
        let expected_frame = self.expected_frame.clone();
        let artifacts = self.artifacts.clone();
        Some(Box::pin(async move {
            if frame != expected_frame {
                return Err(radiust_core::errors::CoreError::Temporary(
                    "fixture adapter received a different frame identity".into(),
                ));
            }
            let artifacts = artifacts
                .iter()
                .map(|artifact| {
                    fixture_artifact(&artifact.name, &artifact.media_type, &artifact.bytes)
                })
                .collect();
            Ok(RawFrame { frame, artifacts, private_locator: None })
        }))
    }
}

#[tokio::test]
async fn native_engine_fixture_adapters_preserve_hashed_raw_contracts_with_local_bytes() {
    let inventory: Value = serde_json::from_str(include_str!("../../../migration/inventory.json"))
        .expect("source inventory is valid JSON");
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut exercised_frames = 0;

    for source_entry in inventory["sources"].as_array().expect("source inventory list") {
        let fixture_path =
            repo_root.join(source_entry["fixture"].as_str().expect("fixture path in inventory"));
        let fixture: Value =
            serde_json::from_slice(&fs::read(&fixture_path).expect("read source fixture manifest"))
                .expect("source fixture is valid JSON");
        if fixture["status"] == "blocked" {
            continue;
        }

        let source_id = source_entry["id"].as_str().expect("source id in inventory");
        assert_eq!(fixture["source"], source_id);
        let source = SOURCE_IDS
            .iter()
            .copied()
            .find(|candidate| *candidate == source_id)
            .expect("fixture source is in the native catalog");
        let frames = fixture["frames"].as_array().expect("source fixture frames");
        assert!(!frames.is_empty(), "{source_id} fixture must contain a frame");

        for fixture_frame in frames {
            let mut frame = FrameRef {
                source: source_id.into(),
                product: fixture_frame["product"].as_str().expect("fixture product").into(),
                station: fixture_frame["station"].as_str().map(str::to_owned),
                valid_time: fixture_frame["valid_time"]
                    .as_str()
                    .expect("fixture valid time")
                    .into(),
                base_time: fixture_frame["base_time"].as_str().map(str::to_owned),
                logical_id: String::new(),
                revision: fixture_frame["revision"].as_str().map(str::to_owned),
                locator_version: fixture_frame["locator_version"]
                    .as_str()
                    .expect("fixture locator version")
                    .into(),
                locator: fixture_frame
                    .get("locator")
                    .cloned()
                    .unwrap_or_else(|| json!({"uri": fixture_frame["uri"]})),
            };
            frame.logical_id = logical_id(&frame).expect("fixture frame identity");
            frame.validate_identity().expect("fixture frame identity validates");

            let fixture_root = fixture_path.parent().expect("fixture directory");
            let expected_artifacts =
                fixture_frame["artifacts"].as_array().expect("fixture artifacts");
            assert!(!expected_artifacts.is_empty(), "{source_id} frame must contain raw artifacts");
            let artifacts = expected_artifacts
                .iter()
                .map(|artifact| {
                    let bytes = fs::read(
                        fixture_root.join(artifact["path"].as_str().expect("artifact path")),
                    )
                    .expect("read retained source artifact");
                    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
                    assert_eq!(
                        actual_sha256,
                        artifact["sha256"].as_str().expect("fixture artifact digest"),
                        "{source_id}/{} fixture artifact digest",
                        artifact["name"].as_str().unwrap_or("unknown")
                    );
                    MatrixFixtureArtifact {
                        name: artifact["name"].as_str().expect("artifact name").into(),
                        media_type: artifact["media_type"]
                            .as_str()
                            .expect("artifact media type")
                            .into(),
                        bytes,
                    }
                })
                .collect::<Vec<_>>();

            let root = tempfile::tempdir().expect("fixture output root");
            let mut config = CoreConfig::default();
            // Engine gates raw fetch hooks behind this option; this adapter serves only local bytes.
            config.runtime.allow_network = true;
            config.runtime.temp_root = Some(root.path().join("temp"));
            config.cache.enabled = false;
            config.cache.dir = root.path().join("cache");
            config.storage.output = root.path().join("output");
            let mut overrides = SourceRegistry::default();
            overrides
                .register(Arc::new(MatrixFixtureAdapter {
                    source,
                    expected_frame: frame.clone(),
                    artifacts: artifacts.clone(),
                }))
                .expect("register fixture adapter override");
            let engine = Engine::new(config, overrides).expect("offline fixture Engine");
            let report = engine
                .download_raw_only(vec![frame.clone()], FetchErrorPolicy::Collect, false, false)
                .await;

            assert_eq!(report.written, 1, "{source_id}/{}", frame.product);
            assert_eq!(report.failed, 0, "{source_id}/{}", frame.product);
            assert_eq!(report.items.len(), 1);
            assert_eq!(report.items[0].status, DownloadStatus::Written);
            assert_eq!(report.items[0].frame, frame);

            let output_root = root.path().join("output");
            let frame_root = output_root.join(format!("frames/{}", frame.logical_id));
            let manifest_path = frame_root.join("raw-manifest.json");
            let expected_uri = fs::canonicalize(&manifest_path)
                .expect("committed raw manifest path")
                .display()
                .to_string();
            assert_eq!(report.items[0].output_uri.as_deref(), Some(expected_uri.as_str()));
            let manifest: Value =
                serde_json::from_slice(&fs::read(&manifest_path).expect("committed raw manifest"))
                    .expect("raw manifest is valid JSON");
            assert_eq!(manifest["ref"]["logical_id"], frame.logical_id);
            assert_eq!(manifest["ref"]["source"], source_id);
            assert_eq!(manifest["ref"]["product"], frame.product);
            assert_eq!(
                manifest["ref"]["valid_time"],
                normalize_time(&frame.valid_time).expect("normalized fixture time")
            );
            assert_eq!(manifest["raw_complete"], true);

            let manifest_artifacts =
                manifest["artifacts"].as_array().expect("raw manifest artifacts");
            assert_eq!(manifest_artifacts.len(), artifacts.len());
            for expected in &artifacts {
                let saved = manifest_artifacts
                    .iter()
                    .find(|saved| saved["name"] == expected.name)
                    .expect("raw manifest retains every artifact");
                let expected_sha256 = hex::encode(Sha256::digest(&expected.bytes));
                assert_eq!(saved["media_type"], expected.media_type);
                assert_eq!(saved["size_bytes"], expected.bytes.len() as u64);
                assert_eq!(saved["sha256"], expected_sha256);
                assert_eq!(
                    fs::read(frame_root.join("raw").join(&expected.name))
                        .expect("committed raw fixture bytes"),
                    expected.bytes
                );
            }
            exercised_frames += 1;
        }
    }

    assert!(exercised_frames > 0);
}

#[tokio::test]
async fn native_single_source_discover_acquire_decode_and_commit_runs_without_python_or_provider_io()
 {
    let raw = rainviewer_fixture();
    let frame = raw.frame.clone();
    let artifacts = raw
        .artifacts
        .iter()
        .map(|artifact| MatrixFixtureArtifact {
            name: artifact.receipt.name.clone(),
            media_type: artifact.receipt.media_type.clone(),
            bytes: fs::read(&artifact.path).expect("read retained RainViewer tile fixture"),
        })
        .collect::<Vec<_>>();

    let root = tempfile::tempdir().expect("isolated single-source workspace");
    let mut config = CoreConfig::default();
    // Engine gates acquisition behind this option; the adapter below serves only embedded local bytes.
    config.runtime.allow_network = true;
    config.runtime.temp_root = Some(root.path().join("temp"));
    config.cache.enabled = false;
    config.cache.dir = root.path().join("cache");
    config.storage.output = root.path().join("output");
    let mut overrides = SourceRegistry::default();
    overrides
        .register(Arc::new(SingleSourceE2eAdapter { frame: frame.clone(), artifacts }))
        .expect("register local fixture adapter");
    let engine = Engine::new(config, overrides).expect("initialize native Engine");

    let discovery = engine
        .discover_seeded(
            Query {
                source: Some("rainviewer".into()),
                product: Some("composite".into()),
                ..Query::default()
            },
            0x5241_4449,
        )
        .await
        .expect("discover fixture frame through native Engine");
    assert_eq!(discovery.counts.total, 1);
    assert_eq!(discovery.counts.success, 1);
    assert_eq!(discovery.items[0].frame.as_ref(), Some(&frame));

    let downloaded = engine
        .download_netcdf_to(
            vec![discovery.items[0].frame.clone().expect("discovery frame")],
            FetchErrorPolicy::Collect,
            false,
            false,
            root.path().join("output"),
        )
        .await;
    assert_eq!(downloaded.written, 1);
    assert_eq!(downloaded.failed, 0);
    assert_eq!(downloaded.items[0].frame, frame);

    let output = PathBuf::from(downloaded.items[0].output_uri.as_ref().expect("NetCDF output URI"));
    assert!(output.is_file());
    let manifest_path = output.with_file_name("decoded.nc.manifest.json");
    let manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read committed output manifest"))
            .expect("committed output manifest is valid JSON");
    assert_eq!(manifest["logical_id"], frame.logical_id);
    assert_eq!(manifest["processing_spec"]["format"], "netcdf");
    let artifacts = manifest["artifacts"].as_array().expect("manifest artifacts");
    assert!(!artifacts.is_empty());
    for artifact in artifacts {
        let path = root
            .path()
            .join("output")
            .join(artifact["relative_uri"].as_str().expect("artifact relative path"));
        let bytes = fs::read(path).expect("read committed artifact");
        assert_eq!(artifact["size_bytes"].as_u64(), Some(bytes.len() as u64));
        assert_eq!(artifact["sha256"].as_str(), Some(hex::encode(Sha256::digest(&bytes)).as_str()));
    }
    let parsed_manifest = radiust_core::storage::manifest::read_manifest(&manifest_path)
        .expect("read manifest through storage contract")
        .expect("committed manifest exists");
    assert!(radiust_core::storage::manifest::is_complete(
        &root.path().join("output"),
        &parsed_manifest
    ));
    let decoded = radiust_core::output::netcdf::read_selected_field(
        &output,
        "reflectivity",
        None,
        &Limits::default(),
    )
    .expect("independently read the committed scientific field");
    assert_eq!(decoded.shape, [1024, 1024]);
    assert_eq!(decoded.units.as_deref(), Some("dBZ"));
}
