use bytes::Bytes;
use futures_util::stream;
use radiust_core::errors::CoreError;
use radiust_core::identity::ProcessingSpec;
use radiust_core::model::FrameRef;
use radiust_core::storage::object::{ObjectStore, ObjectStoreConfig};
use radiust_core::storage::{LocalCommitRequest, LocalCommitStatus, RemoteStore, StagedArtifact};
use std::time::Duration;

#[derive(Clone, Copy)]
struct FakeS3Response {
    status: &'static str,
    declared_length: usize,
    body: &'static [u8],
}

async fn fake_s3_store(
    responses: Vec<FakeS3Response>,
    cancel_after_responses: Option<tokio_util::sync::CancellationToken>,
) -> (ObjectStore, tokio::task::JoinHandle<usize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let mut requests = 0;
        for response in responses {
            let accepted = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await;
            let Ok(Ok((mut socket, _))) = accepted else {
                break;
            };
            requests += 1;
            let mut request = vec![0_u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            let headers = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.status, response.declared_length
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(response.body).await.unwrap();
            socket.shutdown().await.unwrap();
        }

        if let Some(cancellation) = cancel_after_responses {
            tokio::time::sleep(Duration::from_millis(5)).await;
            cancellation.cancel();
            if let Ok(Ok((mut socket, _))) =
                tokio::time::timeout(Duration::from_millis(100), listener.accept()).await
            {
                requests += 1;
                let mut request = vec![0_u8; 4096];
                let _ = socket.read(&mut request).await.unwrap();
                socket
                    .write_all(
                        b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                socket.shutdown().await.unwrap();
            }
        }
        requests
    });

    let endpoint = format!("http://{address}");
    let store = ObjectStore::from_config(ObjectStoreConfig {
        provider: "s3-compatible".into(),
        bucket: "radiust-test".into(),
        prefix: String::new(),
        endpoint: Some(endpoint),
        region: Some("us-east-1".into()),
        access_key_id: None,
        secret_access_key: None,
        anonymous: true,
    })
    .unwrap();
    (store, server)
}

#[tokio::test]
async fn memory_object_store_streams_and_hashes_content() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();

    let written = store
        .write_chunks("artifact.bin", vec![b"hello ".to_vec(), b"world".to_vec()], 64)
        .await
        .unwrap();
    let (payload, read) = store.read("artifact.bin", 64).await.unwrap();

    assert_eq!(payload, b"hello world");
    assert_eq!(written, read);
    assert_eq!(read.size_bytes, 11);
    assert_eq!(read.sha256, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
}

#[tokio::test]
async fn object_store_rejects_unsafe_keys_and_size_overflow() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();

    assert!(store.read("../escape", 64).await.is_err());
    assert!(store.write_chunks("too-large", vec![vec![0; 65]], 64).await.is_err());
}

#[tokio::test]
async fn streaming_object_write_hashes_chunks_and_reports_size() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();
    let chunks =
        stream::iter(vec![Ok(Bytes::from_static(b"hello ")), Ok(Bytes::from_static(b"world"))]);

    let receipt = store.write_stream("streamed.bin", chunks, 64).await.unwrap();
    let (payload, read_receipt) = store.read("streamed.bin", 64).await.unwrap();

    assert_eq!(payload, b"hello world");
    assert_eq!(receipt, read_receipt);
    assert_eq!(receipt.size_bytes, 11);
    assert_eq!(receipt.sha256, "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
}

#[tokio::test]
async fn object_store_streams_a_staged_file_and_enforces_its_size_limit() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("staged.bin");
    let payload = (0..(192 * 1024 + 17)).map(|index| (index % 251) as u8).collect::<Vec<_>>();
    std::fs::write(&source, &payload).unwrap();

    let written = store.write_path("artifact.bin", &source, 256 * 1024).await.unwrap();
    let (stored, read) = store.read("artifact.bin", 256 * 1024).await.unwrap();
    assert_eq!(stored, payload);
    assert_eq!(written, read);
    assert_eq!(written.size_bytes, payload.len() as u64);

    let oversized = store.write_path("too-large.bin", &source, 64 * 1024).await.unwrap_err();
    assert!(matches!(oversized, CoreError::ResourceLimit(_)));
    assert!(store.read_optional("too-large.bin", 256 * 1024).await.unwrap().is_none());
}

#[tokio::test]
async fn object_store_does_not_publish_a_cancelled_staged_file_upload() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("staged.bin");
    std::fs::write(&source, vec![7_u8; 128 * 1024]).unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    cancellation.cancel();

    let error = store
        .write_path_cancellable("cancelled.bin", &source, 256 * 1024, &cancellation)
        .await
        .unwrap_err();

    assert!(matches!(error, CoreError::Cancelled));
    assert!(store.read_optional("cancelled.bin", 256 * 1024).await.unwrap().is_none());
}

#[tokio::test]
async fn object_store_streams_reads_to_a_new_file_and_cleans_partial_files_on_limit() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();
    let expected = store
        .write_chunks("artifact.bin", vec![b"hello ".to_vec(), b"world".to_vec()], 64)
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("artifact.bin");

    let receipt = store.read_to_path("artifact.bin", &destination, 64).await.unwrap();
    assert_eq!(receipt, expected);
    assert_eq!(std::fs::read(&destination).unwrap(), b"hello world");

    let error =
        store.read_to_path("artifact.bin", root.path().join("limited.bin"), 8).await.unwrap_err();
    assert!(matches!(error, CoreError::ResourceLimit(_)));
    assert!(!root.path().join("limited.bin").exists());

    assert!(store.read_to_path("artifact.bin", &destination, 64).await.is_err());
    assert_eq!(std::fs::read(destination).unwrap(), b"hello world");
}

#[tokio::test]
async fn read_to_path_retries_a_temporary_partial_stream_with_fresh_staging() {
    use sha2::Digest;

    let payload = b"verified-after-retry";
    let (store, server) = fake_s3_store(
        vec![
            FakeS3Response {
                status: "200 OK",
                declared_length: b"partial-first-attempt".len() + 32,
                body: b"partial-first-attempt",
            },
            FakeS3Response { status: "200 OK", declared_length: payload.len(), body: payload },
        ],
        None,
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("readback.bin");

    let receipt = store
        .read_to_path_cancellable(
            "artifact.bin",
            &destination,
            1024,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), payload);
    assert_eq!(receipt.size_bytes, payload.len() as u64);
    assert_eq!(receipt.sha256, hex::encode(sha2::Sha256::digest(payload)));
    assert_eq!(server.await.unwrap(), 2);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn read_to_path_retry_exhaustion_removes_partial_staging() {
    let response = FakeS3Response {
        status: "200 OK",
        declared_length: b"partial-attempt".len() + 32,
        body: b"partial-attempt",
    };
    let (store, server) = fake_s3_store(vec![response; 3], None).await;
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("readback.bin");

    let error = store
        .read_to_path_cancellable(
            "artifact.bin",
            &destination,
            1024,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, CoreError::Transport(_)));
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert_eq!(server.await.unwrap(), 3);
}

#[tokio::test]
async fn read_to_path_does_not_retry_a_permanent_provider_error() {
    let response = FakeS3Response { status: "403 Forbidden", declared_length: 0, body: b"" };
    let (store, server) = fake_s3_store(vec![response; 2], None).await;
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("readback.bin");

    let error = store
        .read_to_path_cancellable(
            "artifact.bin",
            &destination,
            1024,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();

    assert!(matches!(error, CoreError::Transport(_)));
    assert!(!destination.exists());
    assert_eq!(server.await.unwrap(), 1);
}

#[tokio::test]
async fn read_to_path_cancellation_interrupts_backoff_and_cleans_partial_staging() {
    let cancellation = tokio_util::sync::CancellationToken::new();
    let (store, server) = fake_s3_store(
        vec![FakeS3Response {
            status: "200 OK",
            declared_length: b"partial-before-cancel".len() + 32,
            body: b"partial-before-cancel",
        }],
        Some(cancellation.clone()),
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("readback.bin");

    let error = store
        .read_to_path_cancellable("artifact.bin", &destination, 1024, &cancellation)
        .await
        .unwrap_err();

    assert!(matches!(error, CoreError::Cancelled));
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert_eq!(server.await.unwrap(), 1, "cancellation must prevent another read attempt");
}

#[tokio::test]
async fn streaming_object_write_aborts_when_limit_is_exceeded() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();
    let chunks = stream::iter(vec![
        Ok(Bytes::from_static(b"first")),
        Ok(Bytes::from_static(b"chunk exceeds the remaining limit")),
    ]);

    let error = store.write_stream("too-large.bin", chunks, 8).await.unwrap_err();

    assert!(matches!(error, CoreError::ResourceLimit(_)));
    assert!(store.read_optional("too-large.bin", 64).await.unwrap().is_none());
}

#[tokio::test]
async fn streaming_object_write_aborts_when_stream_fails() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();
    let chunks = stream::iter(vec![
        Ok(Bytes::from_static(b"partial")),
        Err(CoreError::Transport("upstream stream failed".into())),
    ]);

    let error = store.write_stream("interrupted.bin", chunks, 64).await.unwrap_err();

    assert!(matches!(error, CoreError::Transport(_)));
    assert!(store.read_optional("interrupted.bin", 64).await.unwrap().is_none());
}

#[tokio::test]
async fn streaming_create_only_write_preserves_existing_objects() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();

    let first = stream::iter(vec![Ok(Bytes::from_static(b"first"))]);
    let receipt = store.write_stream_once("artifact.bin", first, 64).await.unwrap();
    assert_eq!(receipt.size_bytes, 5);

    let second = stream::iter(vec![Ok(Bytes::from_static(b"second"))]);
    assert!(store.write_stream_once("artifact.bin", second, 64).await.is_err());
    assert_eq!(store.read("artifact.bin", 64).await.unwrap().0, b"first");
}

#[tokio::test]
async fn object_store_optional_reads_and_single_publish_preserve_existing_objects() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();

    assert!(store.read_optional("missing.bin", 64).await.unwrap().is_none());
    store.write_chunks_once("artifact.bin", vec![b"first".to_vec()], 64).await.unwrap();
    assert!(store.write_chunks_once("artifact.bin", vec![b"second".to_vec()], 64).await.is_err());
    assert_eq!(store.read("artifact.bin", 64).await.unwrap().0, b"first");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_only_writes_have_exactly_one_winner() {
    let operator = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
    let store = ObjectStore::from_operator(operator, "generation-a").unwrap();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(12));
    let tasks = (0..12).map(|index| {
        let store = store.clone();
        let barrier = barrier.clone();
        tokio::spawn(async move {
            barrier.wait().await;
            let payload = format!("writer-{index}").into_bytes();
            let result = store.write_chunks_once("shared.bin", vec![payload.clone()], 64).await;
            (payload, result)
        })
    });
    let results = futures_util::future::join_all(tasks).await;
    let winners: Vec<_> = results
        .into_iter()
        .map(Result::unwrap)
        .filter_map(|(payload, result)| result.ok().map(|_| payload))
        .collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(store.read("shared.bin", 64).await.unwrap().0, winners[0]);
}

#[tokio::test]
async fn object_store_retries_a_temporary_s3_read_failure() {
    use sha2::Digest;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    opendal::install_default();
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            let response = if attempt == 0 {
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice()
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nretry-body"
                    .as_slice()
            };
            socket.write_all(response).await.unwrap();
        }
    });
    let endpoint = format!("http://{address}");
    let operator = opendal::Operator::new(
        opendal::services::S3::default()
            .bucket("radiust-test")
            .endpoint(&endpoint)
            .region("us-east-1")
            .skip_signature(),
    )
    .unwrap();
    let store = ObjectStore::from_operator(operator, "").unwrap();

    let (payload, receipt) = store.read("retry.bin", 64).await.unwrap();
    assert_eq!(payload, b"retry-body");
    assert_eq!(receipt.size_bytes, 10);
    assert_eq!(receipt.sha256, hex::encode(sha2::Sha256::digest(b"retry-body")));
    server.await.unwrap();
}

#[tokio::test]
#[ignore = "opt-in S3-compatible integration; use an isolated loopback endpoint and test bucket"]
async fn s3_compatible_endpoint_streams_and_reads_back_a_large_artifact() {
    let endpoint = std::env::var("RADIUST_TEST_S3_ENDPOINT")
        .expect("set RADIUST_TEST_S3_ENDPOINT to an isolated loopback S3 endpoint");
    let endpoint_url = url::Url::parse(&endpoint).expect("S3 test endpoint must be a URL");
    let host = endpoint_url.host().expect("S3 test endpoint must have a host");
    assert!(
        matches!(host, url::Host::Ipv4(address) if address.is_loopback())
            || matches!(host, url::Host::Ipv6(address) if address.is_loopback())
            || endpoint_url.host_str() == Some("localhost"),
        "S3 integration test only permits a loopback endpoint"
    );
    let bucket = std::env::var("RADIUST_TEST_S3_BUCKET")
        .expect("set RADIUST_TEST_S3_BUCKET to a dedicated test bucket");
    assert!(
        bucket.contains("test") || bucket.contains("validation"),
        "S3 integration test bucket must be explicitly test-scoped"
    );
    let access_key_id = std::env::var("RADIUST_TEST_S3_ACCESS_KEY")
        .expect("set RADIUST_TEST_S3_ACCESS_KEY for the isolated endpoint");
    let secret_access_key = std::env::var("RADIUST_TEST_S3_SECRET_KEY")
        .expect("set RADIUST_TEST_S3_SECRET_KEY for the isolated endpoint");

    let prefix = format!("radiust-validation/{}", uuid::Uuid::new_v4());
    let store = ObjectStore::from_config(ObjectStoreConfig {
        provider: "s3-compatible".into(),
        bucket,
        prefix,
        endpoint: Some(endpoint),
        region: Some("us-east-1".into()),
        access_key_id: Some(access_key_id),
        secret_access_key: Some(secret_access_key),
        anonymous: false,
    })
    .expect("configure Rust S3-compatible object store");

    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.bin");
    let destination = root.path().join("readback.bin");
    let payload = (0..(3 * 1024 * 1024 + 29)).map(|index| (index % 251) as u8).collect::<Vec<_>>();
    std::fs::write(&source, &payload).unwrap();

    let written = store.write_path("artifact.bin", &source, 4 * 1024 * 1024).await.unwrap();
    let read = store.read_to_path("artifact.bin", &destination, 4 * 1024 * 1024).await.unwrap();
    assert_eq!(written, read);
    assert_eq!(std::fs::read(&destination).unwrap(), payload);

    let marker = b"manifest-last-marker";
    store.write_chunks_once("manifest.json", vec![marker.to_vec()], 1024).await.unwrap();
    assert_eq!(store.read("manifest.json", 1024).await.unwrap().0, marker);
    assert!(
        store
            .write_chunks_once("manifest.json", vec![b"replacement".to_vec()], 1024)
            .await
            .is_err()
    );
    assert_eq!(store.read("manifest.json", 1024).await.unwrap().0, marker);
    assert!(store.read_optional("absent.bin", 1024).await.unwrap().is_none());

    store.delete("artifact.bin").await.unwrap();
    store.delete("manifest.json").await.unwrap();
    assert!(store.read_optional("artifact.bin", 4 * 1024 * 1024).await.unwrap().is_none());

    let output_name = format!("transaction-{}.bin", uuid::Uuid::new_v4());
    let request = LocalCommitRequest {
        frame: FrameRef {
            source: "test".into(),
            product: "radar".into(),
            station: Some("loopback".into()),
            valid_time: "2026-09-30T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "loopback-s3-v1".into(),
            locator: serde_json::json!({}),
        },
        revision: "a".repeat(64),
        processing_spec: ProcessingSpec::default(),
        output_name: output_name.clone(),
        artifacts: vec![StagedArtifact {
            name: output_name.clone(),
            relative_uri: output_name.clone(),
            role: "data".into(),
            media_type: "application/octet-stream".into(),
            source: source.clone(),
        }],
        raw_complete: false,
        overwrite: false,
    };
    let remote = RemoteStore::new(store.clone(), 16 * 1024 * 1024, 16 * 1024 * 1024);
    let first = remote
        .commit_cancellable(request.clone(), tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    let second = remote
        .commit_cancellable(request, tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(first.status, LocalCommitStatus::Written);
    assert_eq!(second.status, LocalCommitStatus::Skipped);
    assert_eq!(first.manifest.generation, second.manifest.generation);

    let pointer_key = format!("{output_name}.manifest.json");
    let (pointer_bytes, pointer_receipt) = store.read(&pointer_key, 1024 * 1024).await.unwrap();
    let manifest_bytes = first.manifest.bytes().unwrap();
    assert_eq!(pointer_bytes, manifest_bytes);
    assert_eq!(pointer_receipt.size_bytes, manifest_bytes.len() as u64);
    assert_eq!(first.manifest.artifacts.len(), 1);
    let generation = first.manifest.generation.as_deref().unwrap();
    let generation_manifest_key =
        format!("_generations/{}/{generation}/manifest.json", first.manifest.output_id);
    assert_eq!(store.read(&generation_manifest_key, 1024 * 1024).await.unwrap().0, pointer_bytes);
    for artifact in &first.manifest.artifacts {
        store.delete(&artifact.relative_uri).await.unwrap();
    }
    store.delete(&generation_manifest_key).await.unwrap();
    store.delete(&pointer_key).await.unwrap();
    assert!(store.read_optional(&pointer_key, 1024 * 1024).await.unwrap().is_none());
}
