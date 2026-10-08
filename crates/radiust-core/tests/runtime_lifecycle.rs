use radiust_core::cache::Cache;
use radiust_core::errors::CoreError;
use radiust_core::limits::Limits;
use radiust_core::runtime::RuntimeManager;
use radiust_core::transport::HttpTransport;
use std::io::Read;

#[test]
fn request_budget_can_be_cancelled_without_sleeping() {
    let limits = Limits { request_concurrency: 1, ..Limits::default() };
    let manager = RuntimeManager::new(limits).expect("runtime");
    manager.block_on(async {
        let _permit = manager.budget.acquire_request().await.expect("permit");
        let second = manager.budget.clone();
        let task = tokio::spawn(async move { second.acquire_request().await });
        manager.cancel();
        assert!(task.await.expect("join").is_err());
    });
}

#[test]
fn host_budget_is_shared_and_case_insensitive() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let limits = Limits { host_concurrency: 1, ..Limits::default() };
    let manager = RuntimeManager::new(limits).expect("runtime");
    manager.block_on(async {
        let first = manager.budget.acquire_host("Radar.Example").await.expect("first permit");
        let acquired = Arc::new(AtomicBool::new(false));
        let budget = manager.budget.clone();
        let second_acquired = acquired.clone();
        let second = tokio::spawn(async move {
            let _permit = budget.acquire_host("radar.example").await.expect("second permit");
            second_acquired.store(true, Ordering::SeqCst);
        });
        tokio::task::yield_now().await;
        assert!(!acquired.load(Ordering::SeqCst));
        drop(first);
        second.await.expect("second request completes");
        assert!(acquired.load(Ordering::SeqCst));
    });
}

#[test]
fn temp_owner_keeps_data_until_drop_and_cache_has_index() {
    let directory = tempfile::tempdir().expect("directory");
    let mut owner = tempfile::NamedTempFile::new_in(directory.path()).expect("owner");
    std::io::Write::write_all(&mut owner, b"radar").expect("write");
    let (size, digest) = radiust_core::storage::manifest::hash_file(owner.path(), 5).unwrap();
    assert_eq!(size, 5);
    let mut file = std::fs::File::open(owner.path()).expect("file");
    let mut content = Vec::new();
    file.read_to_end(&mut content).expect("read");
    assert_eq!(content, b"radar");
    let cache = Cache::open(directory.path().join("cache")).expect("cache");
    cache.index.put("key", "object", 5, &digest, None).expect("index");
    assert!(cache.index.get("key").expect("lookup").is_some());
}

#[test]
fn limits_reject_oversized_payloads() {
    let limits = Limits { max_artifact_bytes: 2, ..Limits::default() };
    assert!(limits.validate_bytes(3, 3).is_err());
    assert!(limits.validate_pixels(1).is_ok());
}

#[test]
fn public_network_is_rejected_before_rust_io() {
    let limits = Limits::default();
    let manager = RuntimeManager::new(limits.clone()).expect("runtime");
    manager.block_on(async {
        let transport = HttpTransport::new(limits, false).expect("transport");
        let error = transport
            .get_bytes("https://example.com/data.png")
            .await
            .expect_err("network must be gated");
        assert!(matches!(error, CoreError::NetworkDisabled(_)));
    });
}

#[test]
fn redirect_destination_must_pass_rust_network_policy() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let limits = Limits::default();
    let manager = RuntimeManager::new(limits.clone()).expect("runtime");
    manager.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0_u8; 1024];
            let _ = socket.read(&mut buffer).await.expect("read");
            socket
                .write_all(b"HTTP/1.1 302 Found\r\nLocation: http://0.0.0.0:9/blocked\r\nContent-Length: 0\r\n\r\n")
                .await
                .expect("write");
        });
        let transport = HttpTransport::new(limits, false).expect("transport");
        let error = transport
            .get_bytes(&format!("http://{address}/redirect"))
            .await
            .expect_err("redirect must be gated");
        assert!(matches!(error, CoreError::NetworkDisabled(_)));
        server.await.expect("server");
    });
}
