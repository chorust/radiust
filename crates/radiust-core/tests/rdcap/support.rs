use chrono::{DateTime, Duration as ChronoDuration, Utc};
use parking_lot::Mutex;
use radiust_core::identity::logical_id;
use radiust_core::model::{ArtifactReceipt, FrameRef, RawArtifact, RawFrame};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub const FIXTURE_MANIFEST: &str =
    include_str!("../../../../tests/fixtures/sources/rdcap/manifest.json");

#[derive(Serialize)]
struct RdcapBinding<'a> {
    schema_version: u8,
    source: &'a str,
    product: &'a str,
    station: &'a str,
    country: &'a str,
    station_code: &'a str,
    key: &'a str,
    valid_time: &'a str,
    logical_id: &'a str,
    content_sha256: &'a str,
    content_size_bytes: u64,
}

/// Builds a temporary raw frame from the checked-in reconstructed response.
/// These bytes are decoder fixtures and are never passed off as live HTTP data.
pub fn fixture_raw_frame(
    station_id: &str,
    key: &str,
    response_bytes: &[u8],
    temp_root: &std::path::Path,
) -> RawFrame {
    let (country, station_code) = fixture_station_parts(station_id);
    let valid_time = chrono::DateTime::from_timestamp_millis(key.parse().unwrap())
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    let content_sha256 = hex::encode(Sha256::digest(response_bytes));
    let mut frame = FrameRef {
        source: "rdcap".into(),
        product: "reflectivity".into(),
        station: Some(station_id.into()),
        valid_time: valid_time.clone(),
        base_time: None,
        logical_id: String::new(),
        revision: Some(content_sha256.clone()),
        locator_version: "rdcap-csr-v1".into(),
        locator: json!({
            "country": country,
            "station_code": station_code,
            "key": key,
        }),
    };
    frame.logical_id = logical_id(&frame).unwrap();
    std::fs::create_dir_all(temp_root).unwrap();
    let mut data_file =
        tempfile::Builder::new().prefix("rdcap-science-data-").tempfile_in(temp_root).unwrap();
    data_file.write_all(response_bytes).unwrap();
    data_file.as_file().sync_all().unwrap();
    let data_path = data_file.into_temp_path();
    let data_receipt = ArtifactReceipt {
        name: "file-response.json".into(),
        media_type: "application/json".into(),
        size_bytes: response_bytes.len() as u64,
        sha256: content_sha256.clone(),
    };
    let binding = RdcapBinding {
        schema_version: 1,
        source: "rdcap",
        product: "reflectivity",
        station: station_id,
        country,
        station_code,
        key,
        valid_time: &valid_time,
        logical_id: &frame.logical_id,
        content_sha256: &content_sha256,
        content_size_bytes: response_bytes.len() as u64,
    };
    let binding_bytes = serde_json::to_vec_pretty(&binding).unwrap();
    let mut binding_file =
        tempfile::Builder::new().prefix("rdcap-science-binding-").tempfile_in(temp_root).unwrap();
    binding_file.write_all(&binding_bytes).unwrap();
    binding_file.as_file().sync_all().unwrap();
    let binding_path = binding_file.into_temp_path();
    RawFrame {
        frame,
        artifacts: vec![
            RawArtifact { receipt: data_receipt, path: data_path },
            RawArtifact {
                receipt: ArtifactReceipt {
                    name: "binding.json".into(),
                    media_type: "application/json".into(),
                    size_bytes: binding_bytes.len() as u64,
                    sha256: hex::encode(Sha256::digest(&binding_bytes)),
                },
                path: binding_path,
            },
        ],
        private_locator: None,
    }
}

pub fn reconstructed_file_response(station_id: &str) -> Vec<u8> {
    let (country, station_code) = fixture_station_parts(station_id);
    let relative = format!(
        "tests/fixtures/sources/rdcap/{country}/{station_code}/file-response.reconstructed.json"
    );
    std::fs::read(repository_root().join(relative)).unwrap()
}

fn fixture_station_parts(station_id: &str) -> (&str, &str) {
    let (prefix, code) = station_id.split_at(2);
    let country = match prefix {
        "TW" => "TWN",
        "JP" => "JPN",
        "PH" => "PHL",
        _ => panic!("invalid fixture station id"),
    };
    (country, code)
}

pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

#[derive(Clone)]
pub struct TestClock(Arc<Mutex<DateTime<Utc>>>);

impl TestClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self(Arc::new(Mutex::new(now)))
    }

    pub fn now(&self) -> DateTime<Utc> {
        *self.0.lock()
    }

    pub fn advance(&self, delta: ChronoDuration) -> DateTime<Utc> {
        let mut now = self.0.lock();
        *now += delta;
        *now
    }
}

#[derive(Clone, Debug)]
pub struct LoopbackResponse {
    pub status: u16,
    pub headers: Vec<String>,
    pub body: Vec<u8>,
    pub delay: Duration,
}

impl LoopbackResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self { status, headers: Vec::new(), body: body.into(), delay: Duration::ZERO }
    }

    pub fn header(mut self, value: impl Into<String>) -> Self {
        self.headers.push(value.into());
        self
    }

    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// Starts a loopback-only HTTP server. The task returns each complete request
/// in arrival order, which also makes request-count assertions deterministic.
pub async fn spawn_loopback_server(
    responses: Vec<LoopbackResponse>,
) -> (String, JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::with_capacity(responses.len());
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut stream).await);
            if !response.delay.is_zero() {
                tokio::time::sleep(response.delay).await;
            }
            write_response(&mut stream, response).await;
        }
        requests
    });
    (format!("http://{address}"), task)
}

pub fn loopback_address(url: &str) -> SocketAddr {
    url.strip_prefix("http://").unwrap().parse().unwrap()
}

pub async fn cancel_after(token: CancellationToken, delay: Duration) {
    tokio::time::sleep(delay).await;
    token.cancel();
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 2048];
    loop {
        let count = stream.read(&mut buffer).await.unwrap();
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        if bytes.len() >= header_end + 4 + content_length {
            break;
        }
    }
    String::from_utf8(bytes).unwrap()
}

async fn write_response(stream: &mut TcpStream, response: LoopbackResponse) {
    let reason = match response.status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Fixture Response",
    };
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason,
        response.body.len()
    );
    for header in response.headers {
        headers.push_str(&header);
        headers.push_str("\r\n");
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&response.body).await.unwrap();
    let _ = stream.shutdown().await;
}
