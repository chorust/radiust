use radiust_core::config::CoreConfig;
use radiust_core::download::{FetchErrorPolicy, FetchStatus};
use radiust_core::engine::Engine;
use radiust_core::errors::CoreError;
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
    source_id: &'static str,
    frame: FrameRef,
    bytes: Vec<u8>,
    receipt: ArtifactReceipt,
}

impl SourceAdapter for FixtureAdapter {
    fn source_id(&self) -> &'static str {
        self.source_id
    }

    fn discover(
        self: Arc<Self>,
        _target: DiscoveryTarget,
        _context: SourceContext,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<FrameRef>, CoreError>> + Send>> {
        Box::pin(async move { Ok(vec![self.frame.clone()]) })
    }

    fn fetch_raw(
        self: Arc<Self>,
        frame: FrameRef,
        _context: SourceContext,
        temp_root: PathBuf,
    ) -> Option<Pin<Box<dyn Future<Output = Result<RawFrame, CoreError>> + Send>>> {
        Some(Box::pin(async move {
            tokio::fs::create_dir_all(&temp_root)
                .await
                .map_err(|_| CoreError::Temporary("fixture temp root failed".into()))?;
            let mut file = tempfile::NamedTempFile::new_in(&temp_root)
                .map_err(|_| CoreError::Temporary("fixture raw file failed".into()))?;
            file.write_all(&self.bytes)
                .map_err(|_| CoreError::Temporary("fixture raw write failed".into()))?;
            Ok(RawFrame {
                frame,
                artifacts: vec![RawArtifact {
                    receipt: self.receipt.clone(),
                    path: file.into_temp_path(),
                }],
                private_locator: None,
            })
        }))
    }
}

fn frame_and_bytes(source: &str, product: &str, station: Option<&str>) -> (FrameRef, Vec<u8>) {
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/sources/fr/fixture.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let metadata = &manifest["frames"][0];
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sources/fr/raw/FRCOMP.png"
    ))
    .unwrap();
    let mut frame = FrameRef {
        source: source.into(),
        product: product.into(),
        station: station.map(str::to_owned),
        valid_time: metadata["valid_time"].as_str().unwrap().into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some(metadata["revision"].as_str().unwrap().into()),
        locator_version: metadata["locator_version"].as_str().unwrap().into(),
        locator: metadata["locator"].clone(),
    };
    frame.logical_id = radiust_core::identity::logical_id(&frame).unwrap();
    (frame, bytes)
}

fn engine_for(frame: FrameRef, bytes: Vec<u8>) -> Engine {
    let receipt = ArtifactReceipt {
        name: "FRCOMP.png".into(),
        media_type: "image/png".into(),
        size_bytes: bytes.len() as u64,
        sha256: hex::encode(sha2::Sha256::digest(&bytes)),
    };
    let mut sources = SourceRegistry::default();
    sources
        .register(Arc::new(FixtureAdapter {
            source_id: match frame.source.as_str() {
                "fr" => "fr",
                "bmkg" => "bmkg",
                _ => "au",
            },
            frame,
            bytes,
            receipt,
        }))
        .unwrap();
    let mut config = CoreConfig::default();
    config.runtime.allow_network = true;
    config.runtime.temp_root = Some(tempfile::tempdir().unwrap().keep().join("raw-stage"));
    config.cache.enabled = false;
    Engine::new(config, sources).unwrap()
}

#[tokio::test]
async fn dbz_batch_keeps_ordered_pixel_result_and_dry_run_has_no_data() {
    let (frame, bytes) = frame_and_bytes("fr", "composite", Some("FRCOMP"));
    let engine = engine_for(frame.clone(), bytes);
    let planned = engine
        .fetch_many_mode(vec![frame.clone()], "dbz", FetchErrorPolicy::Collect, true, Some(1))
        .await;
    assert_eq!(planned.planned, 1);
    assert_eq!(planned.items[0].status, FetchStatus::Planned);
    assert!(planned.items[0].dbz.is_none());

    // Use a fresh engine because the raw fixture stage belongs to one Engine's lifetime.
    let (frame, bytes) = frame_and_bytes("fr", "composite", Some("FRCOMP"));
    let engine = engine_for(frame.clone(), bytes);
    let report =
        engine.fetch_many_mode(vec![frame], "dbz", FetchErrorPolicy::Collect, false, Some(2)).await;
    assert_eq!(report.success, 1);
    assert_eq!(report.items[0].status, FetchStatus::Success);
    let result = report.items[0].dbz.as_ref().expect("dBZ result is retained");
    assert_eq!(result.mode_info.actual.as_deref(), Some("dbz"));
    assert!(matches!(result.data, radiust_core::raster::RasterResultData::Pixel(_)));
}

#[tokio::test]
async fn blocked_source_dbz_is_a_typed_failure_with_no_gray_or_raw_success() {
    let (frame, bytes) = frame_and_bytes("bmkg", "composite", None);
    let engine = engine_for(frame.clone(), bytes);
    let report =
        engine.fetch_many_mode(vec![frame], "dbz", FetchErrorPolicy::Collect, false, Some(1)).await;
    assert_eq!(report.failed, 1);
    assert_eq!(report.items[0].status, FetchStatus::Failed);
    assert!(report.items[0].dbz.is_none());
    assert!(report.items[0].gray.is_none());
    assert_eq!(
        report.items[0].error_details.as_ref().map(|error| error.code),
        Some(radiust_core::error_contract::ErrorCode::DecodeUnverified),
        "{:?} / {:?}",
        report.items[0].error,
        report.items[0].error_details
    );
}

#[test]
fn fixture_receipt_identity_is_content_bound() {
    let (frame, bytes) = frame_and_bytes("fr", "composite", Some("FRCOMP"));
    assert_eq!(frame.source, "fr");
    assert_eq!(hex::encode(sha2::Sha256::digest(&bytes)).len(), 64);
    assert_eq!(frame.locator["layer"], json!("BASE_REFLECTIVITY"));
}
