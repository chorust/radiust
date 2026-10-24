use super::super::*;
use crate::model::GeographicPoint;
use crate::source::{SourceAdapter, SourceContext, SourceRegistry};
use futures_util::future::BoxFuture;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy)]
enum DiscoveryMode {
    Frame,
    OldFrame,
    Empty,
    Error,
    Delay(u64, bool),
    RefreshDelay(u64),
    InvalidDirectory,
}

#[derive(Clone, Copy)]
enum FetchMode {
    Fixture,
    AllMissing,
    Error,
    Cancelled,
    ResourceLimit,
}

struct ControlledAdapter {
    source: &'static str,
    discovery: DiscoveryMode,
    fetch: FetchMode,
    discoveries: Arc<AtomicUsize>,
    fetches: Arc<AtomicUsize>,
    catalog_refreshes: Arc<AtomicUsize>,
}

impl SourceAdapter for ControlledAdapter {
    fn source_id(&self) -> &'static str {
        self.source
    }

    fn refresh_station_catalog(
        self: Arc<Self>,
        _context: SourceContext,
    ) -> BoxFuture<
        'static,
        crate::errors::CoreResult<Option<crate::source::catalog::StationCatalogUpdate>>,
    > {
        if self.source != "rdcap" {
            return Box::pin(async { Ok(None) });
        }
        self.catalog_refreshes.fetch_add(1, Ordering::SeqCst);
        let discovery = self.discovery;
        Box::pin(async move {
            if let DiscoveryMode::Delay(milliseconds, _)
            | DiscoveryMode::RefreshDelay(milliseconds) = discovery
            {
                tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
            }
            if matches!(discovery, DiscoveryMode::InvalidDirectory) {
                return Ok(Some(crate::source::catalog::StationCatalogUpdate {
                    source_id: "rdcap".into(),
                    stations: vec![],
                    metadata: None,
                }));
            }
            Ok(None)
        })
    }

    fn discover(
        self: Arc<Self>,
        _target: DiscoveryTarget,
        _context: SourceContext,
    ) -> BoxFuture<'static, crate::errors::CoreResult<Vec<FrameRef>>> {
        self.discoveries.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            match self.discovery {
                DiscoveryMode::OldFrame if self.source == "rainviewer" => {
                    let mut frame = rainviewer_frame();
                    frame.valid_time = "2020-01-01T00:00:00.000000Z".into();
                    Ok(vec![frame])
                }
                DiscoveryMode::OldFrame => Ok(Vec::new()),
                DiscoveryMode::Frame if self.source == "rainviewer" => Ok(vec![rainviewer_frame()]),
                DiscoveryMode::Frame if self.source == "pt" => Ok(vec![pt_frame()]),
                DiscoveryMode::Frame => Ok(Vec::new()),
                DiscoveryMode::Empty
                | DiscoveryMode::RefreshDelay(_)
                | DiscoveryMode::InvalidDirectory => Ok(Vec::new()),
                DiscoveryMode::Error => Err(CoreError::Provider(ProviderError::CatalogUnavailable)),
                DiscoveryMode::Delay(milliseconds, with_frame) => {
                    tokio::time::sleep(std::time::Duration::from_millis(milliseconds)).await;
                    if with_frame && self.source == "rainviewer" {
                        Ok(vec![rainviewer_frame()])
                    } else if with_frame && self.source == "pt" {
                        Ok(vec![pt_frame()])
                    } else {
                        Ok(Vec::new())
                    }
                }
            }
        })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        _context: SourceContext,
        temp_root: std::path::PathBuf,
    ) -> Option<BoxFuture<'static, crate::errors::CoreResult<RawFrame>>> {
        if !matches!(self.source, "rainviewer" | "pt") {
            return None;
        }
        let source = self.source;
        let fetches = self.fetches.clone();
        let mode = self.fetch;
        Some(Box::pin(async move {
            fetches.fetch_add(1, Ordering::SeqCst);
            match mode {
                FetchMode::Error => Err(CoreError::Transport("controlled fetch failure".into())),
                FetchMode::Cancelled => Err(CoreError::Cancelled),
                FetchMode::ResourceLimit => {
                    Err(CoreError::ResourceLimit("controlled acquisition limit".into()))
                }
                FetchMode::Fixture | FetchMode::AllMissing => {
                    std::fs::create_dir_all(&temp_root)
                        .map_err(|_| CoreError::Temporary("test temp directory failed".into()))?;
                    if source == "pt" {
                        let bytes = include_bytes!(
                            "../../../../../tests/fixtures/sources/pt/raw/PTST2.png"
                        );
                        let mut file = tempfile::Builder::new()
                            .prefix("local-view-pt-")
                            .tempfile_in(&temp_root)
                            .map_err(|_| {
                                CoreError::Temporary("test PT image staging failed".into())
                            })?;
                        file.write_all(bytes).and_then(|()| file.as_file().sync_all()).map_err(
                            |_| CoreError::Temporary("test PT image staging failed".into()),
                        )?;
                        let artifact = RawArtifact {
                            receipt: ArtifactReceipt {
                                name: "PTST2.png".into(),
                                media_type: "image/png".into(),
                                size_bytes: bytes.len() as u64,
                                sha256: hex::encode(Sha256::digest(bytes)),
                            },
                            path: file.into_temp_path(),
                        };
                        return Ok(RawFrame {
                            frame,
                            artifacts: vec![artifact],
                            private_locator: None,
                        });
                    }
                    let missing = (matches!(mode, FetchMode::AllMissing))
                        .then(transparent_png)
                        .transpose()?;
                    let tile_data: [&[u8]; 4] = if let Some(ref bytes) = missing {
                        [bytes, bytes, bytes, bytes]
                    } else {
                        [
                            include_bytes!(
                                "../../../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y0.png"
                            ),
                            include_bytes!(
                                "../../../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x1-y0.png"
                            ),
                            include_bytes!(
                                "../../../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y1.png"
                            ),
                            include_bytes!(
                                "../../../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x1-y1.png"
                            ),
                        ]
                    };
                    let names = [
                        "tile-z1-x0-y0.png",
                        "tile-z1-x1-y0.png",
                        "tile-z1-x0-y1.png",
                        "tile-z1-x1-y1.png",
                    ];
                    let mut artifacts = Vec::with_capacity(4);
                    for (name, bytes) in names.into_iter().zip(tile_data) {
                        let mut file = tempfile::Builder::new()
                            .prefix("local-view-tile-")
                            .tempfile_in(&temp_root)
                            .map_err(|_| CoreError::Temporary("test tile staging failed".into()))?;
                        file.write_all(bytes)
                            .and_then(|()| file.as_file().sync_all())
                            .map_err(|_| CoreError::Temporary("test tile staging failed".into()))?;
                        artifacts.push(RawArtifact {
                            receipt: ArtifactReceipt {
                                name: name.to_owned(),
                                media_type: "image/png".into(),
                                size_bytes: bytes.len() as u64,
                                sha256: hex::encode(Sha256::digest(bytes)),
                            },
                            path: file.into_temp_path(),
                        });
                    }
                    Ok(RawFrame { frame, artifacts, private_locator: None })
                }
            }
        }))
    }
}

/// One recent timestamp per test process, so fixtures stay inside the default local-view
/// freshness window while repeated fixture calls still agree.
static FRESH_VALID_TIME: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    (Utc::now() - chrono::Duration::minutes(5)).format("%Y-%m-%dT%H:%M:%S.000000Z").to_string()
});

fn fresh_valid_time() -> String {
    FRESH_VALID_TIME.clone()
}

fn rainviewer_frame() -> FrameRef {
    let mut frame = FrameRef {
        source: "rainviewer".into(),
        product: "composite".into(),
        station: None,
        valid_time: fresh_valid_time(),
        base_time: None,
        logical_id: String::new(),
        revision: Some("7cc4a10f8d53".into()),
        locator_version: "rainviewer-v2".into(),
        locator: serde_json::json!({
            "api_url": "https://api.rainviewer.com/public/weather-maps.json",
            "host": "https://tilecache.rainviewer.com",
            "path": "/v2/radar/7cc4a10f8d53",
            "tile_size": 512,
            "zoom": 1,
            "color": 2,
            "options": "0_0"
        }),
    };
    frame.logical_id = crate::identity::logical_id(&frame).unwrap();
    frame
}

fn pt_frame() -> FrameRef {
    let mut frame = FrameRef {
        source: "pt".into(),
        product: "composite".into(),
        station: Some("PTST2".into()),
        valid_time: fresh_valid_time(),
        base_time: None,
        logical_id: String::new(),
        revision: Some("pcr_pst-2026-09-18T0250".into()),
        locator_version: "pt-legacy-v1".into(),
        locator: serde_json::json!({
            "url": "https://www.ipma.pt/resources.www/transf/radar/mad/pcr_pst-2026-09-18T0250.png",
            "artifacts": [],
            "station": "PTST2",
            "revision": "pcr_pst-2026-09-18T0250",
            "headers": {"Referer": "https://www.ipma.pt/pt/otempo/obs.remote/index-md.jsp"}
        }),
    };
    frame.logical_id = crate::identity::logical_id(&frame).unwrap();
    frame
}

fn transparent_png() -> crate::errors::CoreResult<Vec<u8>> {
    let image = RgbaImage::from_pixel(512, 512, Rgba([0, 0, 0, 0]));
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|_| CoreError::Transport("test PNG encoding failed".into()))?;
    Ok(output.into_inner())
}

fn evidence() -> serde_json::Value {
    serde_json::json!([{
        "source": "controlled test catalog",
        "reference": "unit-test-only",
        "observed_at": "2026-10-10"
    }])
}

fn spatial_catalog(
    include_rdcap: bool,
    automatic: bool,
    rainviewer_coverage: bool,
) -> SourceCatalog {
    let mut sources = Vec::new();
    if include_rdcap {
        sources.push(serde_json::json!({
            "id": "rdcap",
            "availability": "available",
            "products": [{
                "id": "reflectivity",
                "variables": ["reflectivity"],
                "units": {"reflectivity": "dBZ"},
                "native_grid_kind": "geographic",
                "default": true,
                "metadata": {"spatial": {
                    "schema_version": 1,
                    "kind": "product_selector",
                    "source_class": "official",
                    "source_class_evidence": evidence(),
                    "admission": {
                        "status": "verified", "automatic": true,
                        "purposes": ["local_view"], "evidence": evidence()
                    }
                }}
            }],
            "stations": [{
                "id": "TWRCHL", "name": "controlled site",
                "longitude": 0.0, "latitude": 0.0,
                "product_ids": ["reflectivity"],
                "metadata": {"spatial": {
                    "schema_version": 1,
                    "kind": "physical_station",
                    "field_evidence": {
                        "longitude": [{"status": "verified", "value": 0.0}],
                        "latitude": [{"status": "verified", "value": 0.0}]
                    }
                }}
            }]
        }));
    }
    let coverage = if rainviewer_coverage {
        serde_json::json!({
            "kind": "geodesic_circle",
            "status": "verified",
            "geometry": {"longitude": 0.0, "latitude": 0.0, "radius_m": 1000.0},
            "evidence": evidence()
        })
    } else {
        serde_json::json!({"kind": "unknown", "status": "unknown"})
    };
    sources.push(serde_json::json!({
        "id": "rainviewer",
        "availability": "available",
        "products": [{
            "id": "composite",
            "variables": ["reflectivity"],
            "units": {"reflectivity": "dBZ"},
            "native_grid_kind": "geographic",
            "default": true,
            "metadata": {"spatial": {
                "schema_version": 1,
                "kind": "composite",
                "source_class": "third_party",
                "source_class_evidence": evidence(),
                "geometry": {
                    "status": "verified", "crs": "EPSG:4326",
                    "registration": "center", "shape": [1024, 1024],
                    "evidence": evidence()
                },
                "observation_coverage": coverage,
                "admission": {
                    "status": if automatic { "verified" } else { "unverified" },
                    "automatic": automatic,
                    "purposes": ["local_view"],
                    "evidence": if automatic { evidence() } else { serde_json::json!([]) }
                }
            }}
        }]
    }));
    SourceCatalog::parse(&serde_json::json!({"schema_version": 1, "sources": sources}).to_string())
        .unwrap()
}

fn pt_spatial_catalog() -> SourceCatalog {
    SourceCatalog::parse(
        &serde_json::json!({
            "schema_version": 1,
            "sources": [{
                "id": "pt",
                "availability": "available",
                "products": [{
                    "id": "composite",
                    "variables": ["rain_intensity"],
                    "units": {"rain_intensity": "1"},
                    "native_grid_kind": "pixel",
                    "default": true,
                    "metadata": {"spatial": {
                        "schema_version": 1,
                        "kind": "product_selector",
                        "source_class": "official",
                        "source_class_evidence": evidence(),
                        "geometry": {
                            "status": "verified",
                            "crs": "EPSG:3857",
                            "registration": "edge",
                            "shape": [1526, 1500],
                            "affine": [-2211956.1306892154, 517.5146650087266, 0.0,
                                4295191.719145755, 0.0, -517.5334498787525],
                            "data_window": [0, 0, 1500, 1526],
                            "evidence": evidence()
                        },
                        "observation_coverage": {"kind": "unknown", "status": "unknown"},
                        "admission": {"status": "unverified", "automatic": false,
                            "purposes": ["local_view"]},
                        "selector_ids": ["PTST2"]
                    }}
                }]
            }]
        })
        .to_string(),
    )
    .unwrap()
}

fn make_engine(
    catalog: SourceCatalog,
    rainviewer_discovery: DiscoveryMode,
    rainviewer_fetch: FetchMode,
    rdcap_discovery: DiscoveryMode,
) -> (Engine, tempfile::TempDir, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    make_engine_with_limits(
        catalog,
        rainviewer_discovery,
        rainviewer_fetch,
        rdcap_discovery,
        100_000_000,
        10 * 1024 * 1024 * 1024,
    )
}

fn make_engine_with_limits(
    catalog: SourceCatalog,
    rainviewer_discovery: DiscoveryMode,
    rainviewer_fetch: FetchMode,
    rdcap_discovery: DiscoveryMode,
    max_pixels: u64,
    max_temp_bytes: u64,
) -> (Engine, tempfile::TempDir, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let (engine, temp, rainviewer_discoveries, rainviewer_fetches, _) = make_engine_with_runtime(
        catalog,
        rainviewer_discovery,
        rainviewer_fetch,
        rdcap_discovery,
        max_pixels,
        max_temp_bytes,
        300.0,
    );
    (engine, temp, rainviewer_discoveries, rainviewer_fetches)
}

fn make_engine_with_deadline(
    catalog: SourceCatalog,
    rainviewer_discovery: DiscoveryMode,
    rdcap_discovery: DiscoveryMode,
    discovery_deadline: f64,
) -> (Engine, tempfile::TempDir, Arc<AtomicUsize>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    make_engine_with_runtime(
        catalog,
        rainviewer_discovery,
        FetchMode::Fixture,
        rdcap_discovery,
        100_000_000,
        10 * 1024 * 1024 * 1024,
        discovery_deadline,
    )
}

fn make_engine_with_runtime(
    catalog: SourceCatalog,
    rainviewer_discovery: DiscoveryMode,
    rainviewer_fetch: FetchMode,
    rdcap_discovery: DiscoveryMode,
    max_pixels: u64,
    max_temp_bytes: u64,
    discovery_deadline: f64,
) -> (Engine, tempfile::TempDir, Arc<AtomicUsize>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.max_pixels = max_pixels;
    config.runtime.max_temp_bytes = max_temp_bytes;
    config.runtime.discovery_deadline = discovery_deadline;
    config.runtime.temp_root = Some(temp.path().to_path_buf());
    config.cache.enabled = false;
    let rainviewer_discoveries = Arc::new(AtomicUsize::new(0));
    let rainviewer_fetches = Arc::new(AtomicUsize::new(0));
    let rdcap_discoveries = Arc::new(AtomicUsize::new(0));
    let rdcap_refreshes = Arc::new(AtomicUsize::new(0));
    let mut sources = SourceRegistry::default();
    sources
        .register(Arc::new(ControlledAdapter {
            source: "rainviewer",
            discovery: rainviewer_discovery,
            fetch: rainviewer_fetch,
            discoveries: rainviewer_discoveries.clone(),
            fetches: rainviewer_fetches.clone(),
            catalog_refreshes: Arc::new(AtomicUsize::new(0)),
        }))
        .unwrap();
    sources
        .register(Arc::new(ControlledAdapter {
            source: "rdcap",
            discovery: rdcap_discovery,
            fetch: FetchMode::Error,
            discoveries: rdcap_discoveries.clone(),
            fetches: Arc::new(AtomicUsize::new(0)),
            catalog_refreshes: rdcap_refreshes.clone(),
        }))
        .unwrap();
    let mut engine = Engine::new(config, sources).unwrap();
    engine.catalog = catalog;
    (engine, temp, rainviewer_discoveries, rainviewer_fetches, rdcap_refreshes)
}

fn make_pt_engine(cache_enabled: bool) -> (Engine, tempfile::TempDir, Arc<AtomicUsize>) {
    let temp = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.temp_root = Some(temp.path().to_path_buf());
    config.cache.enabled = cache_enabled;
    config.cache.dir = temp.path().join("raw-cache");
    let fetches = Arc::new(AtomicUsize::new(0));
    let mut sources = SourceRegistry::default();
    sources
        .register(Arc::new(ControlledAdapter {
            source: "pt",
            discovery: DiscoveryMode::Frame,
            fetch: FetchMode::Fixture,
            discoveries: Arc::new(AtomicUsize::new(0)),
            fetches: fetches.clone(),
            catalog_refreshes: Arc::new(AtomicUsize::new(0)),
        }))
        .unwrap();
    let mut engine = Engine::new(config, sources).unwrap();
    engine.catalog = pt_spatial_catalog();
    (engine, temp, fetches)
}

fn pt_request(
    mode: LocalViewMode,
    resampling: LocalViewResampling,
    radius_km: f64,
) -> LocalViewRequest {
    LocalViewRequest {
        center: Some(GeographicPoint { longitude: -16.9, latitude: 32.7 }),
        radius_km: Some(radius_km),
        resolution_m: Some(1000.0),
        source: Some("pt".into()),
        mode,
        resampling,
        ..LocalViewRequest::default()
    }
}

fn request(source: Option<&str>, center: GeographicPoint) -> LocalViewRequest {
    LocalViewRequest {
        center: Some(center),
        radius_km: Some(1.0),
        resolution_m: Some(1000.0),
        source: source.map(str::to_owned),
        ..LocalViewRequest::default()
    }
}

#[tokio::test]
async fn local_view_selects_the_first_eligible_candidate_without_fallback() {
    let (engine, _temp, discoveries, fetches) = make_engine(
        spatial_catalog(false, true, true),
        DiscoveryMode::Frame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
    );

    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::Selected);
    assert_eq!(result.selected.as_ref().unwrap().source, "rainviewer");
    assert_eq!(result.selection.len(), 1);
    assert_eq!(result.selection[0].state, LocalViewCandidateState::Selected);
    assert_eq!(discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn local_view_uses_requested_bilinear_resampling_for_native_fields() {
    let (engine, _temp, _, _) = make_engine(
        spatial_catalog(false, true, false),
        DiscoveryMode::Frame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
    );
    let mut request = request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 });
    request.resampling = crate::model::LocalViewResampling::Bilinear;

    let result = engine.local_view(request).await.unwrap();

    assert_eq!(result.status, LocalViewStatus::Selected);
    let LocalViewData::Numeric(dataset) = result.data.as_ref().unwrap() else {
        panic!("native numeric candidate returned a raw image")
    };
    assert!(dataset.fields[0].provenance.iter().any(|item| item.ends_with("resampling=bilinear")));
}

#[tokio::test]
async fn local_view_decoded_and_dbz_modes_return_the_native_numeric_field() {
    for mode in [LocalViewMode::Decoded, LocalViewMode::Dbz] {
        let (engine, _temp, _, _) = make_engine(
            spatial_catalog(false, true, false),
            DiscoveryMode::Frame,
            FetchMode::Fixture,
            DiscoveryMode::Empty,
        );
        let mut request = request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 });
        request.mode = mode;

        let result = engine.local_view(request).await.unwrap();

        assert_eq!(result.status, LocalViewStatus::Selected);
        let LocalViewData::Numeric(dataset) = result.data.unwrap() else {
            panic!("numeric RainViewer product returned a raw image")
        };
        assert_eq!(dataset.fields[0].name, "reflectivity");
        assert_eq!(dataset.fields[0].units.as_deref(), Some("dBZ"));
        assert_eq!(
            result.processing.unwrap().requested_mode,
            match mode {
                LocalViewMode::Decoded => "decoded",
                LocalViewMode::Dbz => "dbz",
                _ => unreachable!(),
            }
        );
    }
}

#[tokio::test]
async fn local_view_falls_back_in_evidence_order_and_preserves_both_attempts() {
    let catalog = spatial_catalog(true, true, false);
    let (engine, _temp, rainviewer_discoveries, rainviewer_fetches) =
        make_engine(catalog, DiscoveryMode::Frame, FetchMode::Fixture, DiscoveryMode::Empty);

    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::Selected);
    assert_eq!(result.selected.as_ref().unwrap().source, "rainviewer");
    assert_eq!(result.selection.len(), 2);
    assert_eq!(result.selection[0].target.source, "rdcap");
    assert_eq!(result.selection[0].discovery_status, Some(DiscoveryStatus::NoData));
    assert_eq!(result.selection[1].state, LocalViewCandidateState::Selected);
    assert!(result.selection[1].reason.starts_with("selected_after_fallback:"));
    assert_eq!(rainviewer_discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(rainviewer_fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn local_view_keeps_partial_current_frame_domain_separate_from_view_mask() {
    let (engine, _temp, _, _) = make_engine(
        spatial_catalog(false, false, false),
        DiscoveryMode::Frame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
    );
    let mut request =
        request(Some("rainviewer"), GeographicPoint { longitude: 0.0, latitude: 84.0 });
    request.radius_km = Some(500.0);
    request.resolution_m = Some(100_000.0);

    let result = engine.local_view(request).await.unwrap();

    assert_eq!(result.status, LocalViewStatus::Selected);
    let grid = result.grid.as_ref().unwrap();
    let domain = grid.source_domain.as_ref().unwrap();
    assert!(grid.in_view.iter().zip(domain).any(|(inside, domain)| *inside && *domain));
    assert!(grid.in_view.iter().zip(domain).any(|(inside, domain)| *inside && !*domain));
}

#[tokio::test]
async fn local_view_keeps_all_missing_frames_and_does_not_fallback() {
    let (engine, _temp, rainviewer_discoveries, rainviewer_fetches) = make_engine(
        spatial_catalog(true, true, true),
        DiscoveryMode::Frame,
        FetchMode::AllMissing,
        DiscoveryMode::Empty,
    );

    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::Selected);
    assert_eq!(result.selected.as_ref().unwrap().source, "rainviewer");
    assert_eq!(rainviewer_discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(rainviewer_fetches.load(Ordering::SeqCst), 1);
    let grid = result.grid.as_ref().unwrap();
    let LocalViewData::Numeric(dataset) = result.data.as_ref().unwrap() else {
        panic!("numeric candidate returned a raw image")
    };
    let field = &dataset.fields[0];
    let center = (grid.grid.shape[0] / 2) * grid.grid.shape[1] + grid.grid.shape[1] / 2;
    assert!(field.values[center].is_nan());
    assert_eq!(field.quality[center] & 1, 1);
    assert!(result.selection.iter().any(|attempt| {
        attempt.target.source == "rdcap"
            && attempt.state == LocalViewCandidateState::NotStarted
            && attempt.reason == "not_needed_after_selection"
    }));
}

#[tokio::test]
async fn local_view_does_not_apply_a_display_threshold_to_low_reflectivity() {
    let (engine, temp, _, _) = make_engine(
        spatial_catalog(false, false, false),
        DiscoveryMode::Frame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
    );
    let raw = Arc::new(ControlledAdapter {
        source: "rainviewer",
        discovery: DiscoveryMode::Frame,
        fetch: FetchMode::Fixture,
        discoveries: Arc::new(AtomicUsize::new(0)),
        fetches: Arc::new(AtomicUsize::new(0)),
        catalog_refreshes: Arc::new(AtomicUsize::new(0)),
    })
    .fetch_raw(
        rainviewer_frame(),
        engine.source_context(
            Query::default(),
            "rainviewer",
            Arc::new(HttpRequestCoalescer::default()),
        ),
        temp.path().to_path_buf(),
    )
    .unwrap()
    .await
    .unwrap();
    let source = crate::science::decode_rainviewer(&raw, &engine.resource_limits()).unwrap();
    let center = GeographicPoint { longitude: source.grid.x[576], latitude: source.grid.y[215] };
    assert_eq!(source.values[215 * 1024 + 576], 10.0);

    let result = engine.local_view(request(Some("rainviewer"), center)).await.unwrap();

    assert_eq!(result.status, LocalViewStatus::Selected);
    let LocalViewData::Numeric(dataset) = result.data.as_ref().unwrap() else {
        panic!("native numeric source returned a raw image")
    };
    let grid = result.grid.as_ref().unwrap();
    let center_index = (grid.grid.shape[0] / 2) * grid.grid.shape[1] + grid.grid.shape[1] / 2;
    assert_eq!(dataset.fields[0].values[center_index], 10.0);
    assert_eq!(dataset.fields[0].quality[center_index], source.quality[215 * 1024 + 576]);
}

#[tokio::test]
async fn local_view_reports_all_failures_and_never_crosses_explicit_source_scope() {
    let (engine, _temp, rainviewer_calls, _) = make_engine(
        spatial_catalog(true, true, false),
        DiscoveryMode::Frame,
        FetchMode::Error,
        DiscoveryMode::Error,
    );
    let failed = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();
    assert_eq!(failed.status, LocalViewStatus::Failed);
    assert_eq!(failed.selection.len(), 2);
    assert_eq!(rainviewer_calls.load(Ordering::SeqCst), 1);

    let (restricted, _temp, rainviewer_discoveries, _) = make_engine(
        spatial_catalog(true, true, false),
        DiscoveryMode::Frame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
    );
    let result = restricted
        .local_view(request(Some("rdcap"), GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();
    assert_eq!(result.status, LocalViewStatus::Failed);
    assert!(result.selection.iter().all(|attempt| attempt.target.source == "rdcap"));
    assert_eq!(rainviewer_discoveries.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_view_requires_center_and_rejects_unverified_admission() {
    let (engine, _temp, rainviewer_discoveries, _) = make_engine(
        spatial_catalog(false, false, false),
        DiscoveryMode::Frame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
    );
    assert!(matches!(
        engine.local_view(LocalViewRequest::default()).await,
        Err(EngineError::InvalidQuery(_))
    ));
    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();
    assert_eq!(result.status, LocalViewStatus::NoCandidate);
    assert!(result.selection.iter().any(|attempt| attempt.reason == "admission_unverified"));
    assert_eq!(rainviewer_discoveries.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_view_grid_limits_fail_before_any_adapter_call() {
    for (max_pixels, max_temp_bytes) in [(1, 1024), (100, 1)] {
        let (engine, _temp, discoveries, fetches) = make_engine_with_limits(
            spatial_catalog(false, true, false),
            DiscoveryMode::Frame,
            FetchMode::Fixture,
            DiscoveryMode::Empty,
            max_pixels,
            max_temp_bytes,
        );

        let result = engine
            .local_view(request(
                Some("rainviewer"),
                GeographicPoint { longitude: 0.0, latitude: 0.0 },
            ))
            .await;

        let Err(error) = result else { panic!("preflight limit unexpectedly passed") };
        assert!(matches!(error, EngineError::Core(CoreError::ResourceLimit(_))));
        assert_eq!(discoveries.load(Ordering::SeqCst), 0);
        assert_eq!(fetches.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn local_view_source_buffer_limit_stops_the_batch_after_fetch() {
    let (engine, _temp, discoveries, fetches) = make_engine_with_limits(
        spatial_catalog(true, true, true),
        DiscoveryMode::Frame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
        100,
        1024,
    );

    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::Failed);
    assert_eq!(discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    assert!(result.selection.iter().any(|attempt| {
        attempt.target.source == "rdcap"
            && attempt.state == LocalViewCandidateState::NotStarted
            && attempt.reason == "resource_limit"
    }));
}

#[tokio::test]
async fn local_view_default_max_age_applies_to_the_builtin_catalog() {
    let catalog = SourceCatalog::builtin().unwrap();
    assert!(
        catalog
            .source("rainviewer")
            .unwrap()
            .products
            .iter()
            .all(|product| product.suggested_max_age_seconds.is_none())
    );
    let (engine, _temp, discoveries, fetches) =
        make_engine(catalog, DiscoveryMode::OldFrame, FetchMode::Fixture, DiscoveryMode::Empty);

    let result = engine
        .local_view(request(Some("rainviewer"), GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::Failed);
    assert_eq!(result.selection[0].discovery_status, Some(DiscoveryStatus::Stale));
    assert_eq!(result.selection[0].reason, "stale");
    assert_eq!(discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_view_applies_catalog_freshness_before_fetching() {
    let mut catalog = spatial_catalog(false, true, false);
    catalog.sources[0].products[0].suggested_max_age_seconds = Some(1.0);
    let (engine, _temp, discoveries, fetches) =
        make_engine(catalog, DiscoveryMode::Frame, FetchMode::Fixture, DiscoveryMode::Empty);

    let result = engine
        .local_view(request(Some("rainviewer"), GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::Failed);
    assert_eq!(result.selection[0].discovery_status, Some(DiscoveryStatus::Stale));
    assert_eq!(discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_view_uses_one_deadline_across_candidate_discovery() {
    let (engine, _temp, discoveries, fetches, _) = make_engine_with_deadline(
        spatial_catalog(false, true, false),
        DiscoveryMode::Delay(1000, false),
        DiscoveryMode::Empty,
        0.05,
    );

    let result = engine
        .local_view(request(Some("rainviewer"), GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::TimedOut);
    assert_eq!(discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_view_refresh_consumes_the_same_absolute_deadline() {
    let (engine, _temp, rainviewer_discoveries, rainviewer_fetches, rdcap_refreshes) =
        make_engine_with_deadline(
            spatial_catalog(true, true, false),
            DiscoveryMode::Empty,
            DiscoveryMode::Delay(1000, false),
            0.05,
        );

    let result = engine
        .local_view(request(Some("rdcap"), GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::TimedOut);
    assert_eq!(rdcap_refreshes.load(Ordering::SeqCst), 1);
    assert!(result.selection.iter().any(|attempt| {
        attempt.target.source == "rdcap"
            && attempt.reason.ends_with(";directory_refresh_failed:timeout")
    }));
    assert_eq!(rainviewer_discoveries.load(Ordering::SeqCst), 0);
    assert_eq!(rainviewer_fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_view_shares_the_deadline_across_the_eight_candidate_batch() {
    let stations = (0..10)
        .map(|index| {
            serde_json::json!({
                "id": format!("RV{index:02}"),
                "product_ids": ["composite"]
            })
        })
        .collect::<Vec<_>>();
    let catalog = SourceCatalog::parse(
        &serde_json::json!({
            "schema_version": 1,
            "sources": [{
                "id": "rainviewer",
                "availability": "available",
                "products": [{ "id": "composite" }],
                "stations": stations
            }]
        })
        .to_string(),
    )
    .unwrap();
    let (engine, _temp, discoveries, fetches, _) = make_engine_with_deadline(
        catalog,
        DiscoveryMode::Delay(70, false),
        DiscoveryMode::Empty,
        0.12,
    );

    let result = engine
        .local_view(request(Some("rainviewer"), GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::TimedOut);
    assert!(result.candidate_limit_reached);
    assert_eq!(discoveries.load(Ordering::SeqCst), 2);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_view_cancellation_stops_discovery_without_fetch_or_publish() {
    let (engine, _temp, discoveries, fetches, _) = make_engine_with_deadline(
        spatial_catalog(false, true, false),
        DiscoveryMode::Delay(1000, true),
        DiscoveryMode::Empty,
        10.0,
    );
    let engine = Arc::new(engine);
    let request_engine = engine.clone();
    let operation = tokio::spawn(async move {
        request_engine
            .local_view(request(
                Some("rainviewer"),
                GeographicPoint { longitude: 0.0, latitude: 0.0 },
            ))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while discoveries.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    engine.request_budget.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.status, LocalViewStatus::Interrupted);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_view_cancellation_during_cpu_work_releases_the_worker() {
    let (engine, _temp, _discoveries, fetches) = make_engine(
        spatial_catalog(false, true, false),
        DiscoveryMode::Frame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
    );
    let engine = Arc::new(engine);
    let initial_permits = engine.decode_workers.available_permits();
    let request_engine = engine.clone();
    let operation = tokio::spawn(async move {
        let mut request =
            request(Some("rainviewer"), GeographicPoint { longitude: 0.0, latitude: 0.0 });
        request.radius_km = Some(500.0);
        request.resolution_m = Some(500.0);
        request_engine.local_view(request).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while fetches.load(Ordering::SeqCst) == 0
            || engine.decode_workers.available_permits() == initial_permits
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    engine.request_budget.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), operation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.status, LocalViewStatus::Interrupted);
    assert!(result.selected.is_none());
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while engine.decode_workers.available_permits() != initial_permits {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn local_view_stops_after_eight_candidates_and_reports_the_rest() {
    let stations = (0..10)
        .map(|index| {
            serde_json::json!({
                "id": format!("S{index:02}"),
                "product_ids": ["reflectivity"]
            })
        })
        .collect::<Vec<_>>();
    let catalog = SourceCatalog::parse(
        &serde_json::json!({
            "schema_version": 1,
            "sources": [{
                "id": "rdcap",
                "availability": "available",
                "products": [{ "id": "reflectivity" }],
                "stations": stations
            }]
        })
        .to_string(),
    )
    .unwrap();
    let (engine, _temp, _, _) =
        make_engine(catalog, DiscoveryMode::Empty, FetchMode::Error, DiscoveryMode::Empty);

    let result = engine
        .local_view(request(Some("rdcap"), GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();

    assert!(result.candidate_limit_reached);
    assert_eq!(result.selection.len(), 10);
    assert_eq!(
        result
            .selection
            .iter()
            .filter(|attempt| attempt.state == LocalViewCandidateState::NotStarted)
            .count(),
        2
    );
    assert!(
        result
            .selection
            .iter()
            .filter(|attempt| { attempt.state == LocalViewCandidateState::NotStarted })
            .all(|attempt| attempt.reason == "candidate_limit")
    );
}

#[tokio::test]
async fn local_view_raw_image_requires_verified_geometry_and_nearest_sampling() {
    let (engine, _temp, fetches) = make_pt_engine(false);

    let result = engine
        .local_view(pt_request(LocalViewMode::Raw, LocalViewResampling::Nearest, 1.0))
        .await
        .unwrap();

    assert_eq!(result.status, LocalViewStatus::Selected);
    assert_eq!(result.selected.as_ref().unwrap().valid_time, pt_frame().valid_time);
    assert!(matches!(result.data, Some(LocalViewData::RawImage { width: 3, height: 3, .. })));
    let processing = result.processing.as_ref().unwrap();
    assert_eq!(processing.data_kind, "raw_image");
    assert_eq!(processing.input_logical_id, pt_frame().logical_id);
    assert_eq!(
        processing.input_receipts[0].sha256,
        "fc44efba8f3c2b8ddfd71b9ba25d54282a2311c423044133a483d7a83eab18fa"
    );
    assert_eq!(fetches.load(Ordering::SeqCst), 1);

    let automatic = engine
        .local_view(pt_request(LocalViewMode::Auto, LocalViewResampling::Nearest, 1.0))
        .await
        .unwrap();
    assert_eq!(automatic.status, LocalViewStatus::Selected);
    assert!(matches!(automatic.data, Some(LocalViewData::RawImage { .. })));

    for (mode, resampling) in [
        (LocalViewMode::Auto, LocalViewResampling::Bilinear),
        (LocalViewMode::Decoded, LocalViewResampling::Nearest),
        (LocalViewMode::Dbz, LocalViewResampling::Nearest),
    ] {
        let rejected = engine.local_view(pt_request(mode, resampling, 1.0)).await.unwrap();
        assert_eq!(rejected.status, LocalViewStatus::NoCandidate);
        assert!(rejected.data.is_none());
        assert!(
            rejected
                .selection
                .iter()
                .any(|attempt| { attempt.reason == "unsupported_mode_or_geometry" })
        );
    }
    assert_eq!(fetches.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn local_view_cache_hit_keeps_input_identity_and_time_when_view_changes() {
    let (engine, _temp, fetches) = make_pt_engine(true);

    let first = engine
        .local_view(pt_request(LocalViewMode::Raw, LocalViewResampling::Nearest, 1.0))
        .await
        .unwrap();
    let second = engine
        .local_view(pt_request(LocalViewMode::Raw, LocalViewResampling::Nearest, 2.0))
        .await
        .unwrap();

    let first_processing = first.processing.as_ref().unwrap();
    let second_processing = second.processing.as_ref().unwrap();
    assert_eq!(first.status, LocalViewStatus::Selected);
    assert_eq!(second.status, LocalViewStatus::Selected);
    assert_eq!(
        first.selected.as_ref().unwrap().valid_time,
        second.selected.as_ref().unwrap().valid_time
    );
    assert_eq!(
        first.selected.as_ref().unwrap().logical_id,
        second.selected.as_ref().unwrap().logical_id
    );
    assert_eq!(first_processing.input_logical_id, second_processing.input_logical_id);
    assert_eq!(first_processing.input_receipts, second_processing.input_receipts);
    assert_ne!(first_processing.processing_id, second_processing.processing_id);
    assert_eq!(fetches.load(Ordering::SeqCst), 1, "second view must use the raw cache hit");
}

#[tokio::test]
async fn local_view_refresh_timeout_preserves_builtin_candidates_and_fallback_time() {
    let (engine, _temp, discoveries, fetches, refreshes) = make_engine_with_deadline(
        spatial_catalog(true, true, false),
        DiscoveryMode::Frame,
        DiscoveryMode::RefreshDelay(1000),
        0.5,
    );
    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();
    assert_eq!(result.status, LocalViewStatus::Selected);
    assert_eq!(result.selected.as_ref().unwrap().source, "rainviewer");
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    let rdcap = result.selection.iter().find(|attempt| attempt.target.source == "rdcap").unwrap();
    assert_eq!(rdcap.discovery_status, Some(DiscoveryStatus::NoData));
    assert!(rdcap.reason.ends_with(";directory_refresh_failed:timeout"));
    assert!(rdcap.spatial_evidence.is_some());
    assert_eq!(discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn local_view_does_not_refresh_rdcap_before_an_earlier_candidate_succeeds() {
    let (engine, _temp, _, _, refreshes) = make_engine_with_deadline(
        spatial_catalog(true, true, true),
        DiscoveryMode::Frame,
        DiscoveryMode::RefreshDelay(1000),
        0.5,
    );
    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();
    assert_eq!(result.status, LocalViewStatus::Selected);
    assert_eq!(refreshes.load(Ordering::SeqCst), 0);
    assert!(result.observation_coverage.is_some());
    assert!(result.selection.iter().all(|attempt| attempt.spatial_evidence.is_some()));
}

#[tokio::test]
async fn local_view_acquisition_cancellation_and_resource_limits_stop_fallback() {
    for (mode, status, reason) in [
        (FetchMode::Cancelled, LocalViewStatus::Interrupted, "cancelled"),
        (FetchMode::ResourceLimit, LocalViewStatus::Failed, "resource_limit"),
    ] {
        let (engine, _temp, discoveries, fetches, refreshes) = make_engine_with_runtime(
            spatial_catalog(true, true, true),
            DiscoveryMode::Frame,
            mode,
            DiscoveryMode::Empty,
            100_000_000,
            10 * 1024 * 1024 * 1024,
            1.0,
        );
        let result = engine
            .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
            .await
            .unwrap();
        assert_eq!(result.status, status);
        assert_eq!(result.error.as_ref().unwrap().code, reason);
        let later =
            result.selection.iter().find(|attempt| attempt.target.source == "rdcap").unwrap();
        assert_eq!(later.state, LocalViewCandidateState::NotStarted);
        assert_eq!(later.reason, reason);
        assert_eq!(refreshes.load(Ordering::SeqCst), 0);
        assert_eq!(discoveries.load(Ordering::SeqCst), 1);
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn local_view_raw_image_rejects_a_center_outside_the_current_frame() {
    let (engine, _temp, fetches) = make_pt_engine(false);
    let mut request = pt_request(LocalViewMode::Raw, LocalViewResampling::Nearest, 1.0);
    request.center = Some(GeographicPoint { longitude: 0.0, latitude: 0.0 });
    let result = engine.local_view(request).await.unwrap();
    assert_eq!(result.status, LocalViewStatus::Failed);
    assert!(result.data.is_none());
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
    assert_eq!(result.selection[0].reason, "outside_current_frame_domain");
}

#[tokio::test]
async fn local_view_directory_merge_failure_keeps_builtin_targets_and_falls_back() {
    let (engine, _temp, discoveries, fetches, refreshes) = make_engine_with_deadline(
        spatial_catalog(true, true, false),
        DiscoveryMode::Frame,
        DiscoveryMode::InvalidDirectory,
        1.0,
    );
    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();
    assert_eq!(result.status, LocalViewStatus::Selected);
    let attempted =
        result.selection.iter().find(|attempt| attempt.target.source == "rdcap").unwrap();
    assert_eq!(attempted.target.station.as_deref(), Some("TWRCHL"));
    assert_eq!(attempted.discovery_status, Some(DiscoveryStatus::NoData));
    assert!(attempted.reason.contains(";directory_refresh_failed:"));
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn local_view_stale_candidate_tries_the_next_target_without_fetching() {
    let (engine, _temp, discoveries, fetches, refreshes) = make_engine_with_runtime(
        spatial_catalog(true, true, true),
        DiscoveryMode::OldFrame,
        FetchMode::Fixture,
        DiscoveryMode::Empty,
        100_000_000,
        10 * 1024 * 1024 * 1024,
        1.0,
    );
    let result = engine
        .local_view(request(None, GeographicPoint { longitude: 0.0, latitude: 0.0 }))
        .await
        .unwrap();
    assert_eq!(result.selection[0].discovery_status, Some(DiscoveryStatus::Stale));
    let next = result.selection.iter().find(|attempt| attempt.target.source == "rdcap").unwrap();
    assert_eq!(next.discovery_status, Some(DiscoveryStatus::NoData));
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(discoveries.load(Ordering::SeqCst), 1);
    assert_eq!(fetches.load(Ordering::SeqCst), 0);
}
