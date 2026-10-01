use base64::Engine as _;
use radiust_core::errors::CoreError;
use radiust_core::limits::{Limits, RequestBudget};
use radiust_core::transport::{HttpGetPolicy, HttpRequestCoalescer, HttpTransport};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn read_request(stream: &mut (impl AsyncRead + Unpin)) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 512];
    loop {
        let count = stream.read(&mut buffer).await.unwrap();
        assert_ne!(count, 0, "request ended before headers were complete");
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn head_metadata_follows_checked_redirects_and_returns_only_selected_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut redirect, _) = listener.accept().await.unwrap();
        let first = read_request(&mut redirect).await;
        assert!(first.starts_with("HEAD /redirect HTTP/1.1\r\n"));
        redirect
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /metadata\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();

        let (mut metadata, _) = listener.accept().await.unwrap();
        let second = read_request(&mut metadata).await;
        assert!(second.starts_with("HEAD /metadata HTTP/1.1\r\n"));
        metadata
            .write_all(
                b"HTTP/1.1 200 OK\r\nLast-Modified: Mon, 29 Dec 2025 06:59:01 GMT\r\nContent-Type: image/png\r\nSet-Cookie: session=secret\r\nContent-Length: 128\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    });

    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let result = transport.head_metadata(&format!("http://{address}/redirect")).await.unwrap();
    server.await.unwrap();

    assert_eq!(result.status, 200);
    assert_eq!(result.headers["last-modified"], "Mon, 29 Dec 2025 06:59:01 GMT");
    assert_eq!(result.headers["content-type"], "image/png");
    assert!(!result.headers.contains_key("set-cookie"));
}

#[tokio::test]
async fn head_metadata_rejects_public_redirects_when_network_is_disabled() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: https://example.invalid/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    });

    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let error = transport.head_metadata(&format!("http://{address}/redirect")).await.unwrap_err();
    server.await.unwrap();

    assert!(matches!(error, CoreError::NetworkDisabled(_)));
}

#[tokio::test]
async fn get_can_send_provider_headers_through_the_shared_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await.to_ascii_lowercase();
        assert!(request.starts_with("get /timeline http/1.1\r\n"));
        assert!(request.contains("referer: https://provider.invalid/radar\r\n"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let body = transport
        .get_bytes_with_headers(
            &format!("http://{address}/timeline"),
            &[("referer", "https://provider.invalid/radar")],
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(body, b"payload");
}

#[tokio::test]
async fn post_form_uses_encoded_body_shared_transport_and_provider_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        let (headers, request_body) = request.split_once("\r\n\r\n").unwrap();
        let mut body = request_body.as_bytes().to_vec();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while body.len() < content_length {
            let mut remainder = vec![0; content_length - body.len()];
            let read = stream.read(&mut remainder).await.unwrap();
            assert_ne!(read, 0);
            body.extend_from_slice(&remainder[..read]);
        }
        let header_text = headers.to_ascii_lowercase();
        assert!(header_text.starts_with("post /timeline http/1.1\r\n"));
        assert!(header_text.contains("content-type: application/x-www-form-urlencoded\r\n"));
        assert!(header_text.contains("authorization: bearer fixture-token\r\n"));
        assert!(std::str::from_utf8(&body).unwrap().starts_with("name=two+words&csrf=a%2Bb"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let body = transport
        .post_form_bytes_with_headers(
            &format!("http://{address}/timeline"),
            &[("name", "two words"), ("csrf", "a+b")],
            &[("authorization", "Bearer fixture-token")],
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(body, b"payload");
}

#[tokio::test]
async fn get_retries_a_temporary_server_failure_and_then_returns_the_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut first).await;
        first
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        let (mut second, _) = listener.accept().await.unwrap();
        let request = read_request(&mut second).await;
        assert!(request.starts_with("GET /retry HTTP/1.1\r\n"));
        second
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let body = transport.get_bytes(&format!("http://{address}/retry")).await.unwrap();
    server.await.unwrap();

    assert_eq!(body, b"payload");
}

#[tokio::test]
async fn get_does_not_forward_caller_headers_across_origins() {
    let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination_address = destination.local_addr().unwrap();
    let destination_server = tokio::spawn(async move {
        let (mut stream, _) = destination.accept().await.unwrap();
        let request = read_request(&mut stream).await.to_ascii_lowercase();
        assert!(!request.contains("authorization:"));
        assert!(!request.contains("referer:"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let redirect = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect_address = redirect.local_addr().unwrap();
    let redirect_server = tokio::spawn(async move {
        let (mut stream, _) = redirect.accept().await.unwrap();
        let request = read_request(&mut stream).await.to_ascii_lowercase();
        assert!(request.contains("authorization: bearer private-token\r\n"));
        assert!(request.contains("referer: https://provider.invalid/radar\r\n"));
        let response = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{destination_address}/final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let transport = HttpTransport::new(Limits::default(), true).unwrap();
    let body = transport
        .get_bytes_with_headers(
            &format!("http://{redirect_address}/redirect"),
            &[
                ("authorization", "Bearer private-token"),
                ("referer", "https://provider.invalid/radar"),
            ],
        )
        .await
        .unwrap();
    redirect_server.await.unwrap();
    destination_server.await.unwrap();

    assert_eq!(body, b"payload");
}

#[tokio::test]
async fn streamed_get_publishes_atomically_and_returns_a_digest() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await.to_ascii_lowercase();
        assert!(request.starts_with("get /artifact http/1.1\r\n"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested").join("frame.bin");
    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let receipt = transport
        .get_to_path_with_headers(&format!("http://{address}/artifact"), &[], &path)
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(std::fs::read(path).unwrap(), b"payload");
    assert_eq!(receipt.size_bytes, 7);
    assert_eq!(receipt.sha256, "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5");
}

#[tokio::test]
async fn streamed_get_removes_partial_files_when_the_size_limit_is_exceeded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("frame.bin");
    let mut limits = Limits::default();
    limits.max_artifact_bytes = 4;
    let transport = HttpTransport::new(limits, false).unwrap();
    let error = transport
        .get_to_path_with_headers(&format!("http://{address}/artifact"), &[], &path)
        .await
        .unwrap_err();
    server.await.unwrap();

    assert!(matches!(error, CoreError::ResourceLimit(_)));
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn streamed_get_cancellation_removes_the_partial_staging_file() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (headers_sent_tx, headers_sent_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 20\r\nConnection: close\r\n\r\npartial")
            .await
            .unwrap();
        let _ = headers_sent_tx.send(());
        let _ = release_rx.await;
    });

    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("frame.bin");
    let limits = Limits::default();
    let budget = Arc::new(RequestBudget::new(&limits));
    let transport = Arc::new(HttpTransport::with_budget(limits, false, budget).unwrap());
    let worker_transport = transport.clone();
    let url = format!("http://{address}/stream");
    let worker = tokio::spawn(async move {
        worker_transport.get_to_path_with_headers(&url, &[], destination).await
    });

    headers_sent_rx.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let mut entries = tokio::fs::read_dir(directory.path()).await.unwrap();
            if let Some(entry) = entries.next_entry().await.unwrap()
                && entry.metadata().await.unwrap().len() == 7
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("partial body reaches staging file before cancellation");
    transport.cancel();
    let error = worker.await.unwrap().unwrap_err();
    let _ = release_tx.send(());
    server.await.unwrap();

    assert!(matches!(error, CoreError::Cancelled));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn streamed_get_honors_a_remaining_frame_byte_budget_before_staging() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("frame.bin");
    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let error = transport
        .get_to_path_same_origin_limited_with_headers(
            &format!("http://{address}/artifact"),
            &[],
            &path,
            4,
        )
        .await
        .unwrap_err();
    server.await.unwrap();

    assert!(matches!(error, CoreError::ResourceLimit(_)));
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn streamed_artifact_rejects_cross_origin_redirect_before_contacting_destination() {
    let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination_address = destination.local_addr().unwrap();
    let destination_server = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_millis(150), destination.accept())
            .await
            .is_ok()
    });

    let redirect = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect_address = redirect.local_addr().unwrap();
    let redirect_server = tokio::spawn(async move {
        let (mut stream, _) = redirect.accept().await.unwrap();
        let request = read_request(&mut stream).await.to_ascii_lowercase();
        assert!(request.starts_with("get /artifact http/1.1\r\n"));
        let response = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://{destination_address}/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("frame.bin");
    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let error = transport
        .get_to_path_same_origin_with_headers(
            &format!("http://{redirect_address}/artifact"),
            &[],
            &path,
        )
        .await
        .unwrap_err();
    redirect_server.await.unwrap();
    assert!(!destination_server.await.unwrap());

    assert!(matches!(error, CoreError::Transport(message) if message.contains("cross-origin")));
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn concurrent_identical_gets_share_one_response_within_a_discovery() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let coalescer = HttpRequestCoalescer::default();
    let url = format!("http://{address}/index");
    let (left, right) = tokio::join!(
        transport.get_bytes_coalesced(&url, &[("Accept", "text/html")], &coalescer),
        transport.get_bytes_coalesced(&url, &[("accept", "text/html")], &coalescer),
    );
    let left = left.unwrap();
    let right = right.unwrap();
    server.await.unwrap();

    assert!(Arc::ptr_eq(&left, &right));
    assert_eq!(left.as_ref(), b"payload");
}

#[tokio::test]
async fn single_attempt_streamed_get_makes_one_get_and_preserves_status_classification() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await.to_ascii_lowercase();
        assert!(request.starts_with("get /ticket http/1.1\r\n"));
        stream
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(150), listener.accept()).await.is_ok()
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ticket.json");
    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let error = transport
        .get_to_path_same_origin_limited_with_policy(
            &format!("http://{address}/ticket"),
            &[],
            &path,
            1024,
            HttpGetPolicy::SingleAttempt,
        )
        .await
        .unwrap_err();
    let second_request_observed = server.await.unwrap();

    assert!(!second_request_observed, "single-attempt GET must not retry HTTP 503");
    assert!(matches!(error, CoreError::HttpStatus { status: 503, retryable: true }));
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn single_attempt_streamed_get_follows_same_origin_redirect_with_provider_headers() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut redirect, _) = listener.accept().await.unwrap();
        let first = read_request(&mut redirect).await.to_ascii_lowercase();
        assert!(first.starts_with("get /ticket http/1.1\r\n"));
        assert!(first.contains("x-provider-test: fixture\r\n"));
        redirect
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();

        let (mut content, _) = listener.accept().await.unwrap();
        let second = read_request(&mut content).await.to_ascii_lowercase();
        assert!(second.starts_with("get /content http/1.1\r\n"));
        assert!(second.contains("x-provider-test: fixture\r\n"));
        content
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload")
            .await
            .unwrap();
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("content.json");
    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let receipt = transport
        .get_to_path_same_origin_limited_with_policy(
            &format!("http://{address}/ticket"),
            &[("x-provider-test", "fixture")],
            &path,
            1024,
            HttpGetPolicy::SingleAttempt,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(std::fs::read(path).unwrap(), b"payload");
    assert_eq!(receipt.size_bytes, 7);
}

#[tokio::test]
async fn single_attempt_streamed_get_rejects_same_origin_redirect_outside_the_reviewed_path() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await.to_ascii_lowercase();
        assert!(request.starts_with("get /file?ft=private http/1.1\r\n"));
        stream
            .write_all(
                b"HTTP/1.1 302 Found\r\nLocation: /other?ft=private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_millis(150), listener.accept()).await.is_ok()
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ticket.json");
    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let error = transport
        .get_to_path_same_origin_path_limited_with_policy(
            &format!("http://{address}/file?ft=private"),
            &[],
            &path,
            1024,
            HttpGetPolicy::SingleAttempt,
            "/file",
        )
        .await
        .unwrap_err();
    let second_request_observed = server.await.unwrap();

    assert!(matches!(error, CoreError::Transport(_)));
    assert!(!second_request_observed);
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn single_attempt_streamed_get_keeps_default_tls_certificate_validation() {
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let certificate_pem = include_bytes!("../../../tests/fixtures/protocols/ftp/server-cert.pem");
    let certificate_pem = std::str::from_utf8(certificate_pem).unwrap();
    let certificate_base64 =
        certificate_pem.lines().filter(|line| !line.starts_with("-----")).collect::<String>();
    let certificate = base64::engine::general_purpose::STANDARD.decode(certificate_base64).unwrap();
    let key_pem = include_bytes!("../../../tests/fixtures/protocols/ftp/server-key.pem");
    let key_pem = std::str::from_utf8(key_pem).unwrap();
    let key_base64 = key_pem.lines().filter(|line| !line.starts_with("-----")).collect::<String>();
    let key = base64::engine::general_purpose::STANDARD.decode(key_base64).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key));
    let config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(certificate)], key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        if let Ok(mut stream) = acceptor.accept(stream).await {
            let _ = read_request(&mut stream).await;
            let _ = stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload",
                )
                .await;
        }
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("content.json");
    let transport = HttpTransport::new(Limits::default(), false).unwrap();
    let error = transport
        .get_to_path_same_origin_limited_with_policy(
            &format!("https://{address}/content"),
            &[],
            &path,
            1024,
            HttpGetPolicy::SingleAttempt,
        )
        .await
        .unwrap_err();
    server.await.unwrap();

    assert!(matches!(error, CoreError::Transport(_)));
    assert!(!path.exists());
}

#[tokio::test]
async fn single_attempt_streamed_get_waits_for_shared_permit_and_cancels_without_a_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_millis(150), listener.accept()).await.is_ok()
    });

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("content.json");
    let mut limits = Limits::default();
    limits.request_concurrency = 0;
    let budget = Arc::new(RequestBudget::new(&limits));
    let transport = Arc::new(HttpTransport::with_budget(limits, false, budget).unwrap());
    let worker_transport = transport.clone();
    let url = format!("http://{address}/content");
    let worker = tokio::spawn(async move {
        worker_transport
            .get_to_path_same_origin_limited_with_policy(
                &url,
                &[],
                &path,
                1024,
                HttpGetPolicy::SingleAttempt,
            )
            .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    transport.cancel();
    let error = worker.await.unwrap().unwrap_err();

    assert!(matches!(error, CoreError::Cancelled));
    assert!(!server.await.unwrap(), "cancelled budget wait must not send a request");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
}
