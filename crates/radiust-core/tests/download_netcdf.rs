use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::download::{DecodedGrid, DecodedProcessing, DownloadStatus, FetchErrorPolicy};
use radiust_core::engine::Engine;
use radiust_core::error_contract::{ErrorCode, ErrorStage};
use radiust_core::errors::{CoreError, CoreResult};
use radiust_core::identity::logical_id;
use radiust_core::limits::Limits;
use radiust_core::model::{
    ArtifactReceipt, DiscoveryTarget, FrameRef, Grid, RadarField, RawArtifact, RawFrame,
};
use radiust_core::output::netcdf::read_selected_field;
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Copy)]
struct FixtureArtifact {
    name: &'static str,
    media_type: &'static str,
    bytes: &'static [u8],
}

struct FixtureAdapter {
    source: &'static str,
    artifacts: Vec<FixtureArtifact>,
}

impl SourceAdapter for FixtureAdapter {
    fn source_id(&self) -> &'static str {
        self.source
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
        let fixtures = self.artifacts.clone();
        Some(Box::pin(async move {
            fs::create_dir_all(&temp_root)
                .map_err(|error| CoreError::Temporary(error.to_string()))?;
            let corrupt =
                frame.locator.get("fixture_corrupt").and_then(Value::as_bool) == Some(true);
            let mut artifacts = Vec::with_capacity(fixtures.len());
            for fixture in fixtures {
                let bytes: &[u8] = if corrupt && fixture.media_type == "image/png" {
                    b"broken fixture tile"
                } else {
                    fixture.bytes
                };
                let path = temp_root.join(format!(
                    "{}-{}-{}",
                    frame.logical_id,
                    uuid::Uuid::new_v4().simple(),
                    fixture.name
                ));
                fs::write(&path, bytes).map_err(|error| CoreError::Temporary(error.to_string()))?;
                let path = tempfile::TempPath::try_from_path(path)
                    .map_err(|_| CoreError::Temporary("fixture temp path failed".into()))?;
                artifacts.push(RawArtifact {
                    receipt: ArtifactReceipt {
                        name: fixture.name.into(),
                        media_type: fixture.media_type.into(),
                        size_bytes: bytes.len() as u64,
                        sha256: hex::encode(Sha256::digest(bytes)),
                    },
                    path,
                });
            }
            Ok(RawFrame { frame, artifacts, private_locator: None })
        }))
    }
}

fn fixture_artifacts(source: &'static str) -> Vec<FixtureArtifact> {
    match source {
        "rainviewer" => vec![
            FixtureArtifact {
                name: "tile-z1-x0-y0.png",
                media_type: "image/png",
                bytes: include_bytes!(
                    "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y0.png"
                ),
            },
            FixtureArtifact {
                name: "tile-z1-x1-y0.png",
                media_type: "image/png",
                bytes: include_bytes!(
                    "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x1-y0.png"
                ),
            },
            FixtureArtifact {
                name: "tile-z1-x0-y1.png",
                media_type: "image/png",
                bytes: include_bytes!(
                    "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x0-y1.png"
                ),
            },
            FixtureArtifact {
                name: "tile-z1-x1-y1.png",
                media_type: "image/png",
                bytes: include_bytes!(
                    "../../../tests/fixtures/sources/rainviewer/raw/tile-z1-x1-y1.png"
                ),
            },
        ],
        "tw" => vec![FixtureArtifact {
            name: "O-A0059-001.json",
            media_type: "application/json",
            bytes: include_bytes!("../../../tests/fixtures/sources/tw/raw/O-A0059-001.json"),
        }],
        _ => unreachable!("only accepted NetCDF decoder fixtures are used"),
    }
}

fn rainviewer_frame(index: usize, corrupt: bool) -> FrameRef {
    let minute = index * 10;
    let mut locator = json!({
        "api_url": "https://api.rainviewer.com/public/weather-maps.json",
        "host": "https://tilecache.rainviewer.com",
        "path": "/v2/radar/7cc4a10f8d53",
        "tile_size": 512,
        "zoom": 1,
        "color": 2,
        "options": "0_0",
    });
    if corrupt {
        locator["fixture_corrupt"] = json!(true);
    }
    let mut frame = FrameRef {
        source: "rainviewer".into(),
        product: "composite".into(),
        station: None,
        valid_time: format!("2026-09-18T02:{minute:02}:00.000000Z"),
        base_time: None,
        logical_id: String::new(),
        revision: Some("7cc4a10f8d53".into()),
        locator_version: "rainviewer-v2".into(),
        locator,
    };
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

fn tw_grid_frame() -> FrameRef {
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
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

fn engine(root: &std::path::Path, frame_concurrency: usize) -> Engine {
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.frame_concurrency = frame_concurrency;
    config.runtime.temp_root = Some(root.join("temp"));
    config.cache.dir = root.join("cache");
    config.storage.output = root.join("output");
    let mut sources = SourceRegistry::default();
    for source in ["rainviewer", "tw"] {
        sources
            .register(Arc::new(FixtureAdapter { source, artifacts: fixture_artifacts(source) }))
            .unwrap();
    }
    Engine::new(config, sources).unwrap()
}

fn assert_netcdf_commit(root: &std::path::Path, frame: &FrameRef) -> PathBuf {
    let output = root.join("output").join(format!("frames/{}/decoded.nc", frame.logical_id));
    assert!(output.is_file());
    let manifest_path = output.with_file_name("decoded.nc.manifest.json");
    assert!(manifest_path.is_file());
    let manifest: Value = serde_json::from_slice(&fs::read(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["processing_spec"]["format"], "netcdf");
    assert_eq!(manifest["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(
        manifest["artifacts"][0]["relative_uri"],
        format!("frames/{}/decoded.nc", frame.logical_id)
    );
    assert_eq!(manifest["artifacts"][0]["media_type"], "application/x-netcdf");
    let parsed_manifest = radiust_core::storage::manifest::read_manifest(
        &output.with_file_name("decoded.nc.manifest.json"),
    )
    .unwrap()
    .unwrap();
    assert!(radiust_core::storage::manifest::is_complete(&root.join("output"), &parsed_manifest));
    output
}

#[tokio::test]
async fn rainviewer_composite_download_roundtrips_through_netcdf_and_manifest_last_commit() {
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), 1);
    let frame = rainviewer_frame(0, false);

    let report =
        engine.download_netcdf(vec![frame.clone()], FetchErrorPolicy::Collect, false, false).await;

    assert_eq!(report.written, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(report.items[0].status, DownloadStatus::Written);
    let output = assert_netcdf_commit(root.path(), &frame);
    assert_eq!(
        report.items[0].output_uri.as_deref(),
        Some(fs::canonicalize(&output).unwrap().to_str().unwrap())
    );
    let field = read_selected_field(&output, "reflectivity", None, &Limits::default()).unwrap();
    assert_eq!(field.shape, [1024, 1024]);
    assert_eq!(field.units.as_deref(), Some("dBZ"));
    assert_eq!(
        radiust_core::model::parse_utc_time(&field.valid_time).unwrap(),
        radiust_core::model::parse_utc_time(&frame.valid_time).unwrap()
    );
}

#[tokio::test]
async fn validated_science_download_commits_all_four_formats_with_complete_manifests() {
    for format in ["png", "netcdf", "geotiff", "zarr"] {
        let root = tempfile::tempdir().unwrap();
        let engine = engine(root.path(), 1);
        let frame = rainviewer_frame(0, false);
        let output_root = root.path().join("format-output");
        let report = match format {
            "png" => {
                engine
                    .download_png_to(
                        vec![frame.clone()],
                        FetchErrorPolicy::Collect,
                        false,
                        false,
                        output_root.clone(),
                    )
                    .await
            }
            "netcdf" => {
                engine
                    .download_netcdf_to(
                        vec![frame.clone()],
                        FetchErrorPolicy::Collect,
                        false,
                        false,
                        output_root.clone(),
                    )
                    .await
            }
            "geotiff" => {
                engine
                    .download_geotiff_to(
                        vec![frame.clone()],
                        FetchErrorPolicy::Collect,
                        false,
                        false,
                        output_root.clone(),
                    )
                    .await
            }
            "zarr" => {
                engine
                    .download_zarr_to(
                        vec![frame.clone()],
                        FetchErrorPolicy::Collect,
                        false,
                        false,
                        output_root.clone(),
                    )
                    .await
            }
            _ => unreachable!(),
        };

        assert_eq!(report.written, 1, "{format} report: {report:?}");
        assert_eq!(report.failed, 0, "{format} report: {report:?}");
        let output = PathBuf::from(report.items[0].output_uri.as_ref().unwrap());
        assert!(output.exists(), "missing {format} output at {}", output.display());
        let manifest_path = PathBuf::from(format!("{}.manifest.json", output.display()));
        let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["processing_spec"]["format"], format);
        let artifacts = manifest["artifacts"].as_array().unwrap();
        assert!(!artifacts.is_empty());
        for artifact in artifacts {
            let relative = artifact["relative_uri"].as_str().unwrap();
            let bytes = fs::read(output_root.join(relative)).unwrap();
            assert_eq!(artifact["size_bytes"].as_u64().unwrap(), bytes.len() as u64);
            assert_eq!(
                artifact["sha256"].as_str().unwrap(),
                hex::encode(Sha256::digest(&bytes)),
                "{format} artifact {relative}"
            );
        }
        let parsed_manifest =
            radiust_core::storage::manifest::read_manifest(&manifest_path).unwrap().unwrap();
        assert!(radiust_core::storage::manifest::is_complete(&output_root, &parsed_manifest));

        match format {
            "png" => {
                let image = image::open(&output).unwrap();
                assert_eq!((image.width(), image.height()), (1024, 1024));
                let sidecar = output.with_file_name("decoded.render.json");
                let sidecar: Value = serde_json::from_slice(&fs::read(sidecar).unwrap()).unwrap();
                assert_eq!(sidecar["crs"], "EPSG:4326");
            }
            "netcdf" => {
                let field =
                    read_selected_field(&output, "reflectivity", None, &Limits::default()).unwrap();
                assert_eq!(field.shape, [1024, 1024]);
                assert_eq!(field.units.as_deref(), Some("dBZ"));
            }
            "geotiff" => {
                let mut data =
                    tiff::decoder::Decoder::new(fs::File::open(&output).unwrap()).unwrap();
                assert_eq!(data.dimensions().unwrap(), (1024, 1024));
                assert!(
                    matches!(data.read_image().unwrap(), tiff::decoder::DecodingResult::F32(values)
                    if values.len() == 1024 * 1024)
                );
                assert!(output.with_file_name("decoded_quality.tif").is_file());
                assert!(output.with_file_name("decoded_provenance.json").is_file());
            }
            "zarr" => {
                let metadata: Value =
                    serde_json::from_slice(&fs::read(output.join(".zmetadata")).unwrap()).unwrap();
                assert_eq!(metadata["metadata"]["reflectivity/.zarray"]["dtype"], "<f4");
                assert_eq!(metadata["metadata"]["quality/.zarray"]["dtype"], "<u2");
                assert_eq!(
                    metadata["metadata"]["reflectivity/.zarray"]["shape"],
                    json!([1024, 1024])
                );
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn decoded_download_can_atomically_supplement_verified_raw_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), 1);
    let frame = rainviewer_frame(0, false);

    let decoded =
        engine.download_netcdf(vec![frame.clone()], FetchErrorPolicy::Collect, false, false).await;
    assert_eq!(decoded.written, 1);
    let output = root.path().join("output").join(format!("frames/{}/decoded.nc", frame.logical_id));
    let manifest_path = output.with_file_name("decoded.nc.manifest.json");
    let original: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(original["raw_complete"], false);

    let supplemented = engine
        .download_decoded_to_with_template_and_raw(
            vec![frame.clone()],
            FetchErrorPolicy::Collect,
            false,
            false,
            root.path().join("output"),
            "netcdf",
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(supplemented.written, 1);
    assert_eq!(supplemented.failed, 0);
    assert_eq!(supplemented.items[0].output_uri.as_deref(), decoded.items[0].output_uri.as_deref());

    let manifest_bytes = fs::read(&manifest_path).unwrap();
    let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    assert_eq!(manifest["raw_complete"], true);
    assert_eq!(manifest["output_id"], original["output_id"]);
    assert_eq!(manifest["processing_hash"], original["processing_hash"]);
    assert_eq!(manifest["artifacts"].as_array().unwrap().len(), 6);
    for artifact in manifest["artifacts"].as_array().unwrap() {
        let relative = artifact["relative_uri"].as_str().unwrap();
        let path = root.path().join("output").join(relative);
        let bytes = fs::read(path).unwrap();
        assert_eq!(artifact["size_bytes"].as_u64().unwrap(), bytes.len() as u64);
        assert_eq!(artifact["sha256"].as_str().unwrap(), hex::encode(Sha256::digest(&bytes)));
    }
    let raw_manifest_path = output.with_file_name("raw-manifest.json");
    let raw_manifest: Value =
        serde_json::from_slice(&fs::read(raw_manifest_path).unwrap()).unwrap();
    assert_eq!(raw_manifest["raw_complete"], true);
    assert_eq!(raw_manifest["artifacts"].as_array().unwrap().len(), 4);
    assert!(read_selected_field(&output, "reflectivity", None, &Limits::default()).is_ok());

    let repeated = engine
        .download_decoded_to_with_template_and_raw(
            vec![frame],
            FetchErrorPolicy::Collect,
            false,
            false,
            root.path().join("output"),
            "netcdf",
            None,
            true,
        )
        .await
        .unwrap();
    assert_eq!(repeated.skipped, 1);
    assert_eq!(repeated.failed, 0);
}

#[tokio::test]
async fn rainviewer_geographic_download_regrids_and_records_processing_identity() {
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), 1);
    let frame = rainviewer_frame(0, false);
    let processing = DecodedProcessing {
        variable: Some("reflectivity".into()),
        grid: DecodedGrid::Geographic { bbox: [-1.0, -1.0, 1.0, 1.0], resolution: 1.0 },
        resampling: radiust_core::grid::Resampling::Nearest,
    };

    let report = engine
        .download_decoded_to_with_processing(
            vec![frame.clone()],
            FetchErrorPolicy::Collect,
            false,
            false,
            root.path().join("output"),
            "netcdf",
            None,
            processing,
        )
        .await
        .unwrap();

    assert_eq!(report.written, 1);
    assert_eq!(report.failed, 0);
    let output = assert_netcdf_commit(root.path(), &frame);
    let manifest: Value = serde_json::from_slice(
        &fs::read(output.with_file_name("decoded.nc.manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["processing_spec"]["variable"], "reflectivity");
    assert_eq!(manifest["processing_spec"]["grid"], "geographic");
    assert_eq!(manifest["processing_spec"]["bbox"], json!([-1.0, -1.0, 1.0, 1.0]));
    assert_eq!(manifest["processing_spec"]["resolution"], 1.0);
    assert_eq!(manifest["processing_spec"]["resampling"], "nearest");

    let field = read_selected_field(&output, "reflectivity", None, &Limits::default()).unwrap();
    assert_eq!(field.shape, [3, 3]);
    assert_eq!(field.grid.crs.as_deref(), Some("EPSG:4326"));
    assert_eq!(field.grid.x, [-1.0, 0.0, 1.0]);
    assert_eq!(field.grid.y, [-1.0, 0.0, 1.0]);
}

#[tokio::test]
async fn changed_processing_spec_does_not_skip_an_older_decoded_output() {
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), 1);
    let frame = rainviewer_frame(0, false);

    let original =
        engine.download_netcdf(vec![frame.clone()], FetchErrorPolicy::Collect, false, false).await;
    assert_eq!(original.written, 1);

    let changed = engine
        .download_decoded_to_with_processing(
            vec![frame.clone()],
            FetchErrorPolicy::Collect,
            false,
            false,
            root.path().join("output"),
            "netcdf",
            None,
            DecodedProcessing {
                variable: Some("reflectivity".into()),
                grid: DecodedGrid::Geographic { bbox: [-1.0, -1.0, 1.0, 1.0], resolution: 1.0 },
                resampling: radiust_core::grid::Resampling::Nearest,
            },
        )
        .await
        .unwrap();

    assert_eq!(changed.skipped, 0);
    assert_eq!(changed.failed, 1);
    assert_eq!(changed.items[0].status, DownloadStatus::Failed);
    let output = root.path().join("output").join(format!("frames/{}/decoded.nc", frame.logical_id));
    let manifest: Value = serde_json::from_slice(
        &fs::read(output.with_file_name("decoded.nc.manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["processing_spec"]["grid"], "native");
}

#[tokio::test]
async fn geographic_download_reports_unsupported_cross_crs_regrid() {
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), 1);
    let frame = tw_grid_frame();

    let report = engine
        .download_decoded_to_with_processing(
            vec![frame],
            FetchErrorPolicy::Collect,
            false,
            false,
            root.path().join("output"),
            "netcdf",
            None,
            DecodedProcessing {
                variable: None,
                grid: DecodedGrid::Geographic { bbox: [120.0, 21.0, 121.0, 22.0], resolution: 0.5 },
                resampling: radiust_core::grid::Resampling::Nearest,
            },
        )
        .await
        .unwrap();

    assert_eq!(report.failed, 1);
    let error = report.items[0].error.as_ref().unwrap();
    assert_eq!(error.code, radiust_core::error_contract::ErrorCode::Unsupported);
    assert_eq!(error.stage, ErrorStage::Regrid);
    assert!(error.message.contains("EPSG:3821 to EPSG:4326"));
    assert!(error.message.contains("no verified transform"));
}

#[tokio::test]
async fn engine_regrid_supports_verified_web_mercator_sampling() {
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), 1);
    let field = RadarField {
        name: "reflectivity".into(),
        values: vec![10.0, 20.0, 30.0, 40.0],
        shape: vec![2, 2],
        quality: vec![0, 1, 4, 8],
        units: Some("dBZ".into()),
        valid_time: "2026-09-25T00:00:00Z".into(),
        grid: Grid {
            shape: vec![2, 2],
            crs: Some("EPSG:4326".into()),
            x: vec![1.0, 4.0],
            y: vec![46.0, 50.0],
            affine: None,
        },
        provenance: vec!["source=fixture".into()],
    };
    let target = Grid {
        shape: vec![1, 1],
        crs: Some("EPSG:3857".into()),
        x: vec![222_638.98],
        y: vec![6_274_861.39],
        affine: None,
    };

    let projected =
        engine.regrid(field, target, radiust_core::grid::Resampling::Nearest).await.unwrap();

    assert_eq!(projected.values, [30.0]);
    assert_eq!(projected.quality, [4]);
    assert_eq!(projected.grid.crs.as_deref(), Some("EPSG:3857"));
    assert!(projected.provenance.iter().any(|item| item.contains("method=web_mercator")));
}

#[tokio::test]
async fn tw_grid_download_roundtrips_through_netcdf_with_native_geometry() {
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), 1);
    let frame = tw_grid_frame();

    let report =
        engine.download_netcdf(vec![frame.clone()], FetchErrorPolicy::Collect, false, false).await;

    assert_eq!(report.written, 1);
    assert_eq!(report.failed, 0);
    let output = assert_netcdf_commit(root.path(), &frame);
    let field = read_selected_field(&output, "reflectivity", None, &Limits::default()).unwrap();
    assert_eq!(field.shape, [881, 921]);
    assert_eq!(field.grid.crs.as_deref(), Some("EPSG:3821"));
    assert_eq!(field.units.as_deref(), Some("dBZ"));
    assert_eq!(
        radiust_core::model::parse_utc_time(&field.valid_time).unwrap(),
        radiust_core::model::parse_utc_time(&frame.valid_time).unwrap()
    );
}

#[tokio::test]
async fn netcdf_download_preserves_collect_and_ordered_stop_reports() {
    let collect_root = tempfile::tempdir().unwrap();
    let collect_engine = engine(collect_root.path(), 1);
    let collect_frames =
        vec![rainviewer_frame(0, false), rainviewer_frame(1, true), rainviewer_frame(2, false)];
    let collect = collect_engine
        .download_netcdf(collect_frames.clone(), FetchErrorPolicy::Collect, false, false)
        .await;

    assert_eq!(collect.written, 2);
    assert_eq!(collect.failed, 1);
    assert_eq!(collect.not_started, 0);
    assert_eq!(
        collect.items.iter().map(|item| item.status).collect::<Vec<_>>(),
        vec![DownloadStatus::Written, DownloadStatus::Failed, DownloadStatus::Written]
    );
    let error = collect.items[1].error.as_ref().unwrap();
    assert_eq!(error.code, ErrorCode::Transport);
    assert_eq!(error.stage, ErrorStage::Decode);
    assert!(error.retryable);
    assert_eq!(error.message, "transport request failed");

    let stop_root = tempfile::tempdir().unwrap();
    let stop_engine = engine(stop_root.path(), 1);
    let stop = stop_engine
        .download_netcdf(collect_frames.clone(), FetchErrorPolicy::Stop, false, false)
        .await;

    assert_eq!(stop.written, 1);
    assert_eq!(stop.failed, 1);
    assert_eq!(stop.not_started, 1);
    assert_eq!(
        stop.items.iter().map(|item| item.status).collect::<Vec<_>>(),
        vec![DownloadStatus::Written, DownloadStatus::Failed, DownloadStatus::NotStarted]
    );
    assert_netcdf_commit(stop_root.path(), &collect_frames[0]);
    assert!(
        !stop_root
            .path()
            .join(format!("output/frames/{}/decoded.nc", collect_frames[2].logical_id))
            .exists()
    );
}

#[tokio::test]
async fn netcdf_dry_run_marks_frames_planned_without_creating_output() {
    let root = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::default();
    config.runtime.allow_network = false;
    config.storage.output = root.path().join("output");
    let engine = Engine::new(config, SourceRegistry::default()).unwrap();
    let frame = rainviewer_frame(0, false);

    let report = engine.download_netcdf(vec![frame], FetchErrorPolicy::Stop, true, false).await;

    assert_eq!(report.planned, 1);
    assert_eq!(report.items[0].status, DownloadStatus::Planned);
    assert!(report.items[0].output_uri.is_none());
    assert!(!root.path().join("output").exists());
}

#[tokio::test]
async fn netcdf_download_rejects_rainviewer_products_without_a_verified_decoder() {
    let root = tempfile::tempdir().unwrap();
    let engine = engine(root.path(), 1);
    let mut frame = rainviewer_frame(0, false);
    frame.product = "unverified".into();
    frame.logical_id = logical_id(&frame).unwrap();

    let report = engine.download_netcdf(vec![frame], FetchErrorPolicy::Collect, false, false).await;

    assert_eq!(report.failed, 1);
    let error = report.items[0].error.as_ref().unwrap();
    assert_eq!(error.code, ErrorCode::Unsupported);
    assert_eq!(error.stage, ErrorStage::Decode);
    assert!(!error.retryable);
    assert_eq!(
        error.message,
        "validated scientific decoding is unavailable for this source/product"
    );
    assert!(!root.path().join("output/frames").exists());
}

#[test]
fn netcdf_runtime_storage_errors_have_a_safe_commit_report() {
    let report = radiust_core::error_contract::ErrorReport::from_core(
        &CoreError::Storage("NetCDF4 operation failed".into()),
        ErrorStage::Commit,
    );
    assert_eq!(report.code, ErrorCode::Storage);
    assert_eq!(report.stage, ErrorStage::Commit);
    assert!(!report.retryable);
    assert_eq!(report.message, "output operation failed");
}
