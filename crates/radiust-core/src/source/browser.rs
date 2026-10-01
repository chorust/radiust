//! Isolated Chromium/CDP support for providers that require browser execution.
//!
//! Every page request is paused by CDP and checked against an exact host
//! allow-list before it is continued. WebSocket, worker, WebRTC, file, and FTP
//! access is disabled so page code cannot bypass that request path.

use crate::errors::{CoreError, CoreResult};
use crate::limits::{Limits, RequestBudget};
use crate::model::{ArtifactReceipt, RawArtifact};
use crate::source::SourceContext;
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, client_async};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

const HARDENING_SCRIPT: &str = r#"(()=>{
  const blocked=['WebSocket','Worker','SharedWorker','EventSource','WebTransport',
    'RTCPeerConnection','webkitRTCPeerConnection'];
  const deny=new Proxy(function(){},{apply(){throw new DOMException('disabled','SecurityError')},
    construct(){throw new DOMException('disabled','SecurityError')}});
  for(const name of blocked) Object.defineProperty(globalThis,name,
    {value:deny,configurable:false,writable:false});
  const descriptor=Object.getOwnPropertyDescriptor(Navigator.prototype,'serviceWorker');
  if(descriptor&&descriptor.configurable) Object.defineProperty(Navigator.prototype,
    'serviceWorker',{value:undefined,configurable:false,writable:false});
  return blocked.every(name=>globalThis[name]===deny) &&
    typeof navigator.serviceWorker==='undefined';
})()"#;

/// Browser executable and the exact host names page requests may contact.
#[derive(Clone, Debug)]
pub struct ChromiumConfig {
    executable: Option<PathBuf>,
    allowed_hosts: Vec<String>,
    user_agent: Option<String>,
    blocked_urls: Vec<String>,
}

impl ChromiumConfig {
    pub fn new<I, H>(allowed_hosts: I) -> Self
    where
        I: IntoIterator<Item = H>,
        H: Into<String>,
    {
        Self {
            executable: None,
            user_agent: None,
            blocked_urls: Vec::new(),
            allowed_hosts: allowed_hosts.into_iter().map(Into::into).collect(),
        }
    }

    pub fn with_blocked_urls(mut self, urls: &[&str]) -> Self {
        self.blocked_urls = urls.iter().map(|url| (*url).to_owned()).collect();
        self
    }

    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    pub fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.executable = Some(executable.into());
        self
    }
}

/// Optional CSS-based token extraction for a warmup page.
#[derive(Clone, Debug)]
pub struct CsrfSelector {
    pub css: String,
    /// DOM attribute to read, such as `content` or `value`.
    pub attribute: String,
    /// Request header name used for the extracted value.
    pub header_name: String,
}

impl CsrfSelector {
    pub fn new(
        css: impl Into<String>,
        attribute: impl Into<String>,
        header_name: impl Into<String>,
    ) -> Self {
        Self { css: css.into(), attribute: attribute.into(), header_name: header_name.into() }
    }
}

/// The main document's HTTP response, preserving the original response bytes.
#[derive(Clone, Eq, PartialEq)]
pub struct BrowserResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl fmt::Debug for BrowserResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrowserResponse")
            .field("status", &self.status)
            .field("content_type", &self.content_type)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

/// Persist a captured body under the same artifact, frame, and temporary-byte
/// limits used by HTTP acquisition.
pub(crate) async fn persist_response_body(
    body: Vec<u8>,
    temp_root: &Path,
    name: String,
    media_type: String,
    limits: &Limits,
    existing_frame_bytes: u64,
) -> CoreResult<RawArtifact> {
    let size_bytes = body.len() as u64;
    let frame_bytes = existing_frame_bytes.saturating_add(size_bytes);
    limits.validate_bytes(size_bytes, frame_bytes)?;
    if frame_bytes > limits.max_temp_bytes {
        return Err(CoreError::ResourceLimit(
            "browser artifacts exceed the temporary byte limit".into(),
        ));
    }
    tokio::fs::create_dir_all(temp_root)
        .await
        .map_err(|_| CoreError::Temporary("browser artifact directory is unavailable".into()))?;
    let destination = temp_root.join(format!("{}.browser", uuid::Uuid::new_v4()));
    let write_result = async {
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .await
            .map_err(|_| CoreError::Temporary("browser artifact could not be created".into()))?;
        file.write_all(&body)
            .await
            .map_err(|_| CoreError::Temporary("browser artifact could not be written".into()))?;
        file.flush()
            .await
            .map_err(|_| CoreError::Temporary("browser artifact could not be written".into()))
    }
    .await;
    if let Err(error) = write_result {
        let _ = tokio::fs::remove_file(&destination).await;
        return Err(error);
    }
    let path = tempfile::TempPath::try_from_path(destination.clone()).map_err(|_| {
        let _ = std::fs::remove_file(destination);
        CoreError::Temporary("browser artifact could not be retained".into())
    })?;
    let sha256 = hex::encode(Sha256::digest(&body));
    Ok(RawArtifact { receipt: ArtifactReceipt { name, media_type, size_bytes, sha256 }, path })
}

/// A non-secret receipt that navigation reached its load event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavigationReceipt {
    pub loader_id: Option<String>,
}

#[derive(Clone, Debug)]
struct HostPolicy {
    hosts: BTreeSet<String>,
}

impl HostPolicy {
    fn new(hosts: &[String]) -> CoreResult<Self> {
        let mut normalized = BTreeSet::new();
        for host in hosts {
            let Some(host) = canonical_host(host) else {
                return Err(CoreError::Transport("browser host allow-list is invalid".into()));
            };
            normalized.insert(host);
        }
        if normalized.is_empty() {
            return Err(CoreError::Transport("browser host allow-list is empty".into()));
        }
        Ok(Self { hosts: normalized })
    }

    fn allows_url(&self, address: &str) -> bool {
        let Ok(url) = Url::parse(address) else {
            return false;
        };
        if !matches!(url.scheme(), "http" | "https")
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return false;
        }
        url.host_str().and_then(canonical_host).is_some_and(|host| self.hosts.contains(&host))
    }
}

fn canonical_host(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.');
    if host.is_empty() || host.contains('*') || host.chars().any(char::is_whitespace) {
        return None;
    }
    match Host::parse(host).ok()? {
        Host::Domain(domain) => Some(domain.to_ascii_lowercase()),
        Host::Ipv4(address) => Some(address.to_string()),
        Host::Ipv6(address) => Some(address.to_string()),
    }
}

/// Find Chromium from explicit configuration, `RADIUST_CHROMIUM_EXECUTABLE`,
/// common install locations, or `PATH`.
pub fn find_chromium_executable() -> CoreResult<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("RADIUST_CHROMIUM_EXECUTABLE") {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/chromium"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
    ]);
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            for name in ["chromium", "chromium-browser", "google-chrome", "chrome"] {
                candidates.push(directory.join(name));
            }
        }
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
        .ok_or_else(|| CoreError::Transport("Chromium executable was not found".into()))
}

struct ProcessControl {
    pid: u32,
    profile_path: PathBuf,
    terminated: AtomicBool,
}

impl ProcessControl {
    fn terminate(&self) {
        if self.terminated.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(unix)]
        // Chromium is started in a new process group; terminate descendants too.
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
        let _ = std::fs::remove_dir_all(&self.profile_path);
    }
}

struct ProcessGuard {
    child: Option<Child>,
    profile: Option<TempDir>,
    control: Arc<ProcessControl>,
}

impl ProcessGuard {
    fn new(child: Child, profile: TempDir) -> Self {
        let control = Arc::new(ProcessControl {
            pid: child.id(),
            profile_path: profile.path().to_path_buf(),
            terminated: AtomicBool::new(false),
        });
        Self { child: Some(child), profile: Some(profile), control }
    }

    fn control(&self) -> Arc<ProcessControl> {
        self.control.clone()
    }

    async fn close(&mut self) -> CoreResult<()> {
        self.control.terminate();
        if let Some(child) = self.child.as_mut() {
            let wait = async {
                loop {
                    if child.try_wait().map_err(|_| ())?.is_some() {
                        return Ok::<(), ()>(());
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            };
            if tokio::time::timeout(Duration::from_secs(2), wait).await.is_err() {
                let _ = child.kill();
                let _ = child.try_wait();
            }
        }
        self.child.take();
        if let Some(profile) = self.profile.take() {
            match profile.close() {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err(CoreError::Temporary("browser profile cleanup failed".into()));
                }
            }
        }
        Ok(())
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.control.terminate();
        if let Some(child) = self.child.as_mut() {
            for _ in 0..50 {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ => std::thread::sleep(Duration::from_millis(10)),
                }
            }
        }
        self.child.take();
        self.profile.take();
    }
}

type CdpSocket = WebSocketStream<TcpStream>;

#[derive(Clone)]
struct CdpClient {
    outgoing: mpsc::UnboundedSender<Message>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<CoreResult<Value>>>>>,
    next_id: Arc<AtomicU64>,
}

impl CdpClient {
    fn start(socket: CdpSocket) -> (Self, mpsc::Receiver<Value>) {
        let (sink, stream) = socket.split();
        let (outgoing, mut outgoing_rx) = mpsc::unbounded_channel::<Message>();
        let pending =
            Arc::new(Mutex::new(HashMap::<u64, oneshot::Sender<CoreResult<Value>>>::new()));
        let pending_reader = pending.clone();
        let (events_tx, events_rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut sink = sink;
            while let Some(message) = outgoing_rx.recv().await {
                if sink.send(message).await.is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            let mut stream = stream;
            while let Some(message) = stream.next().await {
                let Ok(message) = message else { break };
                let Message::Text(text) = message else { continue };
                let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    let command = pending_reader.lock().await.remove(&id);
                    if let Some(command) = command {
                        let result = if value.get("error").is_some() {
                            Err(browser_protocol_error())
                        } else {
                            Ok(value.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = command.send(result);
                    }
                } else {
                    if events_tx.try_send(value).is_err() {
                        break;
                    }
                }
            }
            let pending = std::mem::take(&mut *pending_reader.lock().await);
            for (_, waiter) in pending {
                let _ = waiter.send(Err(browser_protocol_error()));
            }
        });
        (Self { outgoing, pending, next_id: Arc::new(AtomicU64::new(1)) }, events_rx)
    }

    async fn command(&self, method: &str, params: Value) -> CoreResult<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        let command = json!({"id": id, "method": method, "params": params});
        if self.outgoing.send(Message::Text(command.to_string().into())).is_err() {
            self.pending.lock().await.remove(&id);
            return Err(browser_protocol_error());
        }
        receiver.await.map_err(|_| browser_protocol_error())?
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrowserFailure {
    Cancelled,
    Deadline,
    Protocol,
    ResponseLimit,
}

impl BrowserFailure {
    fn into_error(self) -> CoreError {
        match self {
            Self::Cancelled => CoreError::Cancelled,
            Self::Deadline => CoreError::Transport("browser deadline exceeded".into()),
            Self::Protocol => browser_protocol_error(),
            Self::ResponseLimit => {
                CoreError::ResourceLimit("browser response exceeds configured limit".into())
            }
        }
    }
}

struct CaptureRequest {
    target_url: Url,
    headers: Vec<(String, String)>,
    max_bytes: u64,
    sender: Option<oneshot::Sender<CoreResult<BrowserResponse>>>,
}

struct ActiveRequest {
    _request_permit: tokio::sync::OwnedSemaphorePermit,
    _host_permit: tokio::sync::OwnedSemaphorePermit,
    finished: CancellationToken,
}

struct BrowserRuntime {
    cdp: CdpClient,
    budget: Arc<RequestBudget>,
    policy: HostPolicy,
    limits_request_timeout: Duration,
    deadline: Instant,
    shutdown: CancellationToken,
    failure_tx: watch::Sender<Option<BrowserFailure>>,
    loaded_tx: watch::Sender<bool>,
    main_frame_id: Mutex<Option<String>>,
    active: Mutex<HashMap<String, ActiveRequest>>,
    capture: Mutex<Option<CaptureRequest>>,
    max_result_bytes: u64,
    max_profile_bytes: u64,
    process: Arc<ProcessControl>,
}

impl BrowserRuntime {
    fn fail(&self, failure: BrowserFailure) {
        if self.failure_tx.borrow().is_none() {
            self.failure_tx.send_replace(Some(failure));
            self.process.terminate();
        }
    }

    fn operation_deadline(&self) -> Instant {
        (Instant::now() + self.limits_request_timeout).min(self.deadline)
    }
}

/// A headless Chromium instance with an isolated profile and enforced host
/// policy. All methods are async; dropping the session also kills its process
/// group and schedules profile removal.
pub struct ChromiumSession {
    runtime: Arc<BrowserRuntime>,
    failure_rx: watch::Receiver<Option<BrowserFailure>>,
    loaded_rx: watch::Receiver<bool>,
    event_task: Option<JoinHandle<()>>,
    process: Option<ProcessGuard>,
}

impl ChromiumSession {
    /// Launch only after the engine has enabled networking. `temp_root` is the
    /// engine-provided staging directory and is used only for the private profile.
    pub async fn launch(
        context: &SourceContext,
        temp_root: &Path,
        config: ChromiumConfig,
    ) -> CoreResult<Self> {
        if !context.allow_network {
            return Err(CoreError::NetworkDisabled(
                "browser access requires network access".into(),
            ));
        }
        if context.request_budget.cancellation.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let policy = HostPolicy::new(&config.allowed_hosts)?;
        let executable = match config.executable {
            Some(path) if path.is_file() => path
                .canonicalize()
                .map_err(|_| CoreError::Transport("Chromium executable is unavailable".into()))?,
            Some(_) => {
                return Err(CoreError::Transport("Chromium executable is unavailable".into()));
            }
            None => find_chromium_executable()?,
        };
        std::fs::create_dir_all(temp_root).map_err(|_| {
            CoreError::Temporary("browser temporary directory is unavailable".into())
        })?;
        let profile = tempfile::Builder::new()
            .prefix("radiust-browser-")
            .tempdir_in(temp_root)
            .map_err(|_| CoreError::Temporary("browser profile could not be created".into()))?;
        let mut command = Command::new(executable);
        command
            .arg("--headless=new")
            .arg("--disable-gpu")
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .arg("--disable-sync")
            .arg("--disable-default-apps")
            .arg("--disable-extensions")
            .arg("--disable-breakpad")
            .arg("--disable-crash-reporter")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--remote-debugging-address=127.0.0.1")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(user_agent) = config.user_agent {
            command.arg(format!("--user-agent={user_agent}"));
        }
        // Chromium does not inherit the HTTP client's proxy environment.
        if let Some(proxy) = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
            .iter()
            .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        {
            let url = Url::parse(&proxy)
                .map_err(|_| CoreError::Transport("browser proxy URL is invalid".into()))?;
            if !matches!(url.scheme(), "http" | "https" | "socks5")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(CoreError::Transport("browser proxy URL is unsupported".into()));
            }
            command.arg(format!("--proxy-server={proxy}"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command
            .spawn()
            .map_err(|_| CoreError::Transport("Chromium could not be started".into()))?;
        let mut process = ProcessGuard::new(child, profile);
        let request_timeout = Duration::from_secs(context.limits.request_timeout_secs.max(1));
        let frame_deadline =
            Instant::now() + Duration::from_secs(context.limits.frame_deadline_secs.max(1));
        let startup_deadline =
            (Instant::now() + request_timeout.min(Duration::from_secs(30))).min(frame_deadline);
        let endpoint =
            wait_for_debugger(&mut process, &context.request_budget, startup_deadline).await?;
        let socket = connect_debugger(&endpoint, startup_deadline, &context.request_budget).await?;
        let (cdp, events) = CdpClient::start(socket);
        let browser_version = bounded(
            &context.request_budget,
            startup_deadline,
            cdp.command("Browser.getVersion", json!({})),
        )
        .await?;
        validate_browser_version(&browser_version)?;
        let mut blocked_urls = vec![
            "file://*".to_owned(),
            "ftp://*".to_owned(),
            "ws://*".to_owned(),
            "wss://*".to_owned(),
        ];
        blocked_urls.extend(config.blocked_urls);
        for (method, params) in [
            ("Page.enable", json!({})),
            ("Network.enable", json!({})),
            ("Network.setBypassServiceWorker", json!({"bypass": true})),
            ("Network.setCacheDisabled", json!({"cacheDisabled": true})),
            ("Network.setBlockedURLs", json!({"urls": blocked_urls})),
            (
                "Fetch.enable",
                json!({"patterns": [
                    {"urlPattern": "*", "requestStage": "Request"},
                    {"urlPattern": "*", "requestStage": "Response"}
                ]}),
            ),
        ] {
            bounded(&context.request_budget, startup_deadline, cdp.command(method, params)).await?;
        }
        let frame_tree = bounded(
            &context.request_budget,
            startup_deadline,
            cdp.command("Page.getFrameTree", json!({})),
        )
        .await?;
        let main_frame_id = frame_tree
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .ok_or_else(browser_protocol_error)?
            .to_owned();
        let add_script = bounded(
            &context.request_budget,
            startup_deadline,
            cdp.command(
                "Page.addScriptToEvaluateOnNewDocument",
                json!({"source": HARDENING_SCRIPT}),
            ),
        )
        .await?;
        if add_script.get("identifier").and_then(Value::as_str).is_none() {
            return Err(browser_protocol_error());
        }
        let hardening = bounded(
            &context.request_budget,
            startup_deadline,
            cdp.command(
                "Runtime.evaluate",
                json!({"expression": HARDENING_SCRIPT, "returnByValue": true}),
            ),
        )
        .await?;
        if evaluated_value(&hardening)?.as_bool() != Some(true) {
            return Err(CoreError::Transport("browser network hardening failed".into()));
        }
        let (failure_tx, failure_rx) = watch::channel(None);
        let (loaded_tx, loaded_rx) = watch::channel(false);
        let shutdown = CancellationToken::new();
        let process_control = process.control();
        let runtime = Arc::new(BrowserRuntime {
            cdp,
            budget: context.request_budget.clone(),
            policy,
            limits_request_timeout: request_timeout,
            deadline: frame_deadline,
            shutdown,
            failure_tx,
            loaded_tx,
            main_frame_id: Mutex::new(Some(main_frame_id)),
            active: Mutex::new(HashMap::new()),
            capture: Mutex::new(None),
            max_result_bytes: context.limits.max_artifact_bytes,
            max_profile_bytes: context.limits.max_temp_bytes,
            process: process_control,
        });
        let runtime_for_events = runtime.clone();
        let event_task =
            tokio::spawn(async move { run_event_loop(runtime_for_events, events).await });
        Ok(Self {
            runtime,
            failure_rx,
            loaded_rx,
            event_task: Some(event_task),
            process: Some(process),
        })
    }

    /// Navigate to an allow-listed HTTP(S) URL and wait for its load event.
    pub async fn navigate(&mut self, address: &str) -> CoreResult<NavigationReceipt> {
        if !self.runtime.policy.allows_url(address) {
            return self
                .fail_and_close(CoreError::Transport("browser URL is not allowed".into()))
                .await;
        }
        self.runtime.loaded_tx.send_replace(false);
        let response = match self.checked_command("Page.navigate", json!({"url": address})).await {
            Ok(response) => response,
            Err(error) => return self.fail_and_close(error).await,
        };
        if response.get("errorText").is_some() {
            return self
                .fail_and_close(CoreError::Transport("browser navigation failed".into()))
                .await;
        }
        let loader_id = response.get("loaderId").and_then(Value::as_str).map(str::to_owned);
        if loader_id.is_some() {
            if let Err(error) = self.wait_for_load().await {
                return self.fail_and_close(error).await;
            }
        }
        Ok(NavigationReceipt { loader_id })
    }

    /// Evaluate JavaScript in the page and return its JSON value without logging
    /// or formatting it. Results larger than the configured artifact limit fail closed.
    pub async fn evaluate(&mut self, expression: &str) -> CoreResult<Value> {
        let response = match self
            .checked_command(
                "Runtime.evaluate",
                json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
            )
            .await
        {
            Ok(response) => response,
            Err(error) => return self.fail_and_close(error).await,
        };
        let value = match evaluated_value(&response) {
            Ok(value) => value,
            Err(error) => return self.fail_and_close(error).await,
        };
        let encoded = serde_json::to_vec(&value)
            .map_err(|_| CoreError::Transport("browser result could not be read".into()))?;
        if encoded.len() as u64 > self.runtime.max_bytes() {
            return self
                .fail_and_close(CoreError::ResourceLimit(
                    "browser result exceeds configured limit".into(),
                ))
                .await;
        }
        Ok(value)
    }

    /// Read a concise snapshot of the current page. The values are returned raw
    /// to the caller; they are never added to diagnostics.
    pub async fn read_page(&mut self) -> CoreResult<Value> {
        self.evaluate(
            "({url:location.href,title:document.title,text:document.body?.innerText??''})",
        )
        .await
    }

    /// Optionally warm a page, extract a CSRF value, then return the target's
    /// raw main-document response. Extra headers are sent only on an exact URL
    /// match and never forwarded to a redirect or subresource.
    pub async fn fetch_response(
        &mut self,
        target_url: &str,
        warmup_url: Option<&str>,
        headers: &[(String, String)],
        csrf_selector: Option<&CsrfSelector>,
        max_bytes: u64,
    ) -> CoreResult<BrowserResponse> {
        if !self.runtime.policy.allows_url(target_url)
            || warmup_url.is_some_and(|url| !self.runtime.policy.allows_url(url))
        {
            return self
                .fail_and_close(CoreError::Transport("browser URL is not allowed".into()))
                .await;
        }
        let mut request_headers = match validate_headers(headers) {
            Ok(headers) => headers,
            Err(error) => return self.fail_and_close(error).await,
        };
        if let Some(warmup_url) = warmup_url {
            if let Err(error) = self.navigate(warmup_url).await {
                return Err(error);
            }
        }
        if let Some(selector) = csrf_selector {
            let expression = match csrf_expression(selector) {
                Ok(expression) => expression,
                Err(error) => return self.fail_and_close(error).await,
            };
            let token = match self.evaluate(&expression).await {
                Ok(Value::String(token)) if !token.is_empty() => token,
                Ok(_) => {
                    return self
                        .fail_and_close(CoreError::Transport(
                            "browser CSRF value was unavailable".into(),
                        ))
                        .await;
                }
                Err(error) => return Err(error),
            };
            let csrf_header = match validate_headers(&[(selector.header_name.clone(), token)]) {
                Ok(mut values) => values.pop().expect("one validated header"),
                Err(error) => return self.fail_and_close(error).await,
            };
            request_headers.push(csrf_header);
        }
        let target = match Url::parse(target_url) {
            Ok(target) => target,
            Err(_) => {
                return self
                    .fail_and_close(CoreError::Transport("browser URL is invalid".into()))
                    .await;
            }
        };
        if csrf_selector.is_some()
            && warmup_url
                .and_then(|warmup| Url::parse(warmup).ok())
                .is_some_and(|warmup| warmup.origin() != target.origin())
        {
            return self
                .fail_and_close(CoreError::Transport(
                    "browser CSRF header requires a same-origin target".into(),
                ))
                .await;
        }
        if max_bytes == 0 || max_bytes > self.runtime.max_result_bytes {
            return self
                .fail_and_close(CoreError::ResourceLimit(
                    "browser response exceeds configured limit".into(),
                ))
                .await;
        }
        let target_url = canonical_request_url(target);
        let (sender, receiver) = oneshot::channel();
        let capture_is_busy = {
            let mut capture = self.runtime.capture.lock().await;
            if capture.is_some() {
                true
            } else {
                *capture = Some(CaptureRequest {
                    target_url: target_url.clone(),
                    headers: request_headers.clone(),
                    max_bytes,
                    sender: Some(sender),
                });
                false
            }
        };
        if capture_is_busy {
            return self
                .fail_and_close(CoreError::Transport("browser response capture is busy".into()))
                .await;
        }
        self.runtime.loaded_tx.send_replace(false);
        let navigation =
            self.checked_command("Page.navigate", json!({"url": target_url.as_str()})).await;
        let navigation = match navigation {
            Ok(result) if result.get("errorText").is_none() => result,
            Ok(_) => {
                return self
                    .fail_and_close(CoreError::Transport("browser navigation failed".into()))
                    .await;
            }
            Err(error) => return self.fail_and_close(error).await,
        };
        let _ = navigation;
        let capture_result = wait_for_capture(&self.runtime, &mut self.failure_rx, receiver).await;
        match capture_result {
            Ok(response) => Ok(response),
            Err(error) => self.fail_and_close(error).await,
        }
    }

    /// Explicitly stop Chromium and remove its profile.
    pub async fn close(mut self) -> CoreResult<()> {
        self.runtime.shutdown.cancel();
        if let Some(task) = self.event_task.take() {
            task.abort();
        }
        match self.process.as_mut() {
            Some(process) => process.close().await,
            None => Ok(()),
        }
    }

    async fn checked_command(&mut self, method: &str, params: Value) -> CoreResult<Value> {
        let deadline = self.runtime.operation_deadline();
        let result = wait_with_failure(
            &self.runtime,
            &mut self.failure_rx,
            deadline,
            self.runtime.cdp.command(method, params),
        )
        .await;
        if result.is_err() {
            self.runtime.process.terminate();
        }
        result
    }

    async fn wait_for_load(&mut self) -> CoreResult<()> {
        let runtime = self.runtime.clone();
        let load = async {
            loop {
                if *self.loaded_rx.borrow() {
                    return Ok(());
                }
                self.loaded_rx.changed().await.map_err(|_| browser_protocol_error())?;
            }
        };
        wait_with_failure(&runtime, &mut self.failure_rx, runtime.operation_deadline(), load).await
    }

    async fn fail_and_close<T>(&mut self, error: CoreError) -> CoreResult<T> {
        self.runtime.shutdown.cancel();
        self.runtime.process.terminate();
        if let Some(task) = self.event_task.take() {
            task.abort();
        }
        if let Some(mut process) = self.process.take() {
            let _ = process.close().await;
        }
        Err(error)
    }
}

impl Drop for ChromiumSession {
    fn drop(&mut self) {
        self.runtime.shutdown.cancel();
        self.runtime.process.terminate();
        if let Some(task) = self.event_task.take() {
            task.abort();
        }
    }
}

impl BrowserRuntime {
    fn max_bytes(&self) -> u64 {
        self.capture
            .try_lock()
            .ok()
            .and_then(|capture| capture.as_ref().map(|capture| capture.max_bytes))
            .unwrap_or(self.max_result_bytes)
    }
}

async fn bounded<F, T>(budget: &RequestBudget, deadline: Instant, future: F) -> CoreResult<T>
where
    F: Future<Output = CoreResult<T>>,
{
    tokio::select! {
        biased;
        _ = budget.cancellation.cancelled() => Err(CoreError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => Err(CoreError::Transport("browser deadline exceeded".into())),
        result = future => result,
    }
}

async fn wait_with_failure<F, T>(
    runtime: &BrowserRuntime,
    failure_rx: &mut watch::Receiver<Option<BrowserFailure>>,
    deadline: Instant,
    future: F,
) -> CoreResult<T>
where
    F: Future<Output = CoreResult<T>>,
{
    tokio::select! {
        biased;
        _ = runtime.budget.cancellation.cancelled() => Err(CoreError::Cancelled),
        _ = tokio::time::sleep_until(deadline) => Err(CoreError::Transport("browser deadline exceeded".into())),
        changed = failure_rx.changed() => {
            if changed.is_err() { Err(browser_protocol_error()) }
            else { failure_rx.borrow().map_or_else(|| Err(browser_protocol_error()), |failure| Err(failure.into_error())) }
        }
        result = future => result,
    }
}

async fn wait_for_debugger(
    process: &mut ProcessGuard,
    budget: &RequestBudget,
    deadline: Instant,
) -> CoreResult<DebuggerEndpoint> {
    bounded(budget, deadline, async {
        loop {
            if let Some(status) =
                process.child.as_mut().and_then(|child| child.try_wait().ok().flatten())
            {
                let status = browser_exit_status(status);
                return Err(CoreError::Transport(format!(
                    "Chromium exited before CDP was ready ({status})"
                )));
            }
            let active_port = process.control.profile_path.join("DevToolsActivePort");
            if let Ok(contents) = tokio::fs::read_to_string(active_port).await {
                let mut lines = contents.lines();
                if let Some(port) = lines.next().and_then(|port| port.parse::<u16>().ok()) {
                    if port != 0 {
                        let client = reqwest::Client::builder()
                            .no_proxy()
                            .timeout(Duration::from_millis(500))
                            .build()
                            .map_err(|_| {
                                CoreError::Transport("Chromium CDP setup failed".into())
                            })?;
                        let endpoint = format!("http://127.0.0.1:{port}/json/list");
                        if let Ok(response) = client.get(endpoint).send().await {
                            if let Ok(bytes) = response.bytes().await
                                && let Ok(targets) = serde_json::from_slice::<Vec<Value>>(&bytes)
                            {
                                for target in targets {
                                    if target.get("type").and_then(Value::as_str) == Some("page") {
                                        if let Some(ws) = target
                                            .get("webSocketDebuggerUrl")
                                            .and_then(Value::as_str)
                                        {
                                            if validate_debugger_url(ws, port) {
                                                return Ok(DebuggerEndpoint {
                                                    url: ws.to_owned(),
                                                    port,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    })
    .await
}

fn browser_exit_status(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit code {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    "terminated without an exit code".to_owned()
}

#[derive(Clone, Debug)]
struct DebuggerEndpoint {
    url: String,
    port: u16,
}

fn validate_debugger_url(address: &str, expected_port: u16) -> bool {
    let Ok(url) = Url::parse(address) else {
        return false;
    };
    url.scheme() == "ws"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port() == Some(expected_port)
        && url.path().starts_with("/devtools/page/")
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]" | "::1"))
}

fn validate_browser_version(response: &Value) -> CoreResult<()> {
    let Some(product) = response.get("product").and_then(Value::as_str) else {
        return Err(CoreError::Transport("Chromium version is unsupported".into()));
    };
    let Some(version) = ["Chrome/", "Chromium/", "HeadlessChrome/"]
        .iter()
        .find_map(|prefix| product.strip_prefix(prefix))
    else {
        return Err(CoreError::Transport("Chromium version is unsupported".into()));
    };
    let Some(major) = version.split('.').next().and_then(|major| major.parse::<u32>().ok()) else {
        return Err(CoreError::Transport("Chromium version is unsupported".into()));
    };
    if major < 110 || response.get("protocolVersion").and_then(Value::as_str).is_none() {
        return Err(CoreError::Transport("Chromium version is unsupported".into()));
    }
    Ok(())
}

async fn connect_debugger(
    endpoint: &DebuggerEndpoint,
    deadline: Instant,
    budget: &RequestBudget,
) -> CoreResult<CdpSocket> {
    let host = Url::parse(&endpoint.url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .ok_or_else(browser_protocol_error)?;
    let address = if host == "localhost" || host == "::1" || host == "[::1]" {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), endpoint.port)
    } else {
        SocketAddr::new(host.parse().map_err(|_| browser_protocol_error())?, endpoint.port)
    };
    bounded(budget, deadline, async {
        let stream = TcpStream::connect(address).await.map_err(|_| browser_protocol_error())?;
        let (socket, _) = client_async(endpoint.url.as_str(), stream)
            .await
            .map_err(|_| browser_protocol_error())?;
        Ok(socket)
    })
    .await
}

async fn run_event_loop(runtime: Arc<BrowserRuntime>, mut events: mpsc::Receiver<Value>) {
    let shutdown = runtime.shutdown.clone();
    let budget_cancel = runtime.budget.cancellation.clone();
    let mut profile_check = tokio::time::interval_at(
        Instant::now() + Duration::from_millis(250),
        Duration::from_millis(250),
    );
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = budget_cancel.cancelled() => {
                runtime.fail(BrowserFailure::Cancelled);
                return;
            }
            _ = tokio::time::sleep_until(runtime.deadline) => {
                runtime.fail(BrowserFailure::Deadline);
                return;
            }
            _ = profile_check.tick() => {
                let profile_path = runtime.process.profile_path.clone();
                let max_bytes = runtime.max_profile_bytes;
                let exceeds_limit = tokio::task::spawn_blocking(move || {
                    directory_size_exceeds_limit(&profile_path, max_bytes)
                })
                .await
                .unwrap_or(true);
                if exceeds_limit {
                    runtime.fail(BrowserFailure::ResponseLimit);
                    return;
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    runtime.fail(BrowserFailure::Protocol);
                    return;
                };
                match event.get("method").and_then(Value::as_str) {
                    Some("Fetch.requestPaused") => {
                        let runtime = runtime.clone();
                        let params = event.get("params").cloned().unwrap_or(Value::Null);
                        tokio::spawn(async move { handle_paused_request(runtime, params).await });
                    }
                    Some("Network.loadingFinished") | Some("Network.loadingFailed") => {
                        if let Some(id) = event.pointer("/params/requestId").and_then(Value::as_str) {
                            finish_request(&runtime, id).await;
                        }
                    }
                    Some("Network.requestWillBeSent") => {
                        if event.pointer("/params/redirectResponse").is_some()
                            && let Some(id) = event.pointer("/params/requestId").and_then(Value::as_str)
                        {
                            finish_request(&runtime, id).await;
                        }
                    }
                    Some("Page.frameNavigated") => {
                        if let Some(frame) = event.pointer("/params/frame") {
                            let root = frame.get("parentId").is_none();
                            if root {
                                let frame_id = frame.get("id").and_then(Value::as_str);
                                let address = frame.get("url").and_then(Value::as_str).unwrap_or_default();
                                let mut main_id = runtime.main_frame_id.lock().await;
                                if main_id.is_none() {
                                    *main_id = frame_id.map(str::to_owned);
                                }
                                let is_main = main_id.as_deref() == frame_id;
                                drop(main_id);
                                if is_main && address != "about:blank" && !runtime.policy.allows_url(address) {
                                    runtime.fail(BrowserFailure::Protocol);
                                    return;
                                }
                            }
                        }
                    }
                    Some("Page.loadEventFired") => {
                        runtime.loaded_tx.send_replace(true);
                    }
                    _ => {}
                }
            }
        }
    }
}

fn directory_size_exceeds_limit(root: &Path, limit: u64) -> bool {
    let mut pending = vec![root.to_path_buf()];
    let mut total = 0_u64;
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return true;
        };
        for entry in entries {
            let Ok(entry) = entry else { return true };
            let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
                return true;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
                if total > limit {
                    return true;
                }
            }
        }
    }
    false
}

async fn handle_paused_request(runtime: Arc<BrowserRuntime>, params: Value) {
    let fetch_id = match params.get("requestId").and_then(Value::as_str) {
        Some(id) => id.to_owned(),
        None => {
            runtime.fail(BrowserFailure::Protocol);
            return;
        }
    };
    if params.get("responseStatusCode").is_some() {
        handle_paused_response(runtime, params, fetch_id).await;
        return;
    }
    let address = params.pointer("/request/url").and_then(Value::as_str).unwrap_or_default();
    let network_id = params.get("networkId").and_then(Value::as_str).map(str::to_owned);
    let Some(network_id) = network_id else {
        fail_paused_request(&runtime.cdp, &fetch_id).await;
        return;
    };
    if !runtime.policy.allows_url(address) {
        fail_paused_request(&runtime.cdp, &fetch_id).await;
        return;
    }
    let host = match Url::parse(address).ok().and_then(|url| url.host_str().map(str::to_owned)) {
        Some(host) => host,
        None => {
            fail_paused_request(&runtime.cdp, &fetch_id).await;
            return;
        }
    };
    let deadline = runtime.operation_deadline();
    let permit = bounded(&runtime.budget, deadline, runtime.budget.acquire_request()).await;
    let request_permit = match permit {
        Ok(permit) => permit,
        Err(CoreError::Cancelled) => {
            fail_paused_request(&runtime.cdp, &fetch_id).await;
            runtime.fail(BrowserFailure::Cancelled);
            return;
        }
        Err(_) => {
            fail_paused_request(&runtime.cdp, &fetch_id).await;
            runtime.fail(BrowserFailure::Deadline);
            return;
        }
    };
    let host_permit =
        match bounded(&runtime.budget, deadline, runtime.budget.acquire_host(&host)).await {
            Ok(permit) => permit,
            Err(CoreError::Cancelled) => {
                fail_paused_request(&runtime.cdp, &fetch_id).await;
                runtime.fail(BrowserFailure::Cancelled);
                return;
            }
            Err(_) => {
                fail_paused_request(&runtime.cdp, &fetch_id).await;
                runtime.fail(BrowserFailure::Deadline);
                return;
            }
        };
    let finished = CancellationToken::new();
    {
        let mut active = runtime.active.lock().await;
        if let Some(previous) = active.insert(
            network_id.clone(),
            ActiveRequest {
                _request_permit: request_permit,
                _host_permit: host_permit,
                finished: finished.clone(),
            },
        ) {
            previous.finished.cancel();
        }
    }
    let timeout_runtime = runtime.clone();
    let timeout_finished = finished.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = timeout_finished.cancelled() => {}
            _ = timeout_runtime.shutdown.cancelled() => {}
            _ = tokio::time::sleep_until(Instant::now() + timeout_runtime.limits_request_timeout) => {
                timeout_runtime.fail(BrowserFailure::Deadline);
            }
        }
    });
    let headers =
        target_headers(&runtime, address, params.get("frameId").and_then(Value::as_str)).await;
    let mut command_params = json!({"requestId": fetch_id});
    if !headers.is_empty() {
        command_params["headers"] = Value::Array(
            headers
                .into_iter()
                .map(|(name, value)| json!({"name": name, "value": value}))
                .collect(),
        );
    }
    if bounded(
        &runtime.budget,
        deadline,
        runtime.cdp.command("Fetch.continueRequest", command_params),
    )
    .await
    .is_err()
    {
        runtime.fail(BrowserFailure::Protocol);
    }
}

async fn handle_paused_response(runtime: Arc<BrowserRuntime>, params: Value, fetch_id: String) {
    let is_main_document = params.get("resourceType").and_then(Value::as_str) == Some("Document")
        && params.get("frameId").and_then(Value::as_str)
            == runtime.main_frame_id.lock().await.as_deref();
    let status = params.get("responseStatusCode").and_then(Value::as_u64).unwrap_or(0) as u16;
    let request_url = params
        .pointer("/request/url")
        .and_then(Value::as_str)
        .and_then(|address| Url::parse(address).ok())
        .map(canonical_request_url);
    let capture_matches = if is_main_document {
        let capture = runtime.capture.lock().await;
        capture.as_ref().is_some_and(|capture| Some(&capture.target_url) == request_url.as_ref())
    } else {
        false
    };
    if capture_matches && (300..400).contains(&status) {
        let _ = bounded(
            &runtime.budget,
            runtime.operation_deadline(),
            runtime.cdp.command(
                "Fetch.failRequest",
                json!({"requestId": fetch_id, "errorReason": "BlockedByClient"}),
            ),
        )
        .await;
        if let Some(mut capture) = runtime.capture.lock().await.take()
            && let Some(sender) = capture.sender.take()
        {
            let _ =
                sender.send(Err(CoreError::Transport("browser navigation was redirected".into())));
        }
        return;
    }
    let capture = if capture_matches { runtime.capture.lock().await.take() } else { None };
    let Some(mut capture) = capture else {
        if bounded(
            &runtime.budget,
            runtime.operation_deadline(),
            runtime.cdp.command("Fetch.continueResponse", json!({"requestId": fetch_id})),
        )
        .await
        .is_err()
        {
            runtime.fail(BrowserFailure::Protocol);
        }
        return;
    };
    let headers = params.get("responseHeaders").and_then(Value::as_array);
    let content_type = headers.and_then(|headers| {
        headers.iter().find_map(|header| {
            (header
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case("content-type")))
            .then(|| header.get("value").and_then(Value::as_str).map(str::to_owned))
            .flatten()
        })
    });
    let content_length = headers.and_then(|headers| {
        headers.iter().find_map(|header| {
            header
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| name.eq_ignore_ascii_case("content-length"))
                .and_then(|_| header.get("value").and_then(Value::as_str))
                .and_then(|value| value.parse::<u64>().ok())
        })
    });
    if content_length.is_some_and(|length| length > capture.max_bytes) {
        let _ = bounded(
            &runtime.budget,
            runtime.operation_deadline(),
            runtime.cdp.command(
                "Fetch.failRequest",
                json!({"requestId": fetch_id, "errorReason": "BlockedByClient"}),
            ),
        )
        .await;
        if let Some(sender) = capture.sender.take() {
            let _ = sender.send(Err(CoreError::ResourceLimit(
                "browser response exceeds configured limit".into(),
            )));
        }
        runtime.fail(BrowserFailure::ResponseLimit);
        return;
    }
    let body_result = read_response_stream(&runtime, &fetch_id, capture.max_bytes).await;
    let response_result = body_result.map(|body| BrowserResponse { status, content_type, body });
    let too_large = matches!(response_result, Err(CoreError::ResourceLimit(_)));
    let failed_request = bounded(
        &runtime.budget,
        runtime.operation_deadline(),
        runtime.cdp.command(
            "Fetch.failRequest",
            json!({"requestId": fetch_id, "errorReason": "BlockedByClient"}),
        ),
    )
    .await;
    if failed_request.is_err() {
        runtime.fail(BrowserFailure::Protocol);
        return;
    }
    if too_large {
        if let Some(sender) = capture.sender.take() {
            let _ = sender.send(response_result);
        }
        runtime.fail(BrowserFailure::ResponseLimit);
        return;
    }
    if let Some(sender) = capture.sender.take() {
        let _ = sender.send(response_result);
    }
}

async fn read_response_stream(
    runtime: &BrowserRuntime,
    request_id: &str,
    max_bytes: u64,
) -> CoreResult<Vec<u8>> {
    const CHUNK_BYTES: u64 = 64 * 1024;
    let stream = bounded(
        &runtime.budget,
        runtime.operation_deadline(),
        runtime.cdp.command("Fetch.takeResponseBodyAsStream", json!({"requestId": request_id})),
    )
    .await?;
    let handle = stream.get("stream").and_then(Value::as_str).ok_or_else(browser_protocol_error)?;
    let read_result = async {
        let mut body = Vec::new();
        loop {
            let chunk = bounded(
                &runtime.budget,
                runtime.operation_deadline(),
                runtime.cdp.command("IO.read", json!({"handle": handle, "size": CHUNK_BYTES})),
            )
            .await?;
            let bytes = decode_response_body(&chunk)?;
            let new_len = (body.len() as u64)
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| CoreError::ResourceLimit("browser response size overflow".into()))?;
            if new_len > max_bytes {
                return Err(CoreError::ResourceLimit(
                    "browser response exceeds configured limit".into(),
                ));
            }
            body.extend_from_slice(&bytes);
            if chunk.get("eof").and_then(Value::as_bool) == Some(true) {
                return Ok(body);
            }
        }
    }
    .await;
    let close_result = bounded(
        &runtime.budget,
        runtime.operation_deadline(),
        runtime.cdp.command("IO.close", json!({"handle": handle})),
    )
    .await;
    match (read_result, close_result) {
        (Ok(body), Ok(_)) => Ok(body),
        (Err(error), _) => Err(error),
        (_, Err(error)) => Err(error),
    }
}

fn decode_response_body(value: &Value) -> CoreResult<Vec<u8>> {
    let body = value.get("data").and_then(Value::as_str).ok_or_else(browser_protocol_error)?;
    if value.get("base64Encoded").and_then(Value::as_bool) == Some(true) {
        base64::engine::general_purpose::STANDARD.decode(body).map_err(|_| browser_protocol_error())
    } else {
        Ok(body.as_bytes().to_vec())
    }
}

async fn fail_paused_request(cdp: &CdpClient, request_id: &str) {
    let _ = cdp
        .command(
            "Fetch.failRequest",
            json!({"requestId": request_id, "errorReason": "BlockedByClient"}),
        )
        .await;
}

async fn target_headers(
    runtime: &BrowserRuntime,
    address: &str,
    frame_id: Option<&str>,
) -> Vec<(String, String)> {
    if frame_id != runtime.main_frame_id.lock().await.as_deref() {
        return Vec::new();
    }
    let Ok(request_url) = Url::parse(address) else {
        return Vec::new();
    };
    let request_url = canonical_request_url(request_url);
    let capture = runtime.capture.lock().await;
    capture
        .as_ref()
        .filter(|capture| capture.sender.is_some() && capture.target_url == request_url)
        .map(|capture| capture.headers.clone())
        .unwrap_or_default()
}

fn canonical_request_url(mut url: Url) -> Url {
    url.set_fragment(None);
    url
}

async fn finish_request(runtime: &BrowserRuntime, id: &str) {
    if let Some(active) = runtime.active.lock().await.remove(id) {
        active.finished.cancel();
    }
}

async fn wait_for_capture(
    runtime: &BrowserRuntime,
    failure_rx: &mut watch::Receiver<Option<BrowserFailure>>,
    receiver: oneshot::Receiver<CoreResult<BrowserResponse>>,
) -> CoreResult<BrowserResponse> {
    let deadline = runtime.operation_deadline();
    wait_with_failure(runtime, failure_rx, deadline, async {
        receiver.await.map_err(|_| browser_protocol_error())?
    })
    .await
}

fn validate_headers(headers: &[(String, String)]) -> CoreResult<Vec<(String, String)>> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| CoreError::Transport("browser request header is invalid".into()))?;
            let value = reqwest::header::HeaderValue::from_bytes(value.as_bytes())
                .map_err(|_| CoreError::Transport("browser request header is invalid".into()))?;
            Ok((name.as_str().to_owned(), value.to_str().unwrap_or_default().to_owned()))
        })
        .collect()
}

fn csrf_expression(selector: &CsrfSelector) -> CoreResult<String> {
    if selector.css.is_empty() || selector.attribute.is_empty() {
        return Err(CoreError::Transport("browser CSRF selector is invalid".into()));
    }
    let css = serde_json::to_string(&selector.css)
        .map_err(|_| CoreError::Transport("browser CSRF selector is invalid".into()))?;
    let attribute = serde_json::to_string(&selector.attribute)
        .map_err(|_| CoreError::Transport("browser CSRF selector is invalid".into()))?;
    Ok(format!(
        "(()=>{{const e=document.querySelector({css});return e?e.getAttribute({attribute}):null}})()"
    ))
}

fn browser_protocol_error() -> CoreError {
    CoreError::Transport("Chromium CDP operation failed".into())
}

fn evaluated_value(response: &Value) -> CoreResult<Value> {
    if response.get("exceptionDetails").is_some() {
        return Err(CoreError::Transport("browser JavaScript evaluation failed".into()));
    }
    Ok(response.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ftp::FtpTransport;
    use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
    use futures_util::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn host_policy_matches_exact_canonical_hosts_only() {
        let policy = HostPolicy::new(&["Example.COM".into()]).unwrap();
        assert!(policy.allows_url("https://example.com/path"));
        assert!(!policy.allows_url("https://sub.example.com/path"));
        assert!(!policy.allows_url("https://example.com.evil.test/path"));
        assert!(!policy.allows_url("https://example.com:8443/path"));
        assert!(!policy.allows_url("file:///etc/passwd"));
        assert!(!policy.allows_url("https://user@example.com/path"));
        assert!(HostPolicy::new(&["*.example.com".into()]).is_err());
    }

    #[test]
    fn debugger_endpoint_requires_the_random_loopback_port() {
        assert!(validate_debugger_url("ws://127.0.0.1:43127/devtools/page/test", 43127));
        assert!(!validate_debugger_url("ws://127.0.0.1:43128/devtools/page/test", 43127));
        assert!(!validate_debugger_url("ws://192.0.2.1:43127/devtools/page/test", 43127));
        assert!(!validate_debugger_url("wss://127.0.0.1:43127/devtools/page/test", 43127));
    }

    #[test]
    fn browser_version_must_be_a_supported_chromium_product() {
        assert!(
            validate_browser_version(&json!({
                "product": "Chrome/151.0.7882.0",
                "protocolVersion": "1.3"
            }))
            .is_ok()
        );
        assert!(
            validate_browser_version(&json!({
                "product": "HeadlessChrome/151.0.7882.0",
                "protocolVersion": "1.3"
            }))
            .is_ok()
        );
        assert!(
            validate_browser_version(&json!({
                "product": "Chrome/109.0.0.0",
                "protocolVersion": "1.3"
            }))
            .is_err()
        );
        assert!(
            validate_browser_version(&json!({
                "product": "Firefox/151.0",
                "protocolVersion": "1.3"
            }))
            .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires an explicitly configured real Chromium executable"]
    async fn real_chromium_cdp_handshake_and_private_profile_cleanup() {
        let executable = std::env::var_os("RADIUST_CHROMIUM_EXECUTABLE")
            .map(PathBuf::from)
            .expect("set RADIUST_CHROMIUM_EXECUTABLE to the Chromium executable");
        let limits = Limits::default();
        let budget = Arc::new(RequestBudget::new(&limits));
        let context = SourceContext {
            query: crate::model::Query::default(),
            allow_network: true,
            discovery_workers: 1,
            source_options: Arc::new(std::collections::BTreeMap::new()),
            request_budget: budget.clone(),
            limits: limits.clone(),
            http_transport: Arc::new(
                HttpTransport::with_budget(limits.clone(), false, budget.clone()).unwrap(),
            ),
            ftp_transport: Arc::new(FtpTransport::with_budget(limits, false, budget)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        };
        let temporary = tempfile::tempdir().unwrap();
        let session = tokio::time::timeout(
            Duration::from_secs(45),
            ChromiumSession::launch(
                &context,
                temporary.path(),
                ChromiumConfig::new(["127.0.0.1"]).with_executable(executable),
            ),
        )
        .await
        .expect("real Chromium startup exceeded its deadline")
        .expect("real Chromium CDP handshake failed");
        assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 1);
        session.close().await.unwrap();
        assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 0);
    }

    #[test]
    fn runtime_evaluate_reads_the_protocol_remote_object_value() {
        assert_eq!(
            evaluated_value(&json!({"result": {"type": "boolean", "value": true}})).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            evaluated_value(&json!({"result": {"type": "string", "value": "csrf-token"}})).unwrap(),
            Value::String("csrf-token".into())
        );
        assert!(evaluated_value(&json!({"exceptionDetails": {"text": "failure"}})).is_err());
    }

    #[test]
    fn csrf_expression_quotes_page_selectors_as_javascript_data() {
        let selector = CsrfSelector::new("meta[data-x='\\\"value']", "content", "X-CSRF-TOKEN");
        let expression = csrf_expression(&selector).unwrap();
        assert!(expression.contains("document.querySelector(\"meta[data-x='\\\\\\\"value']\")"));
        assert!(csrf_expression(&CsrfSelector::new("", "content", "X-CSRF-TOKEN")).is_err());
    }

    #[test]
    fn response_debug_output_never_contains_body_bytes() {
        let response = BrowserResponse {
            status: 200,
            content_type: Some("text/plain".into()),
            body: b"private-token-value".to_vec(),
        };
        let debug = format!("{response:?}");
        assert!(debug.contains("body_bytes"));
        assert!(!debug.contains("private-token-value"));
    }

    #[test]
    fn browser_profile_disk_usage_is_bounded_without_following_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("profile.data"), b"12345").unwrap();
        assert!(directory_size_exceeds_limit(directory.path(), 4));
        assert!(!directory_size_exceeds_limit(directory.path(), 5));
    }

    #[tokio::test]
    async fn bounded_operation_honors_request_deadline_and_cancellation() {
        let limits = Limits::default();
        let budget = RequestBudget::new(&limits);
        let deadline = Instant::now() + Duration::from_millis(10);
        let timed_out = bounded(&budget, deadline, std::future::pending::<CoreResult<()>>()).await;
        assert!(
            matches!(timed_out, Err(CoreError::Transport(message)) if message.contains("deadline"))
        );

        budget.cancel();
        let cancelled = bounded(
            &budget,
            Instant::now() + Duration::from_secs(1),
            std::future::pending::<CoreResult<()>>(),
        )
        .await;
        assert!(matches!(cancelled, Err(CoreError::Cancelled)));
    }

    #[tokio::test]
    async fn cdp_stub_applies_headers_only_to_target_and_streams_raw_response() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Value>();
        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<Value>();
        let response_bytes = vec![0, 114, 97, 119, 13, 10, 255];
        let encoded_body = base64::engine::general_purpose::STANDARD.encode(&response_bytes);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (mut sink, mut source) = websocket.split();
            loop {
                tokio::select! {
                    message = source.next() => {
                        let Some(Ok(Message::Text(message))) = message else { break };
                        let command: Value = serde_json::from_str(&message).unwrap();
                        let method = command.get("method").and_then(Value::as_str).unwrap_or_default();
                        if method == "Fetch.continueRequest" {
                            let _ = command_tx.send(command.clone());
                        }
                        let result = match method {
                            "Fetch.takeResponseBodyAsStream" => json!({"stream": "stub-body"}),
                            "IO.read" => json!({"data": encoded_body, "base64Encoded": true, "eof": true}),
                            _ => json!({}),
                        };
                        let reply = json!({"id": command.get("id").cloned().unwrap_or(Value::Null), "result": result});
                        if sink.send(Message::Text(reply.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    event = event_rx.recv() => {
                        let Some(event) = event else { break };
                        if sink.send(Message::Text(event.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let tcp = TcpStream::connect(address).await.unwrap();
        let (socket, _) =
            client_async(format!("ws://127.0.0.1:{}/devtools/page/stub", address.port()), tcp)
                .await
                .unwrap();
        let (cdp, events) = CdpClient::start(socket);
        let limits = Limits::default();
        let budget = Arc::new(RequestBudget::new(&limits));
        let (failure_tx, failure_rx) = watch::channel(None);
        let (loaded_tx, _loaded_rx) = watch::channel(false);
        let profile = tempfile::tempdir().unwrap();
        let profile_path = profile.path().to_path_buf();
        let mut process_command = Command::new("sleep");
        process_command.arg("30");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            process_command.process_group(0);
        }
        let child = process_command.spawn().unwrap();
        let mut process = ProcessGuard::new(child, profile);
        let target = Url::parse("https://api.example.test/timeline?token=secret").unwrap();
        let (capture_tx, capture_rx) = oneshot::channel();
        let runtime = Arc::new(BrowserRuntime {
            cdp,
            budget,
            policy: HostPolicy::new(&["api.example.test".into()]).unwrap(),
            limits_request_timeout: Duration::from_secs(3),
            deadline: Instant::now() + Duration::from_secs(5),
            shutdown: CancellationToken::new(),
            failure_tx,
            loaded_tx,
            main_frame_id: Mutex::new(Some("main-frame".into())),
            active: Mutex::new(HashMap::new()),
            capture: Mutex::new(Some(CaptureRequest {
                target_url: target.clone(),
                headers: vec![("X-CSRF-TOKEN".into(), "private-value".into())],
                max_bytes: 64,
                sender: Some(capture_tx),
            })),
            max_result_bytes: 1024,
            max_profile_bytes: 1024,
            process: process.control(),
        });
        let event_task = tokio::spawn(run_event_loop(runtime.clone(), events));
        assert!(
            target_headers(&runtime, "https://api.example.test/other", Some("main-frame"))
                .await
                .is_empty()
        );
        assert!(target_headers(&runtime, target.as_str(), Some("sub-frame")).await.is_empty());
        event_tx
            .send(json!({
                "method": "Fetch.requestPaused",
                "params": {
                    "requestId": "fetch-request",
                    "networkId": "network-request",
                    "frameId": "main-frame",
                    "resourceType": "Document",
                    "request": {"url": target.as_str()}
                }
            }))
            .unwrap();
        let continued =
            tokio::time::timeout(Duration::from_secs(2), command_rx.recv()).await.unwrap().unwrap();
        assert_eq!(
            continued.pointer("/params/headers/0/name").and_then(Value::as_str),
            Some("X-CSRF-TOKEN")
        );
        assert_eq!(
            continued.pointer("/params/headers/0/value").and_then(Value::as_str),
            Some("private-value")
        );

        event_tx
            .send(json!({
                "method": "Fetch.requestPaused",
                "params": {
                    "requestId": "fetch-response",
                    "networkId": "network-request",
                    "frameId": "main-frame",
                    "resourceType": "Document",
                    "request": {"url": target.as_str()},
                    "responseStatusCode": 206,
                    "responseHeaders": [
                        {"name": "content-type", "value": "application/octet-stream"},
                        {"name": "content-length", "value": response_bytes.len().to_string()}
                    ]
                }
            }))
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), capture_rx)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(response.status, 206);
        assert_eq!(response.content_type.as_deref(), Some("application/octet-stream"));
        assert_eq!(response.body, response_bytes);
        assert!(failure_rx.borrow().is_none());

        event_tx
            .send(json!({"method": "Network.loadingFailed", "params": {"requestId": "network-request"}}))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if runtime.active.lock().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        runtime.shutdown.cancel();
        event_task.await.unwrap();
        drop(runtime);
        process.close().await.unwrap();
        assert!(!profile_path.exists());
        server.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_launch_completes_the_cdp_startup_contract_and_cleans_up() {
        use crate::limits::{Limits, RequestBudget};
        use crate::source::SourceContext;
        use crate::transport::ftp::FtpTransport;
        use crate::transport::http::{HttpRequestCoalescer, HttpTransport};
        use std::collections::BTreeMap;
        use std::os::unix::fs::PermissionsExt;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let (csrf_header_tx, csrf_header_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut http, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = http.read(&mut chunk).await.unwrap();
                assert!(count > 0, "CDP discovery request ended early");
                request.extend_from_slice(&chunk[..count]);
            }
            let body = json!([{
                "type": "page",
                "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/page/fake")
            }])
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            http.write_all(response.as_bytes()).await.unwrap();
            drop(http);

            let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("fake Chromium did not open its CDP websocket")
                .unwrap();
            let websocket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (mut sink, mut source) = websocket.split();
            let response_bytes = b"{\"fixture\":true}".to_vec();
            let encoded_body = base64::engine::general_purpose::STANDARD.encode(&response_bytes);
            let mut csrf_header_tx = Some(csrf_header_tx);
            loop {
                let message = tokio::select! {
                    _ = &mut shutdown_rx => break,
                    message = source.next() => message,
                };
                let Some(message) = message else { break };
                let Ok(Message::Text(message)) = message else { break };
                let command: Value = serde_json::from_str(&message).unwrap();
                let method = command.get("method").and_then(Value::as_str).unwrap_or_default();
                let result = match method {
                    "Browser.getVersion" => json!({
                        "product": "Chrome/151.0.7882.0",
                        "protocolVersion": "1.3"
                    }),
                    "Page.getFrameTree" => {
                        json!({"frameTree": {"frame": {"id": "main-frame"}}})
                    }
                    "Page.addScriptToEvaluateOnNewDocument" => {
                        json!({"identifier": "script-1"})
                    }
                    "Runtime.evaluate" => {
                        let expression = command
                            .pointer("/params/expression")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if expression.contains("document.querySelector") {
                            json!({"result": {"type": "string", "value": "fixture-token"}})
                        } else {
                            json!({"result": {"type": "boolean", "value": true}})
                        }
                    }
                    "Page.navigate" => json!({"frameId": "main-frame"}),
                    "Fetch.takeResponseBodyAsStream" => json!({"stream": "body-1"}),
                    "IO.read" => {
                        json!({"data": encoded_body.clone(), "base64Encoded": true, "eof": true})
                    }
                    _ => json!({}),
                };
                let reply = json!({
                    "id": command.get("id").cloned().unwrap_or(Value::Null),
                    "result": result
                });
                if sink.send(Message::Text(reply.to_string().into())).await.is_err() {
                    break;
                }
                match method {
                    "Page.navigate"
                        if command.pointer("/params/url").and_then(Value::as_str)
                            == Some("https://example.test/api/timeline") =>
                    {
                        let event = json!({
                            "method": "Fetch.requestPaused",
                            "params": {
                                "requestId": "fetch-request",
                                "networkId": "network-request",
                                "frameId": "main-frame",
                                "resourceType": "Document",
                                "request": {"url": "https://example.test/api/timeline"}
                            }
                        });
                        if sink.send(Message::Text(event.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    "Fetch.continueRequest" => {
                        let header =
                            command.pointer("/params/headers/0").cloned().unwrap_or(Value::Null);
                        if let Some(sender) = csrf_header_tx.take() {
                            let _ = sender.send(header);
                        }
                        let event = json!({
                            "method": "Fetch.requestPaused",
                            "params": {
                                "requestId": "fetch-response",
                                "networkId": "network-request",
                                "frameId": "main-frame",
                                "resourceType": "Document",
                                "request": {"url": "https://example.test/api/timeline"},
                                "responseStatusCode": 200,
                                "responseHeaders": [
                                    {"name": "content-type", "value": "application/json"},
                                    {"name": "content-length", "value": response_bytes.len().to_string()}
                                ]
                            }
                        });
                        if sink.send(Message::Text(event.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        });

        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("fake-chromium");
        let script = format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\n  case \"$arg\" in\n    --user-data-dir=*) profile=${{arg#*=}} ;;\n  esac\ndone\nprintf '%s\\n%s\\n' '{port}' '/devtools/browser/fake' > \"$profile/DevToolsActivePort\"\nexec /bin/sleep 30\n"
        );
        std::fs::write(&executable, script).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

        let limits = Limits::default();
        let budget = Arc::new(RequestBudget::new(&limits));
        let context = SourceContext {
            query: crate::model::Query::default(),
            allow_network: true,
            discovery_workers: 1,
            source_options: Arc::new(BTreeMap::new()),
            request_budget: budget.clone(),
            limits: limits.clone(),
            http_transport: Arc::new(
                HttpTransport::with_budget(limits.clone(), false, budget.clone()).unwrap(),
            ),
            ftp_transport: Arc::new(FtpTransport::with_budget(limits, false, budget)),
            request_coalescer: Arc::new(HttpRequestCoalescer::default()),
        };
        let mut session = tokio::time::timeout(
            Duration::from_secs(20),
            ChromiumSession::launch(
                &context,
                temporary.path(),
                ChromiumConfig::new(["example.test"]).with_executable(&executable),
            ),
        )
        .await
        .expect("browser startup exceeded its test deadline")
        .unwrap();
        assert_eq!(
            std::fs::read_dir(temporary.path()).unwrap().count(),
            2,
            "the fake executable and private browser profile should exist"
        );
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            session.fetch_response(
                "https://example.test/api/timeline",
                Some("https://example.test/"),
                &[],
                Some(&CsrfSelector::new("meta[name=csrf-token]", "content", "X-CSRF-TOKEN")),
                1024,
            ),
        )
        .await
        .expect("fake browser response capture exceeded its test deadline")
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type.as_deref(), Some("application/json"));
        assert_eq!(response.body, b"{\"fixture\":true}");
        let csrf_header = csrf_header_rx.await.unwrap();
        assert_eq!(csrf_header.get("name").and_then(Value::as_str), Some("x-csrf-token"));
        assert_eq!(csrf_header.get("value").and_then(Value::as_str), Some("fixture-token"));
        session.close().await.unwrap();
        assert_eq!(std::fs::read_dir(temporary.path()).unwrap().count(), 1);
        let _ = shutdown_tx.send(());
        server.await.unwrap();
    }
}
