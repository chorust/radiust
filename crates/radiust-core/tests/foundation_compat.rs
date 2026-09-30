use radiust_core::cache::Cache;
use radiust_core::identity::{
    ProcessingSpec, logical_id, output_id, processing_hash, resolved_revision, safe_ref,
};
use radiust_core::model::FrameRef;
use radiust_core::storage::local::RootLock;
use radiust_core::storage::manifest::{is_complete, read_manifest};
use serde_json::Value;
use sha2::Digest;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust-migration")
}

#[test]
fn python_identity_and_v1_output_fixtures_are_readable_by_rust() {
    let output = fixture_root().join("output");
    let identity: Value =
        serde_json::from_slice(&fs::read(output.join("identity.json")).expect("identity fixture"))
            .expect("valid identity fixture");
    let mut frame_value = identity["frame_identity"].clone();
    frame_value["logical_id"] = identity["logical_id"].clone();
    frame_value["revision"] = "upstream-42".into();
    let frame: FrameRef = serde_json::from_value(frame_value).expect("fixture frame");
    assert_eq!(logical_id(&frame).unwrap(), identity["logical_id"]);
    assert_eq!(safe_ref(&frame).unwrap(), identity["safe_ref"]);

    let processing: ProcessingSpec =
        serde_json::from_value(identity["processing_identity"].clone())
            .expect("fixture processing spec");
    assert_eq!(processing_hash(&processing).unwrap(), identity["processing_hash"]);
    assert_eq!(
        output_id(
            &frame,
            identity["resolved_revision_from_artifacts"].as_str().unwrap(),
            &processing,
        )
        .unwrap(),
        identity["output_id"]
    );
    assert_eq!(
        resolved_revision(&[], Some("upstream-42")).unwrap(),
        identity["resolved_revision_upstream"]
    );

    let manifest_root = output.join("legacy-v1");
    let manifest_path = manifest_root.join("frame.bin.manifest.json");
    let manifest = read_manifest(&manifest_path).unwrap().expect("legacy v1 manifest");
    manifest.validate().unwrap();
    assert!(is_complete(&manifest_root, &manifest));
}

#[test]
fn rust_reads_the_python_entries_cache_fixture_and_checks_its_digest() {
    let source = fixture_root().join("cache/legacy-v1");
    let metadata: Value = serde_json::from_slice(
        &fs::read(source.join("readback-python.json")).expect("cache fixture metadata"),
    )
    .expect("valid cache metadata");
    let root = tempfile::tempdir().expect("cache root");
    fs::create_dir_all(root.path().join("objects")).unwrap();
    fs::copy(source.join("index.sqlite"), root.path().join("index.sqlite")).unwrap();
    let relative = metadata["relative_object"].as_str().unwrap();
    fs::copy(source.join(relative), root.path().join(relative)).unwrap();
    let object_path = root.path().join(relative);
    let connection = rusqlite::Connection::open(root.path().join("index.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE entries SET path=?1 WHERE key=?2",
            rusqlite::params![object_path.to_string_lossy(), metadata["key"].as_str()],
        )
        .unwrap();
    drop(connection);

    let cache = Cache::open(root.path()).unwrap();
    let key = metadata["key"].as_str().unwrap();
    let bytes = cache.get_bytes(key).unwrap().expect("Python cache entry");
    assert_eq!(bytes, hex::decode(metadata["bytes_hex"].as_str().unwrap()).unwrap());
    assert_eq!(hex::encode(sha2::Sha256::digest(&bytes)), metadata["sha256"].as_str().unwrap());
}

#[cfg(unix)]
#[test]
fn rust_output_lock_excludes_a_python_flock_writer() {
    let Ok(root) = tempfile::tempdir() else {
        panic!("temporary lock root should be available");
    };
    let Ok(_lock) = RootLock::acquire(root.path()) else {
        panic!("Rust should acquire a fresh output lock");
    };
    let lock_path = root.path().join(".radiust.lock");
    let script = "import fcntl, os, sys; fd=os.open(sys.argv[1], os.O_RDWR);\ntry: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError: print('locked')\nelse: raise SystemExit('Python unexpectedly acquired the Rust lock')";
    let output = Command::new("python3").arg("-c").arg(script).arg(&lock_path).output();
    let Ok(output) = output else {
        // Rust production and test targets can run without Python installed.
        return;
    };
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "locked");
}
