use crate::errors::{CoreError, CoreResult};
use crate::limits::{Limits, RequestBudget};
use futures_util::StreamExt;
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

#[derive(Clone)]
pub struct HttpTransport {
    client: Client,
    budget: Arc<RequestBudget>,
    allow_public: bool,
    max_bytes: u64,
}

impl HttpTransport {
    pub fn new(limits: Limits, allow_public: bool) -> CoreResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(limits.request_timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| CoreError::Transport(e.to_string()))?;
        Ok(Self {
            client,
            budget: Arc::new(RequestBudget::new(&limits)),
            allow_public,
            max_bytes: limits.max_artifact_bytes,
        })
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

    pub async fn get_bytes(&self, address: &str) -> CoreResult<Vec<u8>> {
        let url = Url::parse(address).map_err(|e| CoreError::Transport(e.to_string()))?;
        self.validate_url(&url)?;
        for attempt in 0..3 {
            let mut current = url.clone();
            let mut redirects = 0_u8;
            let response = loop {
                self.validate_url(&current)?;
                let _permit = self.budget.acquire_request().await?;
                let response = tokio::select! {
                    result = self.client.get(current.clone()).send() => result.map_err(|_| CoreError::Transport("request failed".into()))?,
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
                        tokio::time::sleep(retry_delay(
                            response.headers().get("retry-after"),
                            attempt,
                        ))
                        .await;
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

    pub fn cancel(&self) {
        self.budget.cancel();
    }
}

fn retry_delay(header: Option<&reqwest::header::HeaderValue>, attempt: u32) -> Duration {
    let exponential = Duration::from_millis(250_u64.saturating_mul(2_u64.saturating_pow(attempt)));
    let requested = header
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs);
    requested.unwrap_or(exponential).min(Duration::from_secs(60))
}
