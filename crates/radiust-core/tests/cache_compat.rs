use radiust_core::cache::Cache;
use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixture_root() -> (TempDir, Value) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/rust-migration/cache/legacy-v1");
    let metadata: Value = serde_json::from_slice(
        &fs::read(fixture.join("readback-python.json")).expect("fixture metadata"),
    )
    .expect("valid fixture metadata");
    let root = tempfile::tempdir().expect("cache root");
    fs::create_dir_all(root.path().join("objects")).expect("objects directory");
    fs::copy(fixture.join("index.sqlite"), root.path().join("index.sqlite"))
        .expect("legacy index copy");
    let relative_object = metadata["relative_object"].as_str().expect("relative object");
    fs::copy(fixture.join(relative_object), root.path().join(relative_object))
        .expect("legacy object copy");

    // The checked-in database uses this marker instead of the original
    // machine-specific absolute cache root.
    let object_path = root.path().join(relative_object);
    let connection =
        Connection::open(root.path().join("index.sqlite")).expect("open fixture index");
    connection
        .execute(
            "UPDATE entries SET path=?1 WHERE key=?2",
            rusqlite::params![object_path.to_string_lossy(), metadata["key"].as_str()],
        )
        .expect("rebase fixture path");
    drop(connection);
    (root, metadata)
}

fn key_hash(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

#[test]
fn reads_and_writes_the_python_entries_layout_and_key_hashed_object_paths() {
    let (root, metadata) = fixture_root();
    let unindexed_collision =
        root.path().join("objects").join(format!("{}.bin", key_hash("orphan-key")));
    fs::write(&unindexed_collision, b"unindexed legacy data").expect("legacy orphan");
    let cache = Cache::open(root.path()).expect("open legacy cache");
    let key = metadata["key"].as_str().expect("key");
    let expected =
        hex::decode(metadata["bytes_hex"].as_str().expect("payload hex")).expect("fixture payload");

    assert_eq!(cache.get_bytes(key).expect("read legacy object"), Some(expected));
    let fixture_entry = cache.index.get_entry(key).expect("legacy entry").expect("entry exists");
    assert_eq!(
        fixture_entry.path,
        root.path().join(metadata["relative_object"].as_str().unwrap()).to_string_lossy()
    );
    assert_eq!(fixture_entry.size_bytes, metadata["size_bytes"].as_u64().unwrap());
    assert_eq!(fixture_entry.sha256, metadata["sha256"].as_str().unwrap());

    let payload = b"written with Python-compatible paths";
    assert!(cache.put_bytes("orphan-key", b"must not replace", "object", None).is_err());
    assert_eq!(
        fs::read(&unindexed_collision).expect("orphan survives write"),
        b"unindexed legacy data"
    );
    let (_, payload_digest, path) = cache
        .put_bytes("rust-written-key", payload, "object", None)
        .expect("write legacy-layout entry");
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        format!("{}.bin", key_hash("rust-written-key"))
    );
    assert_ne!(path.file_stem().unwrap().to_str().unwrap(), payload_digest);
    assert_eq!(fs::read(&path).expect("written object"), payload);

    let connection = Connection::open(root.path().join("index.sqlite")).expect("legacy index");
    let (kind, stored_path, digest): (String, String, String) = connection
        .query_row("SELECT kind,path,sha256 FROM entries WHERE key='rust-written-key'", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("Python entry row");
    assert_eq!(kind, "object");
    assert_eq!(Path::new(&stored_path), path);
    assert_eq!(digest, payload_digest);
    let rust_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='objects'",
            [],
            |row| row.get(0),
        )
        .expect("table query");
    assert_eq!(rust_tables, 0, "opening a legacy cache must not create a Rust index");
}

#[test]
fn clear_honors_python_leases_and_preserves_unindexed_legacy_files() {
    let (root, metadata) = fixture_root();
    let legacy_key = metadata["key"].as_str().expect("key");
    let lease = root
        .path()
        .join("leases")
        .join(format!("{}.0123456789abcdef0123456789abcdef.lease", key_hash(legacy_key)));
    fs::create_dir_all(lease.parent().unwrap()).expect("leases directory");
    fs::write(&lease, b"1234").expect("Python lease marker");

    let orphan = root.path().join("objects/unindexed-legacy-object.bin");
    fs::write(&orphan, b"keep legacy object").expect("orphan object");
    let unrelated_temp = root.path().join("tmp/user-data.tmp");
    fs::create_dir_all(unrelated_temp.parent().unwrap()).expect("tmp directory");
    fs::write(&unrelated_temp, b"keep temp data").expect("unrelated temp");

    let cache = Cache::open(root.path()).expect("open legacy cache");
    let fixture_object = root.path().join(metadata["relative_object"].as_str().unwrap());
    let before = fs::read(&fixture_object).expect("leased fixture object");
    assert!(cache.put_bytes(legacy_key, b"replacement", "object", None).is_err());
    assert_eq!(fs::read(&fixture_object).expect("leased object preserved"), before);
    let gc_report = cache.gc(0).expect("GC with Python-style lease");
    assert!(!gc_report.removed.contains(&legacy_key.to_owned()));
    let report = cache.clear().expect("clear cache");

    assert!(!report.removed.contains(&legacy_key.to_owned()));
    assert!(fixture_object.exists());
    assert_eq!(fs::read(&orphan).expect("orphan survives"), b"keep legacy object");
    assert_eq!(fs::read(&unrelated_temp).expect("unrelated temp survives"), b"keep temp data");

    fs::remove_file(&lease).expect("release Python-style lease");
    let report = cache.clear().expect("clear after lease release");
    assert!(report.removed.contains(&legacy_key.to_owned()));
    assert!(!fixture_object.exists());
    assert_eq!(fs::read(&orphan).expect("orphan remains after clear"), b"keep legacy object");
}

#[test]
fn repair_drops_escaping_rows_without_touching_external_files_or_legacy_orphans() {
    let (root, metadata) = fixture_root();
    let outside = tempfile::NamedTempFile::new().expect("outside file");
    fs::write(outside.path(), b"outside cache").expect("outside content");
    let orphan = root.path().join("objects/unindexed.bin");
    fs::write(&orphan, b"keep").expect("unindexed object");
    let connection = Connection::open(root.path().join("index.sqlite")).expect("legacy index");
    connection
        .execute(
            "UPDATE entries SET path=?1 WHERE key=?2",
            rusqlite::params![outside.path().to_string_lossy(), metadata["key"].as_str()],
        )
        .expect("set escaping path");
    drop(connection);

    let cache = Cache::open(root.path()).expect("repair legacy index");
    assert!(cache.index.get(metadata["key"].as_str().unwrap()).expect("lookup").is_none());
    assert_eq!(fs::read(outside.path()).expect("outside survives"), b"outside cache");
    assert_eq!(fs::read(&orphan).expect("orphan survives repair"), b"keep");
}
