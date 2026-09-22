use radiust_core::cache::Cache;

#[test]
fn content_cache_writes_atomically_and_evicts_corruption() {
    let directory = tempfile::tempdir().expect("directory");
    let cache = Cache::open(directory.path().join("cache")).expect("cache");

    let (size, digest, path) = cache.put_bytes("frame", b"radar", "object", None).expect("put");

    assert_eq!(size, 5);
    assert_eq!(digest.len(), 64);
    assert_eq!(cache.get_bytes("frame").expect("get").as_deref(), Some(b"radar".as_slice()));
    std::fs::write(&path, b"changed").expect("corrupt");
    assert!(cache.get_bytes("frame").expect("corrupt lookup").is_none());
    assert!(cache.index.get("frame").expect("index lookup").is_none());
}

#[test]
fn gc_removes_expired_and_lru_entries_but_skips_leased_keys() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path().join("cache");
    let cache = Cache::open(&root).expect("cache");
    let old_path = root.join("objects").join("old.bin");
    let leased_path = root.join("objects").join("leased.bin");
    std::fs::write(&old_path, b"old").expect("old file");
    std::fs::write(&leased_path, b"held").expect("leased file");
    cache.index.put("old", old_path.to_str().expect("path"), 3, "old", Some(0)).expect("old index");
    cache
        .index
        .put("leased", leased_path.to_str().expect("path"), 4, "held", None)
        .expect("lease index");

    let lease = cache.lease("leased").expect("lease");
    let report = cache.gc(0).expect("gc");

    assert!(report.removed.contains(&"old".to_string()));
    assert!(!report.removed.contains(&"leased".to_string()));
    assert!(!old_path.exists());
    assert!(leased_path.exists());
    drop(lease);
}

#[test]
fn updating_one_key_preserves_blob_shared_by_another_key() {
    let directory = tempfile::tempdir().expect("directory");
    let cache = Cache::open(directory.path().join("cache")).expect("cache");
    cache.put_bytes("a", b"shared", "object", None).expect("put a");
    cache.put_bytes("b", b"shared", "object", None).expect("put b");

    cache.put_bytes("a", b"changed", "object", None).expect("update a");

    assert_eq!(cache.get_bytes("b").expect("get b").as_deref(), Some(b"shared".as_slice()));
}

#[test]
fn gc_preserves_shared_blob_when_another_key_is_leased() {
    let directory = tempfile::tempdir().expect("directory");
    let cache = Cache::open(directory.path().join("cache")).expect("cache");
    cache.put_bytes("a", b"shared", "object", None).expect("put a");
    cache.put_bytes("b", b"shared", "object", None).expect("put b");
    let lease = cache.lease("b").expect("lease b");

    cache.gc(0).expect("gc");

    assert_eq!(cache.get_bytes("b").expect("get b").as_deref(), Some(b"shared".as_slice()));
    drop(lease);
}

#[test]
fn reopening_relative_cache_root_does_not_delete_indexed_blobs() {
    let directory = tempfile::tempdir_in(".").expect("directory");
    let root = std::path::PathBuf::from(directory.path().file_name().expect("name")).join("cache");
    {
        let cache = Cache::open(&root).expect("cache");
        cache.put_bytes("frame", b"radar", "object", None).expect("put");
    }

    let reopened = Cache::open(&root).expect("reopen");
    assert_eq!(reopened.get_bytes("frame").expect("get").as_deref(), Some(b"radar".as_slice()));
}
