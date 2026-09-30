use radiust_core::errors::CoreError;
use radiust_core::identity::ProcessingSpec;
use radiust_core::limits::Limits;
use radiust_core::model::FrameRef;
use radiust_core::storage::local::RootLock;
use radiust_core::storage::manifest::{is_complete, read_manifest};
use radiust_core::storage::{LocalCommitRequest, LocalCommitStatus, LocalStore, StagedArtifact};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::thread;

fn frame() -> FrameRef {
    FrameRef {
        source: "test".into(),
        product: "radar".into(),
        station: Some("KXYZ".into()),
        valid_time: "2026-09-26T00:00:00Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: None,
        locator_version: "1".into(),
        locator: serde_json::json!({}),
    }
}

fn request(source: &Path, revision: &str, overwrite: bool) -> LocalCommitRequest {
    LocalCommitRequest {
        frame: frame(),
        revision: revision.into(),
        processing_spec: ProcessingSpec::default(),
        output_name: "frame.bin".into(),
        artifacts: vec![StagedArtifact {
            name: "frame.bin".into(),
            relative_uri: "frame.bin".into(),
            role: "data".into(),
            media_type: "application/octet-stream".into(),
            source: source.to_path_buf(),
        }],
        raw_complete: false,
        overwrite,
    }
}

fn request_with_two_artifacts(
    source: &Path,
    revision: &str,
    overwrite: bool,
) -> LocalCommitRequest {
    let mut request = request(source, revision, overwrite);
    request.artifacts.push(StagedArtifact {
        name: "metadata.bin".into(),
        relative_uri: "metadata.bin".into(),
        role: "metadata".into(),
        media_type: "application/octet-stream".into(),
        source: source.to_path_buf(),
    });
    request
}

#[test]
fn concurrent_handles_commit_one_complete_output() {
    let output = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let source = inputs.path().join("source.bin");
    fs::write(&source, b"shared output").unwrap();

    let first_store = LocalStore::new(output.path(), Limits::default()).unwrap();
    let second_store = LocalStore::new(output.path(), Limits::default()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let first_barrier = barrier.clone();
    let first_source = source.clone();
    let first = thread::spawn(move || {
        first_barrier.wait();
        first_store.commit(request_with_two_artifacts(&first_source, &"a".repeat(64), false))
    });
    let second_barrier = barrier.clone();
    let second_source = source.clone();
    let second = thread::spawn(move || {
        second_barrier.wait();
        second_store.commit(request_with_two_artifacts(&second_source, &"a".repeat(64), false))
    });

    let results = [first.join().unwrap().unwrap(), second.join().unwrap().unwrap()];
    let written = results
        .iter()
        .filter(|result| result.status == LocalCommitStatus::Written)
        .collect::<Vec<_>>();
    let skipped = results
        .iter()
        .filter(|result| result.status == LocalCommitStatus::Skipped)
        .collect::<Vec<_>>();
    assert_eq!(written.len(), 1);
    assert_eq!(skipped.len(), 1);
    assert_eq!(written[0].manifest.generation, skipped[0].manifest.generation);

    let manifest_path = output.path().join("frame.bin.manifest.json");
    let manifest = read_manifest(&manifest_path).unwrap().expect("published manifest");
    assert_eq!(manifest.generation, written[0].manifest.generation);
    assert!(is_complete(output.path(), &manifest));
    let artifact_paths = manifest
        .artifacts
        .iter()
        .map(|artifact| artifact.relative_uri.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(artifact_paths, ["frame.bin", "metadata.bin"].into_iter().collect());
    for artifact in &manifest.artifacts {
        assert_eq!(fs::read(output.path().join(&artifact.relative_uri)).unwrap(), b"shared output");
    }
}

#[test]
fn local_lock_conflict_rejects_commit_and_allows_retry_after_release() {
    let output = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let source = inputs.path().join("source.bin");
    fs::write(&source, b"lock conflict output").unwrap();
    let store = LocalStore::new(output.path(), Limits::default()).unwrap();
    let commit_request = request(&source, &"a".repeat(64), false);

    // Hold the public local root lock so commit exercises the actual lock
    // conflict path instead of an injected failure hook.
    let held_lock = RootLock::acquire(output.path()).unwrap();

    let conflict = store.commit(commit_request.clone());
    assert!(matches!(
        conflict,
        Err(CoreError::Storage(message)) if message.starts_with("output root locked:")
    ));
    assert!(!output.path().join("frame.bin").exists());
    assert!(!output.path().join("frame.bin.manifest.json").exists());

    drop(held_lock);

    let written = store.commit(commit_request).unwrap();
    assert_eq!(written.status, LocalCommitStatus::Written);
    assert!(is_complete(output.path(), &written.manifest));
    assert_eq!(fs::read(output.path().join("frame.bin")).unwrap(), b"lock conflict output");
}

#[test]
fn complete_old_artifact_is_skipped_and_damaged_artifact_is_repaired() {
    let output = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let source = inputs.path().join("source.bin");
    fs::write(&source, b"verified output").unwrap();
    let store = LocalStore::new(output.path(), Limits::default()).unwrap();
    let commit_request = request(&source, &"a".repeat(64), false);

    let first = store.commit(commit_request.clone()).unwrap();
    let skipped = store.commit(commit_request.clone()).unwrap();
    assert_eq!(skipped.status, LocalCommitStatus::Skipped);
    assert_eq!(skipped.manifest.generation, first.manifest.generation);
    assert_eq!(fs::read(output.path().join("frame.bin")).unwrap(), b"verified output");

    fs::write(output.path().join("frame.bin"), b"stale bytes").unwrap();
    let repaired = store.commit(commit_request).unwrap();
    assert_eq!(repaired.status, LocalCommitStatus::Written);
    assert_ne!(repaired.manifest.generation, first.manifest.generation);
    assert_eq!(fs::read(output.path().join("frame.bin")).unwrap(), b"verified output");
    let manifest = read_manifest(&output.path().join("frame.bin.manifest.json"))
        .unwrap()
        .expect("repaired manifest is published");
    assert_eq!(manifest.generation, repaired.manifest.generation);
    assert!(is_complete(output.path(), &manifest));
}

#[test]
fn failed_overwrite_with_missing_source_preserves_output_and_cleans_stage() {
    let output = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let source = inputs.path().join("source.bin");
    fs::write(&source, b"prior output").unwrap();
    let store = LocalStore::new(output.path(), Limits::default()).unwrap();
    let initial = store.commit(request(&source, &"a".repeat(64), false)).unwrap();

    let manifest_path = output.path().join("frame.bin.manifest.json");
    let original_manifest = fs::read(&manifest_path).unwrap();
    let original_artifact = fs::read(output.path().join("frame.bin")).unwrap();
    let missing_source = inputs.path().join("missing.bin");
    assert!(!missing_source.exists());

    let failure = store.commit(request(&missing_source, &"b".repeat(64), true));
    assert!(failure.is_err());
    assert_eq!(fs::read(&manifest_path).unwrap(), original_manifest);
    assert_eq!(fs::read(output.path().join("frame.bin")).unwrap(), original_artifact);

    let manifest = read_manifest(&manifest_path).unwrap().expect("prior manifest remains");
    assert_eq!(manifest.generation, initial.manifest.generation);
    assert!(is_complete(output.path(), &manifest));
    let stage_remains = fs::read_dir(output.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with(".radiust-stage-"));
    assert!(!stage_remains);
}

#[test]
fn failed_copy_cleans_partially_written_staged_artifact() {
    const LIMIT: u64 = 64 * 1024;
    let output = tempfile::tempdir().unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let source = inputs.path().join("source.bin");
    fs::write(&source, vec![0x5a; (LIMIT * 2) as usize]).unwrap();
    let limits = Limits { max_artifact_bytes: LIMIT, ..Limits::default() };
    let store = LocalStore::new(output.path(), limits).unwrap();

    let failure = store.commit(request(&source, &"a".repeat(64), false));
    assert!(matches!(failure, Err(CoreError::ResourceLimit(_))));
    assert!(!output.path().join("frame.bin").exists());
    assert!(!output.path().join("frame.bin.manifest.json").exists());
    let stage_remains = fs::read_dir(output.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with(".radiust-stage-"));
    assert!(!stage_remains, "failed copy must remove its partial staged artifact");
}
