use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn run_radiust(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_radiust"))
        .env_clear()
        .args(args)
        .output()
        .expect("native radiust process starts")
}

fn parse_report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).expect("CLI emits a JSON report")
}

fn assert_aggregate_v1(report: &Value) {
    assert_aggregate_report_v1(report, false);
}

fn assert_aggregate_report_v1(report: &Value, interrupted: bool) {
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["command"], "discover");
    assert_eq!(report["run_id"], Value::Null);
    assert_eq!(report["error"], Value::Null);
    assert_eq!(report["interrupted"], interrupted);
    assert!(report["counts"].is_object());
    assert!(report["items"].is_array());
    assert_eq!(
        report["counts"]["total"].as_u64(),
        Some(report["items"].as_array().unwrap().len() as u64)
    );
    for status in [
        "success",
        "no_data",
        "stale",
        "missing_credentials",
        "retired",
        "network_restricted",
        "upstream_failed",
        "ambiguous",
        "timeout",
        "cancelled",
        "not_started",
    ] {
        assert!(report["counts"][status].is_number(), "missing count for {status}");
    }
    let status_total = [
        "success",
        "no_data",
        "stale",
        "missing_credentials",
        "retired",
        "network_restricted",
        "upstream_failed",
        "ambiguous",
        "timeout",
        "cancelled",
        "not_started",
    ]
    .iter()
    .map(|status| report["counts"][status].as_u64().unwrap())
    .sum::<u64>();
    assert_eq!(report["counts"]["total"].as_u64(), Some(status_total));
    for item in report["items"].as_array().unwrap() {
        assert!(item["source"].is_string());
        assert!(item["status"].is_string());
        assert!(item.get("capabilities").is_some());
        assert!(item.get("error").is_some());
    }
}

struct LocalHttpProxy {
    address: SocketAddr,
    requests: Receiver<String>,
    stopped: Arc<AtomicBool>,
    request_count: Arc<AtomicUsize>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy)]
enum ProxyResponse {
    EmptyPage,
    HoldOpen,
}

impl LocalHttpProxy {
    fn start(response: ProxyResponse) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("local proxy binds");
        listener.set_nonblocking(true).expect("local proxy is nonblocking");
        let address = listener.local_addr().expect("local proxy address");
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        let request_count = Arc::new(AtomicUsize::new(0));
        let worker_request_count = request_count.clone();
        let (request_tx, requests) = mpsc::channel();
        let worker = thread::spawn(move || {
            while !worker_stopped.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
                        let Some(request) = read_proxy_request(&mut stream, &worker_stopped) else {
                            continue;
                        };
                        worker_request_count.fetch_add(1, Ordering::SeqCst);
                        let _ = request_tx.send(String::from_utf8_lossy(&request).into_owned());
                        match response {
                            ProxyResponse::EmptyPage => {
                                let _ = stream.write_all(
                                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                );
                            }
                            ProxyResponse::HoldOpen => {
                                wait_for_proxy_disconnect(&mut stream, &worker_stopped)
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self { address, requests, stopped, request_count, worker: Some(worker) }
    }

    fn proxy_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn receive_request(&self, timeout: Duration) -> Result<String, mpsc::RecvTimeoutError> {
        self.requests.recv_timeout(timeout)
    }

    fn drain_requests(&self) -> Vec<String> {
        self.requests.try_iter().collect()
    }

    fn count(&self) -> usize {
        self.request_count.load(Ordering::SeqCst)
    }

    fn stop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("local proxy thread exits");
        }
    }
}

impl Drop for LocalHttpProxy {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_proxy_request(stream: &mut TcpStream, stopped: &AtomicBool) -> Option<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 2048];
    while !stopped.load(Ordering::SeqCst) {
        match stream.read(&mut buffer) {
            Ok(0) => return None,
            Ok(read) => {
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    return Some(request);
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return None,
        }
    }
    None
}

fn wait_for_proxy_disconnect(stream: &mut TcpStream, stopped: &AtomicBool) {
    let mut buffer = [0_u8; 1];
    while !stopped.load(Ordering::SeqCst) {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
}

struct TemporaryConfig(std::path::PathBuf);

impl TemporaryConfig {
    fn allow_network() -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir()
            .join(format!("radiust-cli-discover-multi-{}-{nonce}.yml", std::process::id()));
        std::fs::write(
            &path,
            "runtime:\n  allow_network: true\n  request_timeout: 2\n  discovery_deadline: 10\n",
        )
        .expect("test configuration is written");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TemporaryConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn command_with_proxy_env(args: &[&str], proxy: &LocalHttpProxy) -> Command {
    let proxy_url = proxy.proxy_url();
    let mut command = Command::new(env!("CARGO_BIN_EXE_radiust"));
    command
        .env_clear()
        .args(args)
        .arg("--json")
        .env("HTTP_PROXY", &proxy_url)
        .env("http_proxy", &proxy_url)
        .env("ALL_PROXY", &proxy_url)
        .env("all_proxy", &proxy_url)
        .env("NO_PROXY", "")
        .env("no_proxy", "");
    command
}

fn command_with_local_proxy(
    args: &[&str],
    config: &TemporaryConfig,
    proxy: &LocalHttpProxy,
) -> Command {
    let mut command = command_with_proxy_env(args, proxy);
    command.arg("--conf").arg(config.path());
    command
}

#[test]
fn discover_all_emits_v1_aggregate_and_keeps_network_disabled_by_default() {
    let proxy = LocalHttpProxy::start(ProxyResponse::EmptyPage);
    let output = command_with_proxy_env(&["discover", "all"], &proxy)
        .output()
        .expect("native radiust process starts");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stderr.is_empty());
    let report = parse_report(&output);
    assert_aggregate_v1(&report);
    assert_eq!(report["query"], json!({"source": "all", "latest": true, "max_age": null}));

    let counts = &report["counts"];
    assert_eq!(counts["total"], 26);
    assert_eq!(counts["success"], 0);
    assert!(counts["network_restricted"].as_u64().unwrap() > 0);
    assert!(counts["retired"].as_u64().unwrap() > 0);
    assert!(counts["missing_credentials"].as_u64().unwrap() > 0);
    assert_eq!(counts["no_data"], 0);

    for item in report["items"].as_array().unwrap() {
        assert!(matches!(
            item["status"].as_str(),
            Some("network_restricted" | "retired" | "missing_credentials")
        ));
    }
    let source_ids = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["source"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(source_ids.len(), 24);
    assert_eq!(proxy.count(), 0, "network-disabled discovery must not request public sources");
}

#[test]
fn explicit_multi_source_discover_reports_query_sources_and_offline_statuses() {
    let output = run_radiust(&["discover", "au", "vn", "--json"]);

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stderr.is_empty());
    let report = parse_report(&output);
    assert_aggregate_v1(&report);
    assert_eq!(report["query"]["sources"], json!(["au", "vn"]));
    assert!(report["query"].get("source").is_none());
    assert_eq!(report["query"]["latest"], true);
    assert_eq!(report["query"]["max_age"], Value::Null);
    assert_eq!(report["counts"]["total"], 2);
    assert_eq!(report["counts"]["network_restricted"], 2);
    assert_eq!(report["counts"]["success"], 0);
    assert_eq!(
        report["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["source"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["au", "vn"]
    );
    for item in report["items"].as_array().unwrap() {
        assert_eq!(item["status"], "network_restricted");
        assert_eq!(item["error"]["code"], "network_restricted");
    }
}

#[test]
fn retired_and_credential_limited_sources_are_classified_offline() {
    let output = run_radiust(&["discover", "uk", "id", "ph", "--json"]);

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stderr.is_empty());
    let report = parse_report(&output);
    assert_aggregate_v1(&report);
    assert_eq!(report["query"]["sources"], json!(["uk", "id", "ph"]));
    assert_eq!(report["counts"]["total"], 3);
    assert_eq!(report["counts"]["retired"], 1);
    assert_eq!(report["counts"]["missing_credentials"], 1);
    assert_eq!(report["counts"]["network_restricted"], 1);

    let statuses = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| (item["source"].as_str().unwrap(), item["status"].as_str().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![("id", "missing_credentials"), ("ph", "network_restricted"), ("uk", "retired")]
    );
}

#[test]
fn invalid_multi_source_combinations_exit_two_before_configuration_or_discovery() {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let absent_config = format!(
        "{}/radiust-cli-discover-multi-missing-{}-{nonce}.yml",
        std::env::temp_dir().display(),
        std::process::id()
    );
    let cases: &[(&[&str], &str)] = &[
        (
            &["discover", "all", "--product", "composite"],
            "invalid query: all cannot be combined with source filters",
        ),
        (
            &["discover", "au", "vn", "--station", "peninsular"],
            "invalid query: multi-source queries do not accept product, station, or base-time filters",
        ),
        (
            &["discover", "au", "vn", "--at", "2026-09-24T00:00:00Z"],
            "invalid query: multi-source queries accept latest only",
        ),
        (&["discover", "au", "au"], "invalid query: source list contains duplicates"),
        (&["discover", "all", "au"], "invalid query: all cannot be combined with explicit sources"),
    ];

    for (args, expected_message) in cases {
        let mut command_args = args.to_vec();
        command_args.extend(["--conf", absent_config.as_str(), "--json"]);
        let output = run_radiust(&command_args);

        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert!(output.stderr.is_empty(), "args: {args:?}");
        let report = parse_report(&output);
        assert_eq!(report["schema_version"], 1, "args: {args:?}");
        assert_eq!(report["error"]["stage"], "validate", "args: {args:?}");
        assert_eq!(report["error"]["code"], "error", "args: {args:?}");
        assert_eq!(report["error"]["message"], *expected_message, "args: {args:?}");
        assert_eq!(report["command"], Value::Null, "args: {args:?}");
        assert_eq!(report["items"], json!([]), "args: {args:?}");
        assert_eq!(report["interrupted"], false, "args: {args:?}");
    }

    let config = TemporaryConfig::allow_network();
    let mut proxy = LocalHttpProxy::start(ProxyResponse::EmptyPage);
    let output = command_with_local_proxy(
        &["discover", "au", "vn", "--station", "peninsular"],
        &config,
        &proxy,
    )
    .output()
    .expect("native radiust process starts");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let report = parse_report(&output);
    assert_eq!(report["error"]["stage"], "validate");
    assert_eq!(report["error"]["message"], cases[1].1);
    assert_eq!(proxy.count(), 0, "invalid queries must not dispatch source requests");
    proxy.stop();
}

#[test]
fn multi_source_report_classifies_local_empty_pages_with_retired_and_missing_credentials() {
    let config = TemporaryConfig::allow_network();
    let mut proxy = LocalHttpProxy::start(ProxyResponse::EmptyPage);
    let output = command_with_local_proxy(&["discover", "vn", "uk", "id"], &config, &proxy)
        .output()
        .expect("native radiust process starts");

    assert_eq!(output.status.code(), Some(5));
    assert!(output.stderr.is_empty());
    let report = parse_report(&output);
    assert_aggregate_v1(&report);
    assert_eq!(report["query"]["sources"], json!(["vn", "uk", "id"]));
    assert_eq!(report["counts"]["total"], 3);
    assert_eq!(report["counts"]["no_data"], 1);
    assert_eq!(report["counts"]["retired"], 1);
    assert_eq!(report["counts"]["missing_credentials"], 1);
    assert_eq!(report["counts"]["network_restricted"], 0);

    let statuses = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["source"].as_str().unwrap(),
                item["status"].as_str().unwrap(),
                item["error"]["code"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        vec![
            ("id", "missing_credentials", "missing_credentials"),
            ("uk", "retired", "retired"),
            ("vn", "no_data", "no_data"),
        ]
    );

    let requests = proxy.drain_requests();
    assert_eq!(proxy.count(), 11, "VN metadata is served entirely by the local proxy");
    assert_eq!(requests.len(), 11);
    assert!(
        requests.iter().all(|request| { request.starts_with("GET http://hymetnet.gov.vn/radar/") })
    );
    proxy.stop();
}

#[cfg(unix)]
#[test]
fn ctrl_c_returns_a_complete_interrupted_aggregate_report_with_exit_130() {
    let config = TemporaryConfig::allow_network();
    let mut proxy = LocalHttpProxy::start(ProxyResponse::HoldOpen);
    let child: Child = command_with_local_proxy(&["discover", "vn", "uk", "id"], &config, &proxy)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("native radiust process starts");
    let request = proxy
        .receive_request(Duration::from_secs(5))
        .expect("CLI reaches the local fixture before interruption");
    assert!(request.starts_with("GET http://hymetnet.gov.vn/radar/"));
    // Let the CLI's startup poll register Tokio's SIGINT handler before sending.
    thread::sleep(Duration::from_millis(250));

    let signal = Command::new("/bin/kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("SIGINT sender starts");
    assert!(signal.success(), "SIGINT must be delivered to the CLI child");
    let output = child.wait_with_output().expect("interrupted CLI process exits");

    assert_eq!(
        output.status.code(),
        Some(130),
        "CLI must handle SIGINT and emit an envelope; terminating signal={:?}, stdout bytes={}",
        output.status.signal(),
        output.stdout.len()
    );
    assert!(output.stderr.is_empty());
    let report = parse_report(&output);
    assert_aggregate_report_v1(&report, true);
    assert_eq!(report["query"]["sources"], json!(["vn", "uk", "id"]));
    assert_eq!(report["counts"]["total"], 3);
    assert!(report["counts"]["cancelled"].as_u64().unwrap() > 0);
    assert_eq!(report["counts"]["retired"], 1);
    assert_eq!(report["counts"]["missing_credentials"], 1);
    proxy.stop();
}
