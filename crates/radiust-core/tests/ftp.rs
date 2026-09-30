use radiust_core::errors::CoreError;
use radiust_core::limits::{Limits, RequestBudget};
use radiust_core::transport::FtpTransport;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

struct FixtureServer {
    child: Child,
    port: u16,
    log_path: PathBuf,
}

impl FixtureServer {
    fn start(tls: bool, stall_retr: bool) -> Self {
        Self::start_mode(tls, stall_retr, false, 0)
    }

    fn start_broken_retr() -> Self {
        Self::start_mode(false, false, true, 0)
    }

    fn start_transient_list_failure() -> Self {
        Self::start_mode(false, false, false, 1)
    }

    fn start_mode(
        tls: bool,
        stall_retr: bool,
        break_retr: bool,
        transient_list_failures: usize,
    ) -> Self {
        let root = workspace_root();
        let fixture = root.join("tests/fixtures/protocols/ftp");
        let log_path =
            std::env::temp_dir().join(format!("radiust-ftp-commands-{}.log", uuid::Uuid::new_v4()));
        let mut command = Command::new("python3");
        command
            .arg(root.join("tests/support/ftp_server.py"))
            .arg("--root")
            .arg(&fixture)
            .arg("--log-file")
            .arg(&log_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if tls {
            command
                .arg("--tls")
                .arg("--certfile")
                .arg(fixture.join("server-cert.pem"))
                .arg("--keyfile")
                .arg(fixture.join("server-key.pem"));
        }
        if stall_retr {
            command.arg("--stall-retr");
        }
        if break_retr {
            command.arg("--break-retr");
        }
        if transient_list_failures > 0 {
            command.arg("--transient-list-failures").arg(transient_list_failures.to_string());
        }
        let mut child = command.spawn().expect("start FTP fixture server");
        let stdout = child.stdout.take().expect("fixture server stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read fixture server port");
        let value: serde_json::Value =
            serde_json::from_str(&line).expect("fixture server ready JSON");
        let port = value["port"].as_u64().expect("fixture server port") as u16;
        child.stdout = Some(reader.into_inner());
        Self { child, port, log_path }
    }

    fn url(&self, scheme: &str, path: &str) -> String {
        format!("{scheme}://127.0.0.1:{}/{}", self.port, path.trim_start_matches('/'))
    }

    fn commands(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.log_path);
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("workspace root")
}

fn fixture_bytes() -> &'static [u8] {
    include_bytes!("../../../tests/fixtures/protocols/ftp/fixture.bin")
}

#[tokio::test]
async fn ftp_transport_uses_shared_request_and_host_budgets() {
    let mut limits = Limits::default();
    limits.request_concurrency = 1;
    limits.host_concurrency = 1;
    let budget = Arc::new(RequestBudget::new(&limits));
    let transport = Arc::new(FtpTransport::with_budget(limits.clone(), false, budget.clone()));
    let address = "ftp://127.0.0.1:1/";

    let request_permit = budget.acquire_request().await.unwrap();
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let worker = transport.clone();
    let request_task = tokio::spawn(async move {
        let _ = started.send(());
        worker.list(address, "anonymous", "anonymous").await
    });
    started_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!request_task.is_finished(), "FTP must wait on the shared request permit");
    drop(request_permit);
    transport.cancel();
    assert!(matches!(request_task.await.unwrap(), Err(CoreError::Cancelled)));

    let budget = Arc::new(RequestBudget::new(&limits));
    let transport = Arc::new(FtpTransport::with_budget(limits, false, budget.clone()));
    let host_permit = budget.acquire_host("127.0.0.1").await.unwrap();
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let worker = transport.clone();
    let host_task = tokio::spawn(async move {
        let _ = started.send(());
        worker.list(address, "anonymous", "anonymous").await
    });
    started_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(!host_task.is_finished(), "FTP must wait on the shared host permit");
    drop(host_permit);
    transport.cancel();
    assert!(matches!(host_task.await.unwrap(), Err(CoreError::Cancelled)));
}

#[tokio::test]
async fn passive_plain_ftp_lists_stats_and_reads_with_receipt() {
    let server = FixtureServer::start(false, false);
    let transport = FtpTransport::new(Limits::default(), false);

    let listing =
        transport.list(&server.url("ftp", ""), "radiust", "secret").await.expect("LIST succeeds");
    assert!(listing.iter().any(|line| line.contains("fixture.bin")));
    let names =
        transport.names(&server.url("ftp", ""), "radiust", "secret").await.expect("NLST succeeds");
    assert!(names.iter().any(|name| name == "fixture.bin"));

    let size = transport
        .size(&server.url("ftp", "fixture.bin"), "radiust", "secret")
        .await
        .expect("SIZE succeeds");
    assert_eq!(size, fixture_bytes().len() as u64);

    let object = transport
        .get_bytes(&server.url("ftp", "fixture.bin"), "radiust", "secret")
        .await
        .expect("RETR succeeds");
    assert_eq!(object.bytes, fixture_bytes());
    assert_eq!(object.size_bytes, fixture_bytes().len() as u64);
    assert_eq!(object.sha256, "712202b73d2d16c5b27e85b649bb97f0242c4eb797cc871953153da1537b0455");
}

#[tokio::test]
async fn ftp_streaming_download_writes_receipt_and_atomically_replaces_destination() {
    let server = FixtureServer::start(false, false);
    let transport = FtpTransport::new(Limits::default(), false);
    let directory = tempfile::tempdir().expect("create destination directory");
    let destination = directory.path().join("download.bin");
    std::fs::write(&destination, b"previous contents").expect("seed destination");

    let receipt = transport
        .get_to_path_limited(
            &server.url("ftp", "fixture.bin"),
            "radiust",
            "secret",
            &destination,
            fixture_bytes().len() as u64,
        )
        .await
        .expect("streamed RETR succeeds");

    assert_eq!(receipt.size_bytes, fixture_bytes().len() as u64);
    assert_eq!(receipt.sha256, "712202b73d2d16c5b27e85b649bb97f0242c4eb797cc871953153da1537b0455");
    assert_eq!(std::fs::read(&destination).expect("read published file"), fixture_bytes());
    assert_eq!(
        std::fs::read_dir(directory.path()).expect("list destination directory").count(),
        1,
        "successful publish leaves no staging file"
    );
}

#[tokio::test]
async fn ftp_streaming_download_enforces_remaining_limit_without_residue() {
    let server = FixtureServer::start(false, false);
    let transport = FtpTransport::new(Limits::default(), false);
    let directory = tempfile::tempdir().expect("create destination directory");
    let destination = directory.path().join("download.bin");
    std::fs::write(&destination, b"old contents").expect("seed destination");

    let error = transport
        .get_to_path_limited(
            &server.url("ftp", "fixture.bin"),
            "radiust",
            "secret",
            &destination,
            fixture_bytes().len() as u64 - 1,
        )
        .await
        .expect_err("remaining byte limit must be enforced");

    assert!(matches!(error, CoreError::ResourceLimit(_)));
    assert_eq!(std::fs::read(&destination).unwrap(), b"old contents");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    assert!(server.commands().lines().any(|command| command == "ABOR"));
}

#[tokio::test]
async fn ftp_streaming_download_enforces_artifact_limit_without_residue() {
    let server = FixtureServer::start(false, false);
    let mut limits = Limits::default();
    limits.max_artifact_bytes = fixture_bytes().len() as u64 - 1;
    let transport = FtpTransport::new(limits, false);
    let directory = tempfile::tempdir().expect("create destination directory");
    let destination = directory.path().join("download.bin");
    std::fs::write(&destination, b"old contents").expect("seed destination");

    let error = transport
        .get_to_path_limited(
            &server.url("ftp", "fixture.bin"),
            "radiust",
            "secret",
            &destination,
            u64::MAX,
        )
        .await
        .expect_err("artifact byte limit must be enforced");

    assert!(matches!(error, CoreError::ResourceLimit(_)));
    assert_eq!(std::fs::read(&destination).unwrap(), b"old contents");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    assert!(server.commands().lines().any(|command| command == "ABOR"));
}

#[tokio::test]
async fn ftp_streaming_download_timeout_removes_partial_files() {
    let server = FixtureServer::start(false, true);
    let mut limits = Limits::default();
    limits.request_timeout_secs = 1;
    let transport = FtpTransport::new(limits, false);
    let directory = tempfile::tempdir().expect("create destination directory");
    let destination = directory.path().join("download.bin");
    std::fs::write(&destination, b"old contents").expect("seed destination");
    let started = std::time::Instant::now();

    let error = transport
        .get_to_path_limited(
            &server.url("ftp", "fixture.bin"),
            "radiust",
            "secret",
            &destination,
            u64::MAX,
        )
        .await
        .expect_err("stalled RETR must time out");

    assert!(matches!(error, CoreError::Transport(_)));
    assert!(started.elapsed() < Duration::from_secs(6));
    assert!(server.commands().lines().any(|command| command == "ABOR"));
    assert_eq!(std::fs::read(&destination).unwrap(), b"old contents");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn ftp_streaming_download_cancellation_removes_partial_files() {
    let server = FixtureServer::start(false, true);
    let mut limits = Limits::default();
    limits.request_timeout_secs = 30;
    let transport = FtpTransport::new(limits, false);
    let worker = transport.clone();
    let directory = tempfile::tempdir().expect("create destination directory");
    let destination = directory.path().join("download.bin");
    std::fs::write(&destination, b"old contents").expect("seed destination");
    let task_destination = destination.clone();
    let url = server.url("ftp", "fixture.bin");
    let task = tokio::spawn(async move {
        worker.get_to_path_limited(&url, "radiust", "secret", &task_destination, u64::MAX).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    transport.cancel();

    let error = task.await.expect("join FTP task").expect_err("cancel must stop RETR");
    assert!(matches!(error, CoreError::Cancelled));
    assert!(server.commands().lines().any(|command| command == "ABOR"));
    assert_eq!(std::fs::read(&destination).unwrap(), b"old contents");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn ftp_authentication_failure_is_an_error() {
    let server = FixtureServer::start(false, false);
    let transport = FtpTransport::new(Limits::default(), false);
    let error = transport
        .list(&server.url("ftp", ""), "radiust", "wrong")
        .await
        .expect_err("wrong password must fail");
    assert!(matches!(error, CoreError::Transport(_)));
    assert_eq!(
        server.commands().lines().filter(|command| *command == "USER").count(),
        1,
        "authentication failures are permanent and must not consume retry attempts"
    );
}

#[tokio::test]
async fn ftp_transient_listing_failure_retries_within_budget() {
    let server = FixtureServer::start_transient_list_failure();
    let transport = FtpTransport::new(Limits::default(), false);
    let listing = transport
        .list(&server.url("ftp", ""), "radiust", "secret")
        .await
        .expect("temporary 450 should be retried");
    assert!(listing.iter().any(|line| line.contains("fixture.bin")));
    assert_eq!(server.commands().lines().filter(|command| *command == "USER").count(), 2);
}

#[tokio::test]
async fn explicit_ftps_uses_rustls_and_test_trust_root() {
    let server = FixtureServer::start(true, false);
    let certificate =
        include_bytes!("../../../tests/fixtures/protocols/ftp/server-cert.der").to_vec();
    let transport = FtpTransport::with_root_certificate(Limits::default(), false, certificate);
    let object = transport
        .get_bytes(&server.url("ftps", "fixture.bin"), "radiust", "secret")
        .await
        .expect("explicit FTPS RETR succeeds");
    assert_eq!(object.bytes, fixture_bytes());
}

#[tokio::test]
async fn explicit_ftps_rejects_untrusted_certificate() {
    let server = FixtureServer::start(true, false);
    let transport = FtpTransport::new(Limits::default(), false);
    let error = transport
        .list(&server.url("ftps", ""), "radiust", "secret")
        .await
        .expect_err("self-signed certificate must be rejected");
    assert!(matches!(error, CoreError::Transport(_)));
}

#[tokio::test]
async fn ftp_data_timeout_is_bounded() {
    let server = FixtureServer::start(false, true);
    let mut limits = Limits::default();
    limits.request_timeout_secs = 1;
    let transport = FtpTransport::new(limits, false);
    let started = std::time::Instant::now();
    let error = transport
        .get_bytes(&server.url("ftp", "fixture.bin"), "radiust", "secret")
        .await
        .expect_err("stalled RETR must time out");
    assert!(matches!(error, CoreError::Transport(_)));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "three 1s attempts plus bounded backoff must stay below the frame-scale bound"
    );
    assert!(server.commands().lines().any(|command| command == "ABOR"));
}

#[tokio::test]
async fn ftp_broken_data_channel_is_reported() {
    let server = FixtureServer::start_broken_retr();
    let transport = FtpTransport::new(Limits::default(), false);
    let error = transport
        .get_bytes(&server.url("ftp", "fixture.bin"), "radiust", "secret")
        .await
        .expect_err("broken RETR must fail");
    assert!(matches!(error, CoreError::Transport(_)));
}

#[tokio::test]
async fn ftp_cancel_interrupts_stalled_read() {
    let server = FixtureServer::start(false, true);
    let mut limits = Limits::default();
    limits.request_timeout_secs = 30;
    let transport = FtpTransport::new(limits, false);
    let worker = transport.clone();
    let url = server.url("ftp", "fixture.bin");
    let task = tokio::spawn(async move { worker.get_bytes(&url, "radiust", "secret").await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    transport.cancel();
    let error = task.await.expect("join FTP task").expect_err("cancel must stop RETR");
    assert!(matches!(error, CoreError::Cancelled));
    assert!(server.commands().lines().any(|command| command == "ABOR"));
}
