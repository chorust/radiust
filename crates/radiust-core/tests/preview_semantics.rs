use futures_util::future::BoxFuture;
use radiust_core::config::CoreConfig;
use radiust_core::engine::{Engine, EngineError};
use radiust_core::errors::CoreResult;
use radiust_core::identity::logical_id;
use radiust_core::limits::Limits;
use radiust_core::model::{
    ArtifactReceipt, DiscoveryStatus, DiscoveryTarget, FrameRef, PreviewMode, Query, RawArtifact,
    RawFrame,
};
use radiust_core::preview::{preview_artifact, preview_bytes, preview_file};
use radiust_core::source::{SourceAdapter, SourceContext, SourceRegistry};
use serde_json::Value;
use sha2::Digest;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const FIXTURE: &[u8] =
    include_bytes!("../../../tests/fixtures/sources/au/raw/IDR021.T.202609180511.png");
const LEGACY_DISPLAY_FIXTURE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/legacy-display/au/old-gray.png");

#[test]
fn byte_and_file_preview_preserve_every_raw_pixel_and_unknown_identity() {
    let from_bytes = preview_bytes(FIXTURE, &Limits::default()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("IDR021.T.209912312359.png");
    std::fs::write(&path, FIXTURE).unwrap();
    let from_file = preview_file(&path, &Limits::default()).unwrap();

    assert_eq!((from_file.preview.width, from_file.preview.height), (512, 512));
    assert_eq!(from_file.format, "PNG");
    assert_eq!(from_file.sha256, from_bytes.sha256);
    assert_eq!(from_file.preview.rgba, from_bytes.preview.rgba);
    assert_eq!(
        hex::encode(sha2::Sha256::digest(&from_file.preview.rgba)),
        "1297841e50e05f7e69274ffcc5bdd10be3fc34a1473dd818fbd04dfb4ea0de21"
    );
    assert_eq!(from_bytes.preview.mode, PreviewMode::Raw);
    assert!(from_bytes.preview.frame.is_none());
    assert!(from_bytes.preview.rule_version.is_none());
    assert_eq!(from_file.preview.mode, PreviewMode::Raw);
    assert!(from_file.preview.frame.is_none());
    assert!(from_file.preview.rule_version.is_none());
}

#[test]
fn preview_preserves_every_pixel_of_the_pinned_legacy_display_fixture() {
    let preview = preview_file(LEGACY_DISPLAY_FIXTURE, &Limits::default()).unwrap();

    assert_eq!((preview.preview.width, preview.preview.height), (461, 461));
    assert_eq!(preview.format, "PNG");
    assert_eq!(preview.preview.mode, PreviewMode::Raw);
    assert_eq!(
        hex::encode(sha2::Sha256::digest(&preview.preview.rgba)),
        "aa4d83a7b3245a0de4ba56b0f570917993db748110ba441c4813201039f8c08d"
    );
}

#[test]
fn preview_rejects_encoded_byte_and_decoded_pixel_limits() {
    let byte_limited =
        Limits { max_artifact_bytes: (FIXTURE.len() - 1) as u64, ..Limits::default() };
    assert!(preview_bytes(FIXTURE, &byte_limited).is_err());

    let pixel_limited = Limits { max_pixels: 512 * 512 - 1, ..Limits::default() };
    assert!(preview_bytes(FIXTURE, &pixel_limited).is_err());

    let decode_memory_limited = Limits { max_temp_bytes: 12 * 512 * 512 - 1, ..Limits::default() };
    assert!(matches!(
        preview_bytes(FIXTURE, &decode_memory_limited),
        Err(radiust_core::errors::CoreError::ResourceLimit(_))
    ));

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("raw.png");
    std::fs::write(&path, FIXTURE).unwrap();
    assert!(matches!(
        preview_file(&path, &decode_memory_limited),
        Err(radiust_core::errors::CoreError::ResourceLimit(_))
    ));
}

#[test]
fn acquired_image_preview_requires_a_matching_source_receipt() {
    let digest = hex::encode(sha2::Sha256::digest(FIXTURE));
    let receipt = ArtifactReceipt {
        name: "frame.png".into(),
        media_type: "image/png".into(),
        size_bytes: FIXTURE.len() as u64,
        sha256: digest.clone(),
    };
    let valid = preview_artifact(&source_artifact(receipt.clone()), &Limits::default()).unwrap();
    assert_eq!(valid.sha256, digest);
    assert_eq!(valid.size_bytes, FIXTURE.len() as u64);

    let mut bad_digest = receipt.clone();
    bad_digest.sha256 = "0".repeat(64);
    assert!(matches!(
        preview_artifact(&source_artifact(bad_digest), &Limits::default()),
        Err(radiust_core::errors::CoreError::Transport(message))
            if message == "raw image preview artifact does not match its receipt"
    ));

    let mut bad_size = receipt.clone();
    bad_size.size_bytes += 1;
    assert!(matches!(
        preview_artifact(&source_artifact(bad_size), &Limits::default()),
        Err(radiust_core::errors::CoreError::Transport(message))
            if message == "raw image preview artifact does not match its receipt"
    ));

    let mut wrong_media_type = receipt;
    wrong_media_type.media_type = "application/octet-stream".into();
    assert!(matches!(
        preview_artifact(&source_artifact(wrong_media_type), &Limits::default()),
        Err(radiust_core::errors::CoreError::Transport(message))
            if message == "raw image preview artifact media type is not an image"
    ));
}

fn source_artifact(receipt: ArtifactReceipt) -> RawArtifact {
    let temporary = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temporary.path(), FIXTURE).unwrap();
    RawArtifact { receipt, path: temporary.into_temp_path() }
}

#[test]
fn malformed_local_bytes_are_not_guessed_as_a_scientific_image() {
    assert!(preview_bytes(b"not an image", &Limits::default()).is_err());
}

#[tokio::test]
async fn science_decode_refuses_sources_without_validated_science_evidence() {
    let engine = Engine::new(CoreConfig::default(), SourceRegistry::default()).unwrap();
    let frame = FrameRef {
        source: "au".into(),
        product: "composite".into(),
        station: None,
        valid_time: "2026-09-18T05:11:00Z".into(),
        base_time: None,
        logical_id: "raw-only-fixture".into(),
        revision: None,
        locator_version: "fixture-v1".into(),
        locator: Value::Null,
    };
    let raw = RawFrame { frame, artifacts: Vec::new(), private_locator: None };

    let error = engine.decode_science(Arc::new(raw)).await.unwrap_err();
    assert!(matches!(error, EngineError::UnsupportedScience(source) if source == "au/composite"));
}

struct CountingFixtureAdapter {
    candidates: Vec<FrameRef>,
    fetch_count: Arc<AtomicUsize>,
    fetched_ids: Arc<Mutex<Vec<String>>>,
}

impl SourceAdapter for CountingFixtureAdapter {
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
        _temp_root: PathBuf,
    ) -> Option<BoxFuture<'static, CoreResult<RawFrame>>> {
        let fetch_count = self.fetch_count.clone();
        let fetched_ids = self.fetched_ids.clone();
        Some(Box::pin(async move {
            fetch_count.fetch_add(1, Ordering::SeqCst);
            fetched_ids.lock().unwrap().push(frame.logical_id.clone());
            Ok(RawFrame { frame, artifacts: Vec::new(), private_locator: None })
        }))
    }
}

fn fixture_frame(valid_time: &str) -> FrameRef {
    let mut frame = FrameRef {
        source: "au".into(),
        product: "composite".into(),
        station: None,
        valid_time: valid_time.into(),
        base_time: None,
        logical_id: String::new(),
        revision: None,
        locator_version: "preview-fixture-v1".into(),
        locator: Value::Null,
    };
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

#[tokio::test]
async fn latest_selection_fetches_only_the_selected_frame_once() {
    let candidates =
        vec![fixture_frame("2026-09-18T05:00:00Z"), fixture_frame("2026-09-18T05:11:00Z")];
    let selected_id = candidates[1].logical_id.clone();
    let fetch_count = Arc::new(AtomicUsize::new(0));
    let fetched_ids = Arc::new(Mutex::new(Vec::new()));
    let adapter = CountingFixtureAdapter {
        candidates,
        fetch_count: fetch_count.clone(),
        fetched_ids: fetched_ids.clone(),
    };
    let mut registry = SourceRegistry::default();
    registry.register(Arc::new(adapter)).unwrap();
    let mut config = CoreConfig::default();
    // The injected adapter is fully offline; Engine requires this capability
    // flag before it dispatches any raw acquisition hook.
    config.runtime.allow_network = true;
    let engine = Engine::new(config, registry).unwrap();

    let report = engine
        .discover_seeded(
            Query {
                source: Some("au".into()),
                product: Some("composite".into()),
                ..Query::default()
            },
            1,
        )
        .await
        .unwrap();
    assert_eq!(report.counts.success, 1);
    let selected = report
        .items
        .iter()
        .find(|item| item.status == DiscoveryStatus::Success)
        .and_then(|item| item.frame.clone())
        .unwrap();
    assert_eq!(selected.logical_id, selected_id);

    let fetched = engine.fetch_raw(selected).await.unwrap();

    assert_eq!(fetched.frame.logical_id, selected_id);
    assert_eq!(fetch_count.load(Ordering::SeqCst), 1);
    assert_eq!(*fetched_ids.lock().unwrap(), vec![selected_id]);
}
