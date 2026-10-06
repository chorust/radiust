use radiust_core::config::CoreConfig;
use radiust_core::download::{DownloadStatus, FetchErrorPolicy};
use radiust_core::engine::Engine;
use radiust_core::errors::CoreError;
use radiust_core::identity::logical_id;
use radiust_core::model::{ArtifactReceipt, DiscoveryTarget, FrameRef, RawArtifact, RawFrame};
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use serde_json::json;
use sha2::Digest;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

struct FixtureAdapter {
    bytes: Vec<u8>,
    receipt: ArtifactReceipt,
}

impl SourceAdapter for FixtureAdapter {
    fn source_id(&self) -> &'static str {
        "fr"
    }

    fn discover(
        self: Arc<Self>,
        _target: DiscoveryTarget,
        _context: SourceContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<FrameRef>, CoreError>> + Send>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        _context: SourceContext,
        temp_root: PathBuf,
    ) -> Option<Pin<Box<dyn Future<Output = Result<RawFrame, CoreError>> + Send>>> {
        let bytes = if frame.locator["fixture_corrupt"] == true {
            b"not a radar image".to_vec()
        } else {
            self.bytes.clone()
        };
        let mut receipt = self.receipt.clone();
        receipt.size_bytes = bytes.len() as u64;
        receipt.sha256 = hex::encode(sha2::Sha256::digest(&bytes));
        Some(Box::pin(async move {
            tokio::fs::create_dir_all(&temp_root)
                .await
                .map_err(|_| CoreError::Temporary("fixture temp root failed".into()))?;
            let mut file = tempfile::NamedTempFile::new_in(&temp_root)
                .map_err(|_| CoreError::Temporary("fixture raw file failed".into()))?;
            file.write_all(&bytes)
                .map_err(|_| CoreError::Temporary("fixture raw write failed".into()))?;
            Ok(RawFrame {
                frame,
                artifacts: vec![RawArtifact { receipt, path: file.into_temp_path() }],
                private_locator: None,
            })
        }))
    }
}

fn fixture_frame(valid_time: &str, corrupt: bool) -> FrameRef {
    let metadata: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../tests/fixtures/sources/fr/fixture.json"))
            .unwrap();
    let original = &metadata["frames"][0];
    let mut frame = FrameRef {
        source: "fr".into(),
        product: "composite".into(),
        station: Some("FRCOMP".into()),
        valid_time: valid_time.into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some(original["revision"].as_str().unwrap().into()),
        locator_version: original["locator_version"].as_str().unwrap().into(),
        locator: original["locator"].clone(),
    };
    if corrupt {
        frame.locator["fixture_corrupt"] = json!(true);
    }
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

fn fixture_engine(root: &std::path::Path) -> Engine {
    let bytes = include_bytes!("../../../tests/fixtures/sources/fr/raw/FRCOMP.png").to_vec();
    let receipt = ArtifactReceipt {
        name: "FRCOMP.png".into(),
        media_type: "image/png".into(),
        size_bytes: bytes.len() as u64,
        sha256: hex::encode(sha2::Sha256::digest(&bytes)),
    };
    let mut sources = SourceRegistry::default();
    sources.register(Arc::new(FixtureAdapter { bytes, receipt })).unwrap();
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.temp_root = Some(root.join("stage"));
    config.runtime.frame_concurrency = 2;
    config.cache.enabled = false;
    Engine::new(config, sources).unwrap()
}

#[tokio::test]
async fn source_dbz_download_writes_numeric_result_and_can_attach_raw_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let engine = fixture_engine(temp.path());
    let frame = fixture_frame("2026-09-18T02:45:00Z", false);

    let fetched = engine
        .fetch_many_mode(vec![frame.clone()], "dbz", FetchErrorPolicy::Collect, false, Some(1))
        .await;
    let decoded = fetched.items.into_iter().next().unwrap().dbz.unwrap();
    engine
        .write_raster_to(
            &decoded,
            temp.path().join("direct"),
            "frames/test/reflectivity.nc",
            "netcdf",
            &json!({}),
            false,
        )
        .unwrap();

    let plain = engine
        .download_dbz_to(
            vec![frame.clone()],
            FetchErrorPolicy::Collect,
            false,
            false,
            temp.path().join("plain"),
            "netcdf",
            false,
        )
        .await
        .unwrap();
    assert_eq!(plain.written, 1, "{plain:#?}");
    let plain_uri = plain.items[0].output_uri.as_deref().unwrap();
    let plain_result = engine.read_dbz_file(plain_uri, None, None).unwrap();
    let radiust_core::raster::RasterResultData::Pixel(plain_field) = plain_result.data else {
        panic!("passed source gray should persist as pixel dBZ");
    };
    assert_eq!(plain_field.units, "dBZ");
    let plain_manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{plain_uri}.manifest.json")).unwrap())
            .unwrap();
    assert!(!plain_manifest["raw_complete"].as_bool().unwrap());

    let repeated = engine
        .download_dbz_to(
            vec![frame.clone()],
            FetchErrorPolicy::Collect,
            false,
            false,
            temp.path().join("plain"),
            "netcdf",
            false,
        )
        .await
        .unwrap();
    assert_eq!(repeated.skipped, 1, "{repeated:#?}");
    assert_eq!(repeated.items[0].status, DownloadStatus::Skipped);

    std::fs::write(plain_uri, b"corrupted numeric output").unwrap();
    let repaired = engine
        .download_dbz_to(
            vec![frame.clone()],
            FetchErrorPolicy::Collect,
            false,
            false,
            temp.path().join("plain"),
            "netcdf",
            false,
        )
        .await
        .unwrap();
    assert_eq!(repaired.written, 1, "{repaired:#?}");
    assert_eq!(repaired.items[0].status, DownloadStatus::Written);
    assert!(engine.read_dbz_file(plain_uri, None, None).is_ok());

    let with_raw = engine
        .download_dbz_to(
            vec![frame],
            FetchErrorPolicy::Collect,
            false,
            false,
            temp.path().join("with-raw"),
            "netcdf",
            true,
        )
        .await
        .unwrap();
    assert_eq!(with_raw.written, 1);
    let output = with_raw.items[0].output_uri.as_deref().unwrap();
    let frame_root = std::path::Path::new(output).parent().unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("{output}.manifest.json")).unwrap()).unwrap();
    assert!(manifest["raw_complete"].as_bool().unwrap());
    assert!(frame_root.join("raw-manifest.json").is_file());
    assert!(frame_root.join("raw/FRCOMP.png").is_file());
}

#[tokio::test]
async fn source_dbz_download_keeps_success_when_another_frame_decode_fails() {
    let temp = tempfile::tempdir().unwrap();
    let engine = fixture_engine(temp.path());
    let frames = vec![
        fixture_frame("2026-09-18T02:45:00Z", false),
        fixture_frame("2026-09-18T02:46:00Z", true),
    ];

    let report = engine
        .download_dbz_to(
            frames,
            FetchErrorPolicy::Collect,
            false,
            false,
            temp.path().join("output"),
            "netcdf",
            false,
        )
        .await
        .unwrap();

    assert_eq!(report.written, 1, "{report:#?}");
    assert_eq!(report.failed, 1);
    assert_eq!(report.items[0].status, DownloadStatus::Written);
    assert_eq!(report.items[1].status, DownloadStatus::Failed);
    assert!(report.items[0].output_uri.is_some());
    assert!(report.items[1].output_uri.is_none());
}
