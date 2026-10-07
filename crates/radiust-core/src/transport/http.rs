use crate::errors::{CoreError, CoreResult};
use crate::limits::{Limits, RequestBudget};
use futures_util::StreamExt;
use reqwest::Client;
use reqwest::header::{HeaderName, HeaderValue};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, OnceCell};
use url::Url;

#[derive(Clone)]
pub struct HttpTransport {
    client: Client,
    rdcap_client: Option<Client>,
    budget: Arc<RequestBudget>,
    allow_public: bool,
    max_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpMetadata {
    pub status: u16,
    /// Only non-secret response metadata is returned to provider adapters.
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpBodyReceipt {
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpGetPolicy {
    /// Preserve the transport's existing transient-status retries.
    RetryTransient,
    /// Make one initial GET attempt and do not retry transient statuses.
    SingleAttempt,
}

/// In-flight and completed GET responses shared only within one Engine
/// discovery call. Create a fresh instance for the next query so listings do
/// not become stale across user operations.
#[derive(Clone, Default)]
pub struct HttpRequestCoalescer {
    calls: Arc<Mutex<HashMap<String, Arc<OnceCell<Result<Arc<[u8]>, CoreError>>>>>>,
}

impl HttpTransport {
    pub fn new(limits: Limits, allow_public: bool) -> CoreResult<Self> {
        let budget = Arc::new(RequestBudget::new(&limits));
        Self::with_budget(limits, allow_public, budget)
    }

    pub fn with_budget(
        limits: Limits,
        allow_public: bool,
        budget: Arc<RequestBudget>,
    ) -> CoreResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(limits.request_timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        Ok(Self {
            client,
            rdcap_client: None,
            budget,
            allow_public,
            max_bytes: limits.max_artifact_bytes,
        })
    }

    /// Maintain the anonymous index/file session only for RDCAP HTTPS requests.
    /// The TLS exception is optional; budget and network policy remain shared.
    pub(crate) fn with_rdcap_session(
        mut self,
        limits: &Limits,
        insecure_tls: bool,
    ) -> CoreResult<Self> {
        self.rdcap_client = Some(
            Client::builder()
                .timeout(Duration::from_secs(limits.request_timeout_secs))
                .redirect(reqwest::redirect::Policy::none())
                .danger_accept_invalid_certs(insecure_tls)
                .cookie_store(true)
                .build()
                .map_err(|_| {
                    CoreError::Transport("RDCAP TLS client could not be initialized".into())
                })?,
        );
        Ok(self)
    }

    fn client_for(&self, url: &Url) -> &Client {
        if url.scheme() == "https" && url.host_str() == Some("rdcap.cwa.gov.tw") {
            if let Some(client) = &self.rdcap_client {
                return client;
            }
        }
        &self.client
    }

    fn allowed(&self, url: &Url) -> bool {
        if self.allow_public || std::env::var("RADIUST_TEST_ALLOW_LIVE").as_deref() == Ok("1") {
            return true;
        }
        matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
    }

    fn validate_url(&self, url: &Url) -> CoreResult<()> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(CoreError::Transport("only http and https are supported".into()));
        }
        if !self.allowed(url) {
            return Err(CoreError::NetworkDisabled(url.to_string()));
        }
        Ok(())
    }

    async fn wait_retry(&self, delay: Duration) -> CoreResult<()> {
        tokio::select! {
            _ = self.budget.cancellation.cancelled() => Err(CoreError::Cancelled),
            _ = tokio::time::sleep(delay) => Ok(()),
        }
    }

    pub async fn get_bytes(&self, address: &str) -> CoreResult<Vec<u8>> {
        self.get_bytes_with_headers(address, &[]).await
    }

    pub async fn get_bytes_with_headers(
        &self,
        address: &str,
        headers: &[(&str, &str)],
    ) -> CoreResult<Vec<u8>> {
        let url = Url::parse(address).map_err(|e| CoreError::Transport(e.to_string()))?;
        self.validate_url(&url)?;
        let request_headers = headers
            .iter()
            .map(|(name, value)| {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| CoreError::Transport("request header is invalid".into()))?;
                let value = HeaderValue::from_bytes(value.as_bytes())
                    .map_err(|_| CoreError::Transport("request header is invalid".into()))?;
                Ok((name, value))
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let original_origin = url.origin().ascii_serialization();
        for attempt in 0..3 {
            let mut current = url.clone();
            let mut redirects = 0_u8;
            let response = loop {
                self.validate_url(&current)?;
                let host = current
                    .host_str()
                    .ok_or_else(|| CoreError::Transport("request URL has no host".into()))?;
                let _host_permit = self.budget.acquire_host(host).await?;
                let _permit = self.budget.acquire_request().await?;
                let mut request = self.client_for(&current).get(current.clone());
                if current.origin().ascii_serialization() == original_origin {
                    for (name, value) in &request_headers {
                        request = request.header(name, value);
                    }
                }
                let response = tokio::select! {
                    result = request.send() => result.map_err(|_| CoreError::Transport("request failed".into()))?,
                    _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
                };
                if response.status().is_redirection() {
                    if redirects >= 10 {
                        return Err(CoreError::Transport("too many HTTP redirects".into()));
                    }
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            CoreError::Transport("redirect missing Location header".into())
                        })?;
                    current =
                        current.join(location).map_err(|e| CoreError::Transport(e.to_string()))?;
                    self.validate_url(&current)?;
                    redirects += 1;
                    continue;
                }
                break response;
            };
            let status = response.status();
            if !status.is_success() {
                if matches!(status.as_u16(), 408 | 425 | 429) || status.is_server_error() {
                    if attempt < 2 {
                        self.wait_retry(retry_delay(
                            response.headers().get("retry-after"),
                            attempt,
                        ))
                        .await?;
                        continue;
                    }
                }
                return Err(CoreError::Transport(format!("HTTP status {}", status.as_u16())));
            }
            if let Some(length) = response.content_length() {
                if length > self.max_bytes {
                    return Err(CoreError::ResourceLimit(format!(
                        "response {length} > {}",
                        self.max_bytes
                    )));
                }
            }
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = tokio::select! {
                value = stream.next() => value,
                _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
            } {
                let chunk =
                    chunk.map_err(|_| CoreError::Transport("response stream failed".into()))?;
                if bytes.len() as u64 + chunk.len() as u64 > self.max_bytes {
                    return Err(CoreError::ResourceLimit(format!(
                        "response exceeds {}",
                        self.max_bytes
                    )));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(bytes);
        }
        Err(CoreError::Transport("request failed after the retry budget was exhausted".into()))
    }

    /// Submit an URL-encoded form using the shared request budget. Redirects
    /// are rejected so a POST body is never replayed to a different endpoint.
    pub async fn post_form_bytes_with_headers(
        &self,
        address: &str,
        form: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> CoreResult<Vec<u8>> {
        let url = Url::parse(address).map_err(|e| CoreError::Transport(e.to_string()))?;
        self.validate_url(&url)?;

        let mut request_headers = reqwest::header::HeaderMap::new();
        request_headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| CoreError::Transport("request header is invalid".into()))?;
            let value = HeaderValue::from_bytes(value.as_bytes())
                .map_err(|_| CoreError::Transport("request header is invalid".into()))?;
            request_headers.insert(name, value);
        }

        let body = {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (name, value) in form {
                serializer.append_pair(name, value);
            }
            serializer.finish()
        };
        let host =
            url.host_str().ok_or_else(|| CoreError::Transport("request URL has no host".into()))?;

        for attempt in 0..3 {
            let _host_permit = self.budget.acquire_host(host).await?;
            let _permit = self.budget.acquire_request().await?;
            let response = tokio::select! {
                result = self.client_for(&url)
                    .post(url.clone())
                    .headers(request_headers.clone())
                    .body(body.clone())
                    .send() => result.map_err(|_| CoreError::Transport("request failed".into()))?,
                _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
            };

            let status = response.status();
            if status.is_redirection() {
                return Err(CoreError::Transport("POST redirects are not followed".into()));
            }
            if !status.is_success() {
                if (matches!(status.as_u16(), 408 | 425 | 429) || status.is_server_error())
                    && attempt < 2
                {
                    self.wait_retry(retry_delay(response.headers().get("retry-after"), attempt))
                        .await?;
                    continue;
                }
                let retryable =
                    matches!(status.as_u16(), 408 | 425 | 429) || status.is_server_error();
                return Err(CoreError::HttpStatus { status: status.as_u16(), retryable });
            }
            if response.content_length().is_some_and(|length| length > self.max_bytes) {
                return Err(CoreError::ResourceLimit(format!(
                    "response exceeds {}",
                    self.max_bytes
                )));
            }

            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = tokio::select! {
                value = stream.next() => value,
                _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
            } {
                let chunk =
                    chunk.map_err(|_| CoreError::Transport("response stream failed".into()))?;
                if bytes.len() as u64 + chunk.len() as u64 > self.max_bytes {
                    return Err(CoreError::ResourceLimit(format!(
                        "response exceeds {}",
                        self.max_bytes
                    )));
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(bytes);
        }
        Err(CoreError::Transport("request failed after the retry budget was exhausted".into()))
    }

    pub async fn get_bytes_coalesced(
        &self,
        address: &str,
        headers: &[(&str, &str)],
        coalescer: &HttpRequestCoalescer,
    ) -> CoreResult<Arc<[u8]>> {
        let key = get_request_key(address, headers);
        let cell = {
            let mut calls = coalescer.calls.lock().await;
            calls.entry(key).or_insert_with(|| Arc::new(OnceCell::new())).clone()
        };
        cell.get_or_init(|| async {
            self.get_bytes_with_headers(address, headers).await.map(Arc::<[u8]>::from)
        })
        .await
        .clone()
    }

    /// Read an exact range from one immutable version of a large object.
    /// Redirects and servers ignoring Range are rejected; If-Match prevents
    /// assembling bytes from different revisions of a mutable provider file.
    pub(crate) async fn get_range_bytes(
        &self,
        address: &str,
        start: u64,
        end: u64,
        total: u64,
        etag: &str,
    ) -> CoreResult<Vec<u8>> {
        if start > end || end >= total || total > self.max_bytes {
            return Err(CoreError::ResourceLimit("invalid or oversized HTTP range".into()));
        }
        let url = Url::parse(address).map_err(|e| CoreError::Transport(e.to_string()))?;
        self.validate_url(&url)?;
        let host =
            url.host_str().ok_or_else(|| CoreError::Transport("request URL has no host".into()))?;
        let etag_header = HeaderValue::from_str(etag)
            .map_err(|_| CoreError::Transport("object ETag is invalid".into()))?;
        if !etag.starts_with('"') || !etag.ends_with('"') {
            return Err(CoreError::Transport("object ETag is not a strong validator".into()));
        }
        let length = end - start + 1;
        let expected_range = format!("bytes {start}-{end}/{total}");
        for attempt in 0..3 {
            let _host_permit = self.budget.acquire_host(host).await?;
            let _permit = self.budget.acquire_request().await?;
            let request = self
                .client_for(&url)
                .get(url.clone())
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .header(reqwest::header::IF_MATCH, etag_header.clone())
                .header(reqwest::header::ACCEPT_ENCODING, "identity");
            let response = tokio::select! {
                result = request.send() => result.map_err(|_| CoreError::Transport("range request failed".into()))?,
                _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
            };
            let status = response.status();
            if (matches!(status.as_u16(), 408 | 425 | 429) || status.is_server_error())
                && attempt < 2
            {
                let delay = retry_delay(response.headers().get("retry-after"), attempt);
                drop(response);
                drop(_permit);
                drop(_host_permit);
                self.wait_retry(delay).await?;
                continue;
            }
            if status != reqwest::StatusCode::PARTIAL_CONTENT
                || response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|value| value.to_str().ok())
                    != Some(expected_range.as_str())
                || response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|value| value.to_str().ok())
                    != Some(etag)
                || response.content_length().is_some_and(|value| value != length)
                || response
                    .headers()
                    .get(reqwest::header::CONTENT_ENCODING)
                    .is_some_and(|value| value != "identity")
            {
                return Err(CoreError::Transport(
                    "range response changed object revision or byte range".into(),
                ));
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = tokio::select! {
                value = stream.next() => value,
                _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
            } {
                let chunk = chunk
                    .map_err(|_| CoreError::Transport("range response stream failed".into()))?;
                if bytes.len() as u64 + chunk.len() as u64 > length {
                    return Err(CoreError::Transport(
                        "range response exceeds expected length".into(),
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            if bytes.len() as u64 != length {
                return Err(CoreError::Transport("range response is incomplete".into()));
            }
            return Ok(bytes);
        }
        Err(CoreError::Transport("range request retry budget exhausted".into()))
    }

    /// Stream a response into a same-directory temporary file, then publish it
    /// with create-only semantics. On any failure the partial file is removed.
    pub async fn get_to_path_with_headers(
        &self,
        address: &str,
        headers: &[(&str, &str)],
        destination: impl AsRef<Path>,
    ) -> CoreResult<HttpBodyReceipt> {
        self.get_to_path_inner(
            address,
            headers,
            destination,
            false,
            self.max_bytes,
            HttpGetPolicy::RetryTransient,
            None,
        )
        .await
    }

    /// Stream an artifact while restricting every redirect to the original
    /// origin. Provider download URLs are private locator data, so a provider
    /// redirect cannot silently broaden its source host allow-list.
    pub async fn get_to_path_same_origin_with_headers(
        &self,
        address: &str,
        headers: &[(&str, &str)],
        destination: impl AsRef<Path>,
    ) -> CoreResult<HttpBodyReceipt> {
        self.get_to_path_inner(
            address,
            headers,
            destination,
            true,
            self.max_bytes,
            HttpGetPolicy::RetryTransient,
            None,
        )
        .await
    }

    /// Stream an artifact under a per-response cap no larger than the
    /// transport's configured artifact limit.
    pub async fn get_to_path_same_origin_limited_with_headers(
        &self,
        address: &str,
        headers: &[(&str, &str)],
        destination: impl AsRef<Path>,
        max_bytes: u64,
    ) -> CoreResult<HttpBodyReceipt> {
        self.get_to_path_inner(
            address,
            headers,
            destination,
            true,
            max_bytes.min(self.max_bytes),
            HttpGetPolicy::RetryTransient,
            None,
        )
        .await
    }

    /// Stream an artifact with an explicit retry policy, while restricting
    /// redirects to the original origin and charging every request to the
    /// shared host and request budget.
    pub async fn get_to_path_same_origin_limited_with_policy(
        &self,
        address: &str,
        headers: &[(&str, &str)],
        destination: impl AsRef<Path>,
        max_bytes: u64,
        policy: HttpGetPolicy,
    ) -> CoreResult<HttpBodyReceipt> {
        self.get_to_path_inner(
            address,
            headers,
            destination,
            true,
            max_bytes.min(self.max_bytes),
            policy,
            None,
        )
        .await
    }

    /// Stream a single-attempt artifact while keeping every redirect on the
    /// original origin and the reviewed endpoint path.
    pub async fn get_to_path_same_origin_path_limited_with_policy(
        &self,
        address: &str,
        headers: &[(&str, &str)],
        destination: impl AsRef<Path>,
        max_bytes: u64,
        policy: HttpGetPolicy,
        allowed_path: &str,
    ) -> CoreResult<HttpBodyReceipt> {
        self.get_to_path_inner(
            address,
            headers,
            destination,
            true,
            max_bytes.min(self.max_bytes),
            policy,
            Some(allowed_path),
        )
        .await
    }

    async fn get_to_path_inner(
        &self,
        address: &str,
        headers: &[(&str, &str)],
        destination: impl AsRef<Path>,
        same_origin_only: bool,
        max_bytes: u64,
        policy: HttpGetPolicy,
        allowed_redirect_path: Option<&str>,
    ) -> CoreResult<HttpBodyReceipt> {
        if max_bytes == 0 {
            return Err(CoreError::ResourceLimit("response exceeds 0".into()));
        }
        let destination = destination.as_ref();
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| CoreError::Temporary("download directory could not be prepared".into()))?;

        let url = Url::parse(address).map_err(|e| CoreError::Transport(e.to_string()))?;
        self.validate_url(&url)?;
        if allowed_redirect_path.is_some_and(|path| url.path() != path) {
            return Err(CoreError::Transport("artifact URL is outside the reviewed path".into()));
        }
        let request_headers = headers
            .iter()
            .map(|(name, value)| {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| CoreError::Transport("request header is invalid".into()))?;
                let value = HeaderValue::from_bytes(value.as_bytes())
                    .map_err(|_| CoreError::Transport("request header is invalid".into()))?;
                Ok((name, value))
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let original_origin = url.origin().ascii_serialization();

        let max_attempts = match policy {
            HttpGetPolicy::RetryTransient => 3,
            HttpGetPolicy::SingleAttempt => 1,
        };
        for attempt in 0..max_attempts {
            let mut current = url.clone();
            let mut redirects = 0_u8;
            let response = loop {
                self.validate_url(&current)?;
                let host = current
                    .host_str()
                    .ok_or_else(|| CoreError::Transport("request URL has no host".into()))?;
                let host_permit = self.budget.acquire_host(host).await?;
                let request_permit = self.budget.acquire_request().await?;
                let mut request = self.client_for(&current).get(current.clone());
                if current.origin().ascii_serialization() == original_origin {
                    for (name, value) in &request_headers {
                        request = request.header(name, value);
                    }
                }
                let response = tokio::select! {
                    result = request.send() => result.map_err(|_| CoreError::Transport("request failed".into()))?,
                    _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
                };
                if response.status().is_redirection() {
                    if redirects >= 10 {
                        return Err(CoreError::Transport("too many HTTP redirects".into()));
                    }
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            CoreError::Transport("redirect missing Location header".into())
                        })?;
                    current =
                        current.join(location).map_err(|e| CoreError::Transport(e.to_string()))?;
                    self.validate_url(&current)?;
                    if same_origin_only && current.origin().ascii_serialization() != original_origin
                    {
                        return Err(CoreError::Transport(
                            "cross-origin artifact redirects are not allowed".into(),
                        ));
                    }
                    if allowed_redirect_path.is_some_and(|path| current.path() != path) {
                        return Err(CoreError::Transport(
                            "artifact redirect is outside the reviewed path".into(),
                        ));
                    }
                    redirects += 1;
                    continue;
                }
                break (response, host_permit, request_permit);
            };

            let (response, _host_permit, _request_permit) = response;
            let status = response.status();
            if !status.is_success() {
                let retryable =
                    matches!(status.as_u16(), 408 | 425 | 429) || status.is_server_error();
                if retryable && attempt + 1 < max_attempts {
                    let delay = retry_delay(response.headers().get("retry-after"), attempt);
                    drop(response);
                    drop(_request_permit);
                    drop(_host_permit);
                    self.wait_retry(delay).await?;
                    continue;
                }
                if policy == HttpGetPolicy::SingleAttempt {
                    return Err(CoreError::HttpStatus { status: status.as_u16(), retryable });
                }
                return Err(CoreError::Transport(format!("HTTP status {}", status.as_u16())));
            }
            if response.content_length().is_some_and(|length| length > max_bytes) {
                return Err(CoreError::ResourceLimit(format!("response exceeds {}", max_bytes)));
            }

            let staging = NamedTempFile::new_in(parent).map_err(|_| {
                CoreError::Temporary("download staging file could not be created".into())
            })?;
            let staging_file = staging.reopen().map_err(|_| {
                CoreError::Temporary("download staging file could not be opened".into())
            })?;
            let mut file = tokio::fs::File::from_std(staging_file);
            let mut stream = response.bytes_stream();
            let mut size_bytes = 0_u64;
            let mut hasher = Sha256::new();
            while let Some(chunk) = tokio::select! {
                value = stream.next() => value,
                _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
            } {
                let chunk =
                    chunk.map_err(|_| CoreError::Transport("response stream failed".into()))?;
                size_bytes = size_bytes.saturating_add(chunk.len() as u64);
                if size_bytes > max_bytes {
                    return Err(CoreError::ResourceLimit(format!(
                        "response exceeds {}",
                        max_bytes
                    )));
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|_| CoreError::Temporary("download staging write failed".into()))?;
                hasher.update(&chunk);
            }
            file.flush()
                .await
                .map_err(|_| CoreError::Temporary("download staging flush failed".into()))?;
            file.sync_all()
                .await
                .map_err(|_| CoreError::Temporary("download staging sync failed".into()))?;
            drop(file);
            staging.persist_noclobber(destination).map_err(|_| {
                CoreError::Temporary(
                    "download destination already exists or cannot be published".into(),
                )
            })?;
            return Ok(HttpBodyReceipt { size_bytes, sha256: hex::encode(hasher.finalize()) });
        }
        Err(CoreError::Transport("request failed after the retry budget was exhausted".into()))
    }

    pub async fn head_metadata(&self, address: &str) -> CoreResult<HttpMetadata> {
        let url = Url::parse(address).map_err(|e| CoreError::Transport(e.to_string()))?;
        self.validate_url(&url)?;
        for attempt in 0..3 {
            let mut current = url.clone();
            let mut redirects = 0_u8;
            let response = loop {
                self.validate_url(&current)?;
                let host = current
                    .host_str()
                    .ok_or_else(|| CoreError::Transport("request URL has no host".into()))?;
                let _host_permit = self.budget.acquire_host(host).await?;
                let _permit = self.budget.acquire_request().await?;
                let response = tokio::select! {
                    result = self.client_for(&current).head(current.clone()).send() => result.map_err(|_| CoreError::Transport("request failed".into()))?,
                    _ = self.budget.cancellation.cancelled() => return Err(CoreError::Cancelled),
                };
                if response.status().is_redirection() {
                    if redirects >= 10 {
                        return Err(CoreError::Transport("too many HTTP redirects".into()));
                    }
                    let location = response
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .ok_or_else(|| {
                            CoreError::Transport("redirect missing Location header".into())
                        })?;
                    current =
                        current.join(location).map_err(|e| CoreError::Transport(e.to_string()))?;
                    self.validate_url(&current)?;
                    redirects += 1;
                    continue;
                }
                break response;
            };
            let status = response.status();
            if (matches!(status.as_u16(), 408 | 425 | 429) || status.is_server_error())
                && attempt < 2
            {
                self.wait_retry(retry_delay(response.headers().get("retry-after"), attempt))
                    .await?;
                continue;
            }
            let mut headers = BTreeMap::new();
            for name in [
                reqwest::header::LAST_MODIFIED,
                reqwest::header::CONTENT_TYPE,
                reqwest::header::CONTENT_LENGTH,
                reqwest::header::ETAG,
            ] {
                if let Some(value) =
                    response.headers().get(&name).and_then(|value| value.to_str().ok())
                {
                    headers.insert(name.as_str().to_owned(), value.to_owned());
                }
            }
            return Ok(HttpMetadata { status: status.as_u16(), headers });
        }
        Err(CoreError::Transport("request failed after the retry budget was exhausted".into()))
    }

    pub fn cancel(&self) {
        self.budget.cancel();
    }
}

fn get_request_key(address: &str, headers: &[(&str, &str)]) -> String {
    let mut sorted_headers =
        headers.iter().map(|(name, value)| (name.to_ascii_lowercase(), *value)).collect::<Vec<_>>();
    sorted_headers.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(b"GET\0");
    hasher.update((address.len() as u64).to_be_bytes());
    hasher.update(address.as_bytes());
    for (name, value) in sorted_headers {
        hasher.update((name.len() as u64).to_be_bytes());
        hasher.update(name.as_bytes());
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn retry_delay(header: Option<&reqwest::header::HeaderValue>, attempt: u32) -> Duration {
    let exponential = Duration::from_millis(250_u64.saturating_mul(2_u64.saturating_pow(attempt)));
    let requested = header
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    requested.unwrap_or(exponential).min(Duration::from_secs(60))
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn response(raw: &str) -> CoreResult<Vec<u8>> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/grid.json", listener.local_addr().unwrap());
        let raw = raw.to_owned();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(request.contains("range: bytes=2-5\r\n"));
            assert!(request.contains("if-match: \"version1\"\r\n"));
            assert!(request.contains("accept-encoding: identity\r\n"));
            socket.write_all(raw.as_bytes()).await.unwrap();
        });
        let transport = HttpTransport::new(Limits::default(), false).unwrap();
        let result = transport.get_range_bytes(&url, 2, 5, 8, "\"version1\"").await;
        server.await.unwrap();
        result
    }

    #[tokio::test]
    async fn range_download_requires_exact_bytes_and_one_object_revision() {
        let valid = "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 2-5/8\r\nETag: \"version1\"\r\nContent-Length: 4\r\nConnection: close\r\n\r\ncdef";
        assert_eq!(response(valid).await.unwrap(), b"cdef");
        for invalid in [
            valid.replace("206 Partial Content", "200 OK"),
            valid.replace("bytes 2-5/8", "bytes 3-6/8"),
            valid.replace("bytes 2-5/8", "bytes 2-5/9"),
            valid.replace("ETag: \"version1\"", "ETag: \"version2\""),
            valid.replace("Content-Length: 4", "Content-Length: 5"),
            valid.replace("cdef", "cde"),
            "HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
            "HTTP/1.1 302 Found\r\nLocation: https://example.invalid/grid\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        ] {
            assert!(response(&invalid).await.is_err());
        }
    }

    #[tokio::test]
    async fn range_download_respects_network_and_object_size_limits() {
        let transport =
            HttpTransport::new(Limits { max_artifact_bytes: 8, ..Limits::default() }, false)
                .unwrap();
        assert!(matches!(
            transport.get_range_bytes("https://example.invalid/grid", 0, 3, 8, "\"v1\"").await,
            Err(CoreError::NetworkDisabled(_))
        ));
        assert!(matches!(
            transport.get_range_bytes("http://127.0.0.1/grid", 0, 3, 9, "\"v1\"").await,
            Err(CoreError::ResourceLimit(_))
        ));
        assert!(
            transport.get_range_bytes("http://127.0.0.1/grid", 0, 3, 8, "W/\"v1\"").await.is_err()
        );
    }
}

#[cfg(test)]
mod tls_tests {
    use super::*;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_rustls::{TlsAcceptor, rustls};

    async fn tls_endpoint() -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        tls_endpoint_responses(vec![(None, String::new(), "[]".into())]).await
    }

    async fn tls_endpoint_responses(
        responses: Vec<(Option<String>, String, String)>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let cert = CertificateDer::from_pem_slice(include_bytes!(
            "../../../../tests/fixtures/protocols/ftp/server-cert.pem"
        ))
        .unwrap();
        let key = PrivateKeyDer::from_pem_slice(include_bytes!(
            "../../../../tests/fixtures/protocols/ftp/server-key.pem"
        ))
        .unwrap();
        let provider = rustls::crypto::ring::default_provider();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for (expected_cookie, extra_headers, body) in responses {
                let (socket, _) = listener.accept().await.unwrap();
                if let Ok(mut socket) = acceptor.accept(socket).await {
                    let mut data = Vec::new();
                    let mut buffer = [0; 1024];
                    loop {
                        let size = socket.read(&mut buffer).await.unwrap();
                        if size == 0 {
                            return;
                        }
                        data.extend_from_slice(&buffer[..size]);
                        if data.windows(4).any(|part| part == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let request = String::from_utf8(data).unwrap().to_ascii_lowercase();
                    match expected_cookie {
                        Some(cookie) => assert!(request.contains(&format!("cookie: {cookie}\r\n"))),
                        None => assert!(!request.contains("\r\ncookie:")),
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                        body.len(),
                        extra_headers,
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                }
            }
        });
        (address, server)
    }

    #[tokio::test]
    async fn rdcap_session_keeps_rotated_cookies_for_index_and_raw_but_isolates_other_clients() {
        let (address, server) = tls_endpoint_responses(vec![
            (
                None,
                "Set-Cookie: rdcap_session=one; Path=/; Secure; HttpOnly\r\n".into(),
                "[]".into(),
            ),
            (
                Some("rdcap_session=one".into()),
                "Set-Cookie: rdcap_session=two; Path=/; Secure; HttpOnly\r\n".into(),
                "[]".into(),
            ),
            (Some("rdcap_session=two".into()), String::new(), "\"grid\"".into()),
            (None, String::new(), "[]".into()),
            (None, String::new(), "[]".into()),
        ])
        .await;
        let limits = Limits::default();
        let client = |cookies| {
            Client::builder()
                .no_proxy()
                .resolve("rdcap.cwa.gov.tw", address)
                .danger_accept_invalid_certs(true)
                .cookie_store(cookies)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap()
        };
        let mut transport = HttpTransport::new(limits.clone(), true)
            .unwrap()
            .with_rdcap_session(&limits, true)
            .unwrap();
        transport.rdcap_client = Some(client(true));
        let base = format!("https://rdcap.cwa.gov.tw:{}", address.port());
        transport
            .post_form_bytes_with_headers(&format!("{base}/data_access/get_country_list"), &[], &[])
            .await
            .unwrap();
        let cloned = transport.clone();
        cloned
            .post_form_bytes_with_headers(&format!("{base}/data_access/get_radar_data"), &[], &[])
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("frame.json");
        cloned
            .get_to_path_same_origin_path_limited_with_policy(
                &format!("{base}/file?ft=fixture"),
                &[],
                &destination,
                1024,
                HttpGetPolicy::SingleAttempt,
                "/file",
            )
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), b"\"grid\"");
        let mut independent = HttpTransport::new(limits.clone(), true)
            .unwrap()
            .with_rdcap_session(&limits, true)
            .unwrap();
        independent.rdcap_client = Some(client(true));
        independent.get_bytes(&format!("{base}/file?ft=independent")).await.unwrap();
        let mut generic = HttpTransport::new(limits, true).unwrap();
        generic.client = client(false);
        generic.get_bytes(&format!("{base}/file?ft=generic")).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rdcap_insecure_tls_accepts_only_the_explicit_host_and_preserves_network_opt_in() {
        for (enabled, host, post, succeeds) in [
            (false, "rdcap.cwa.gov.tw", false, false),
            (true, "rdcap.cwa.gov.tw", false, true),
            (true, "rdcap.cwa.gov.tw", true, true),
            (true, "localhost", false, false),
        ] {
            let (address, server) = tls_endpoint().await;
            let limits = Limits::default();
            let mut transport = HttpTransport::new(limits.clone(), true)
                .unwrap()
                .with_rdcap_session(&limits, enabled)
                .unwrap();
            let client = |insecure| {
                Client::builder()
                    .no_proxy()
                    .resolve(host, address)
                    .danger_accept_invalid_certs(insecure)
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap()
            };
            transport.client = client(false);
            transport.rdcap_client = Some(client(enabled));
            let url = format!("https://{host}:{}/data_access/get_country_list", address.port());
            let result = if post {
                transport.post_form_bytes_with_headers(&url, &[], &[]).await
            } else {
                transport.get_bytes(&url).await
            };
            assert_eq!(result.is_ok(), succeeds, "enabled={enabled} host={host} post={post}");
            if succeeds {
                assert_eq!(result.unwrap(), b"[]");
            }
            server.await.unwrap();
        }
        let limits = Limits::default();
        let transport = HttpTransport::new(limits.clone(), false)
            .unwrap()
            .with_rdcap_session(&limits, true)
            .unwrap();
        assert!(matches!(
            transport.get_bytes("https://rdcap.cwa.gov.tw/").await,
            Err(CoreError::NetworkDisabled(_))
        ));
        for url in [
            "https://rdcap.cwa.gov.tw.example.invalid/",
            "https://example.invalid/",
            "http://rdcap.cwa.gov.tw/",
        ] {
            assert!(std::ptr::eq(
                transport.client_for(&Url::parse(url).unwrap()),
                &transport.client
            ));
        }
    }
}
