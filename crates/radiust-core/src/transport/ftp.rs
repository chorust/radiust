use crate::errors::{CoreError, CoreResult};
use crate::limits::{Limits, RequestBudget};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use suppaftp::tokio::{
    AsyncFtpStream, AsyncRustlsConnector, AsyncRustlsFtpStream, ImplAsyncFtpStream, TokioTlsStream,
};
use suppaftp::{FtpError, FtpResult};
use tokio::io::AsyncReadExt;
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FtpObject {
    pub bytes: Vec<u8>,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone)]
pub struct FtpTransport {
    limits: Limits,
    budget: Arc<RequestBudget>,
    allow_public: bool,
    extra_roots: Vec<CertificateDer<'static>>,
}

impl FtpTransport {
    pub fn new(limits: Limits, allow_public: bool) -> Self {
        Self {
            budget: Arc::new(RequestBudget::new(&limits)),
            limits,
            allow_public,
            extra_roots: Vec::new(),
        }
    }

    pub fn with_root_certificate(limits: Limits, allow_public: bool, certificate: Vec<u8>) -> Self {
        Self {
            budget: Arc::new(RequestBudget::new(&limits)),
            limits,
            allow_public,
            extra_roots: vec![CertificateDer::from(certificate)],
        }
    }

    fn allowed(&self, url: &Url) -> bool {
        if self.allow_public || std::env::var("RADIUST_TEST_ALLOW_LIVE").as_deref() == Ok("1") {
            return true;
        }
        matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
    }

    fn parse(&self, address: &str) -> CoreResult<(Url, String, u16, String)> {
        let url = Url::parse(address).map_err(|e| CoreError::Transport(e.to_string()))?;
        if !matches!(url.scheme(), "ftp" | "ftps") {
            return Err(CoreError::Transport(
                "FTP transport supports only ftp:// and ftps://".into(),
            ));
        }
        if !self.allowed(&url) {
            return Err(CoreError::NetworkDisabled(url.to_string()));
        }
        let host = url
            .host_str()
            .ok_or_else(|| CoreError::Transport("FTP URL has no host".into()))?
            .to_string();
        let port = url.port().unwrap_or(21);
        let path = url.path().trim_start_matches('/').to_string();
        Ok((url, host, port, path))
    }

    async fn guarded<T, F>(&self, future: F) -> CoreResult<T>
    where
        F: Future<Output = FtpResult<T>>,
    {
        let timeout = Duration::from_secs(self.limits.request_timeout_secs);
        tokio::select! {
            _ = self.budget.cancellation.cancelled() => Err(CoreError::Cancelled),
            result = tokio::time::timeout(timeout, future) => {
                match result {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(error)) => Err(map_ftp_error(error)),
                    Err(_) => Err(CoreError::Transport("FTP request timed out".into())),
                }
            }
        }
    }

    async fn frame_guarded<T, F>(&self, future: F) -> CoreResult<T>
    where
        F: Future<Output = CoreResult<T>>,
    {
        let _permit = self.budget.acquire_frame().await?;
        tokio::time::timeout(Duration::from_secs(self.limits.frame_deadline_secs), future)
            .await
            .map_err(|_| CoreError::Transport("FTP frame deadline exceeded".into()))?
    }

    async fn retry_delay(&self, attempt: u32) -> CoreResult<()> {
        let delay = Duration::from_millis(250_u64.saturating_mul(2_u64.saturating_pow(attempt)));
        tokio::select! {
            _ = tokio::time::sleep(delay) => Ok(()),
            _ = self.budget.cancellation.cancelled() => Err(CoreError::Cancelled),
        }
    }

    async fn connect_plain(
        &self,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
    ) -> CoreResult<AsyncFtpStream> {
        let _permit = self.budget.acquire_request().await?;
        let mut stream = self.guarded(AsyncFtpStream::connect((host, port))).await?;
        self.guarded(stream.login(username, password)).await?;
        Ok(stream)
    }

    async fn connect_tls(
        &self,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
    ) -> CoreResult<AsyncRustlsFtpStream> {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let _permit = self.budget.acquire_request().await?;
        let mut roots = RootCertStore::empty();
        if self.extra_roots.is_empty() {
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        } else {
            for certificate in &self.extra_roots {
                roots.add(certificate.clone()).map_err(|e| {
                    CoreError::Transport(format!("invalid FTPS root certificate: {e}"))
                })?;
            }
        }
        let config = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let connector = AsyncRustlsConnector::from(connector);
        let stream = self.guarded(AsyncRustlsFtpStream::connect((host, port))).await?;
        let mut stream = self.guarded(stream.into_secure(connector, host)).await?;
        self.guarded(stream.login(username, password)).await?;
        Ok(stream)
    }

    async fn list_from<T>(
        &self,
        stream: &mut ImplAsyncFtpStream<T>,
        path: Option<&str>,
    ) -> CoreResult<Vec<String>>
    where
        T: TokioTlsStream + Send,
    {
        self.guarded(stream.list(path)).await
    }

    async fn names_from<T>(
        &self,
        stream: &mut ImplAsyncFtpStream<T>,
        path: Option<&str>,
    ) -> CoreResult<Vec<String>>
    where
        T: TokioTlsStream + Send,
    {
        self.guarded(stream.nlst(path)).await
    }

    async fn size_from<T>(&self, stream: &mut ImplAsyncFtpStream<T>, path: &str) -> CoreResult<u64>
    where
        T: TokioTlsStream + Send,
    {
        self.guarded(stream.size(path)).await.map(|size| size as u64)
    }

    async fn read_from<T>(
        &self,
        stream: &mut ImplAsyncFtpStream<T>,
        path: &str,
    ) -> CoreResult<FtpObject>
    where
        T: TokioTlsStream + Send + 'static,
    {
        let request_timeout = Duration::from_secs(self.limits.request_timeout_secs);
        let max_bytes = self.limits.max_artifact_bytes;
        let cancellation = self.budget.cancellation.clone();
        let result = tokio::time::timeout(
            request_timeout,
            stream.retr(path, move |mut data_stream| {
                let cancellation = cancellation.clone();
                Box::pin(async move {
                    let mut bytes = Vec::new();
                    let mut chunk = [0_u8; 64 * 1024];
                    loop {
                        let read = tokio::select! {
                            _ = cancellation.cancelled() => {
                                return Err(FtpError::ConnectionError(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "FTP transfer cancelled",
                                )));
                            }
                            result = tokio::time::timeout(request_timeout, data_stream.read(&mut chunk)) => {
                                result
                                    .map_err(|_| FtpError::ConnectionError(std::io::Error::new(
                                        std::io::ErrorKind::TimedOut,
                                        "FTP data read timed out",
                                    )))
                                    .and_then(|result| result.map_err(FtpError::ConnectionError))?
                            }
                        };
                        if read == 0 {
                            break;
                        }
                        let next = bytes.len() as u64 + read as u64;
                        if next > max_bytes {
                            return Err(FtpError::ConnectionError(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "FTP response exceeds configured limit",
                            )));
                        }
                        bytes.extend_from_slice(&chunk[..read]);
                    }
                    let digest = Sha256::digest(&bytes);
                    Ok((
                        FtpObject {
                            size_bytes: bytes.len() as u64,
                            sha256: hex::encode(digest),
                            bytes,
                        },
                        data_stream,
                    ))
                })
            }),
        )
        .await;

        match result {
            Ok(Ok(object)) => Ok(object),
            Ok(Err(_error)) if self.budget.cancellation.is_cancelled() => {
                let _ =
                    tokio::time::timeout(request_timeout, stream.abort(tokio::io::empty())).await;
                Err(CoreError::Cancelled)
            }
            Ok(Err(error)) => {
                let _ =
                    tokio::time::timeout(request_timeout, stream.abort(tokio::io::empty())).await;
                Err(map_ftp_error(error))
            }
            Err(_) => {
                let _ =
                    tokio::time::timeout(request_timeout, stream.abort(tokio::io::empty())).await;
                Err(CoreError::Transport("FTP data read timed out".into()))
            }
        }
    }

    async fn list_once(
        &self,
        tls: bool,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
        directory: Option<&str>,
    ) -> CoreResult<Vec<String>> {
        if tls {
            let mut stream = self.connect_tls(host, port, username, password).await?;
            let result = self.list_from(&mut stream, directory).await;
            let _ = self.guarded(stream.quit()).await;
            result
        } else {
            let mut stream = self.connect_plain(host, port, username, password).await?;
            let result = self.list_from(&mut stream, directory).await;
            let _ = self.guarded(stream.quit()).await;
            result
        }
    }

    async fn names_once(
        &self,
        tls: bool,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
        directory: Option<&str>,
    ) -> CoreResult<Vec<String>> {
        if tls {
            let mut stream = self.connect_tls(host, port, username, password).await?;
            let result = self.names_from(&mut stream, directory).await;
            let _ = self.guarded(stream.quit()).await;
            result
        } else {
            let mut stream = self.connect_plain(host, port, username, password).await?;
            let result = self.names_from(&mut stream, directory).await;
            let _ = self.guarded(stream.quit()).await;
            result
        }
    }

    async fn size_once(
        &self,
        tls: bool,
        host: &str,
        port: u16,
        path: &str,
        username: &str,
        password: &str,
    ) -> CoreResult<u64> {
        if tls {
            let mut stream = self.connect_tls(host, port, username, password).await?;
            let result = self.size_from(&mut stream, path).await;
            let _ = self.guarded(stream.quit()).await;
            result
        } else {
            let mut stream = self.connect_plain(host, port, username, password).await?;
            let result = self.size_from(&mut stream, path).await;
            let _ = self.guarded(stream.quit()).await;
            result
        }
    }

    async fn get_once(
        &self,
        tls: bool,
        host: &str,
        port: u16,
        path: &str,
        username: &str,
        password: &str,
    ) -> CoreResult<FtpObject> {
        if tls {
            let mut stream = self.connect_tls(host, port, username, password).await?;
            let result = self.read_from(&mut stream, path).await;
            let _ = self.guarded(stream.quit()).await;
            result
        } else {
            let mut stream = self.connect_plain(host, port, username, password).await?;
            let result = self.read_from(&mut stream, path).await;
            let _ = self.guarded(stream.quit()).await;
            result
        }
    }

    pub async fn list(
        &self,
        address: &str,
        username: &str,
        password: &str,
    ) -> CoreResult<Vec<String>> {
        self.frame_guarded(async {
            let (url, host, port, path) = self.parse(address)?;
            let directory = (!path.is_empty()).then_some(path.as_str());
            for attempt in 0..3 {
                let result = self
                    .list_once(url.scheme() == "ftps", &host, port, username, password, directory)
                    .await;
                match result {
                    Ok(value) => return Ok(value),
                    Err(error) if attempt < 2 && retryable_ftp_core_error(&error) => {
                        self.retry_delay(attempt).await?;
                    }
                    Err(error) => return Err(error),
                }
            }
            unreachable!("FTP retry loop always returns")
        })
        .await
    }

    pub async fn names(
        &self,
        address: &str,
        username: &str,
        password: &str,
    ) -> CoreResult<Vec<String>> {
        self.frame_guarded(async {
            let (url, host, port, path) = self.parse(address)?;
            let directory = (!path.is_empty()).then_some(path.as_str());
            for attempt in 0..3 {
                let result = self
                    .names_once(url.scheme() == "ftps", &host, port, username, password, directory)
                    .await;
                match result {
                    Ok(value) => return Ok(value),
                    Err(error) if attempt < 2 && retryable_ftp_core_error(&error) => {
                        self.retry_delay(attempt).await?;
                    }
                    Err(error) => return Err(error),
                }
            }
            unreachable!("FTP retry loop always returns")
        })
        .await
    }

    pub async fn size(&self, address: &str, username: &str, password: &str) -> CoreResult<u64> {
        self.frame_guarded(async {
            let (url, host, port, path) = self.parse(address)?;
            if path.is_empty() {
                return Err(CoreError::Transport("FTP size requires a file path".into()));
            }
            for attempt in 0..3 {
                let result = self
                    .size_once(url.scheme() == "ftps", &host, port, &path, username, password)
                    .await;
                match result {
                    Ok(value) => return Ok(value),
                    Err(error) if attempt < 2 && retryable_ftp_core_error(&error) => {
                        self.retry_delay(attempt).await?;
                    }
                    Err(error) => return Err(error),
                }
            }
            unreachable!("FTP retry loop always returns")
        })
        .await
    }

    pub async fn get_bytes(
        &self,
        address: &str,
        username: &str,
        password: &str,
    ) -> CoreResult<FtpObject> {
        self.frame_guarded(async {
            let (url, host, port, path) = self.parse(address)?;
            if path.is_empty() {
                return Err(CoreError::Transport("FTP read requires a file path".into()));
            }
            for attempt in 0..3 {
                let result = self
                    .get_once(url.scheme() == "ftps", &host, port, &path, username, password)
                    .await;
                match result {
                    Ok(value) => return Ok(value),
                    Err(error) if attempt < 2 && retryable_ftp_core_error(&error) => {
                        self.retry_delay(attempt).await?;
                    }
                    Err(error) => return Err(error),
                }
            }
            unreachable!("FTP retry loop always returns")
        })
        .await
    }

    pub fn cancel(&self) {
        self.budget.cancel();
    }
}

fn map_ftp_error(error: FtpError) -> CoreError {
    CoreError::Transport(format!("FTP error: {error}"))
}

fn retryable_ftp_core_error(error: &CoreError) -> bool {
    let CoreError::Transport(message) = error else {
        return false;
    };
    if message.contains("timed out")
        || message.contains("Connection error")
        || message.contains("connection reset")
        || message.contains("broken pipe")
        || message.contains("unexpected end of file")
    {
        return true;
    }
    let Some(status) = message
        .split('[')
        .nth(1)
        .and_then(|tail| tail.get(..3))
        .and_then(|value| value.parse::<u16>().ok())
    else {
        return false;
    };
    (400..500).contains(&status) && !matches!(status, 430 | 530)
}
