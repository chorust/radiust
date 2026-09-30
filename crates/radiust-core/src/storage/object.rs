//! OpenDAL-backed object storage primitives.
//!
//! The Python layer owns output identity and manifest state.  This module only
//! handles safe object keys, bounded streaming I/O, and content receipts.

use crate::errors::{CoreError, CoreResult};
use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStreamExt};
use opendal::{ErrorKind, Operator, services};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const MAX_OBJECT_READ_ATTEMPTS: usize = 3;

#[derive(Clone, Debug, Default)]
pub struct ObjectStoreConfig {
    pub provider: String,
    pub bucket: String,
    pub prefix: String,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub anonymous: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectReceipt {
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone)]
pub struct ObjectStore {
    operator: Operator,
    prefix: String,
}

impl ObjectStore {
    pub fn from_config(config: ObjectStoreConfig) -> CoreResult<Self> {
        opendal::install_default();
        if config.bucket.trim().is_empty() {
            return Err(CoreError::Storage("object storage bucket is required".into()));
        }
        let prefix = clean_prefix(&config.prefix)?;
        let provider = config.provider.to_ascii_lowercase();
        let operator = match provider.as_str() {
            "s3" | "aws" | "s3-compatible" | "minio" => {
                let mut builder = services::S3::default().bucket(&config.bucket);
                if let Some(endpoint) = config.endpoint.as_deref() {
                    builder = builder.endpoint(endpoint);
                }
                if let Some(region) = config.region.as_deref() {
                    builder = builder.region(region);
                }
                if let Some(access_key_id) = config.access_key_id.as_deref() {
                    builder = builder.access_key_id(access_key_id);
                }
                if let Some(secret_access_key) = config.secret_access_key.as_deref() {
                    builder = builder.secret_access_key(secret_access_key);
                }
                if config.anonymous {
                    builder = builder.skip_signature();
                }
                Operator::new(builder)
            }
            "oss" | "aliyun-oss" => {
                let mut builder = services::Oss::default().bucket(&config.bucket);
                if let Some(endpoint) = config.endpoint.as_deref() {
                    builder = builder.endpoint(endpoint);
                }
                if let Some(access_key_id) = config.access_key_id.as_deref() {
                    builder = builder.access_key_id(access_key_id);
                }
                if let Some(secret_access_key) = config.secret_access_key.as_deref() {
                    builder = builder.access_key_secret(secret_access_key);
                }
                if config.anonymous {
                    builder = builder.skip_signature();
                }
                Operator::new(builder)
            }
            _ => {
                return Err(CoreError::Storage(format!(
                    "unsupported object storage provider: {}",
                    config.provider
                )));
            }
        }
        .map_err(|error| CoreError::Storage(format!("object storage setup failed: {error}")))?;
        Ok(Self { operator, prefix })
    }

    pub fn from_operator(operator: Operator, prefix: impl Into<String>) -> CoreResult<Self> {
        Ok(Self { operator, prefix: clean_prefix(&prefix.into())? })
    }

    pub fn capabilities(&self) -> opendal::Capability {
        self.operator.info().capability()
    }

    pub async fn read(&self, key: &str, max_bytes: u64) -> CoreResult<(Vec<u8>, ObjectReceipt)> {
        let path = self.object_key(key)?;
        for attempt in 0..MAX_OBJECT_READ_ATTEMPTS {
            match read_object_attempt(&self.operator, &path, max_bytes).await {
                Err(ReadAttemptError::Cancelled) => return Err(CoreError::Cancelled),
                Ok(result) => return Ok(result),
                Err(ReadAttemptError::ResourceLimit) => {
                    return Err(CoreError::ResourceLimit(format!(
                        "object exceeds configured limit {max_bytes}"
                    )));
                }
                Err(ReadAttemptError::Local(error)) => return Err(error),
                Err(ReadAttemptError::Provider { retryable: true, .. })
                    if attempt + 1 < MAX_OBJECT_READ_ATTEMPTS =>
                {
                    tokio::time::sleep(read_retry_delay(attempt)).await;
                }
                Err(ReadAttemptError::Provider { operation, message, .. }) => {
                    return Err(object_read_error(operation, message));
                }
            }
        }
        unreachable!("the object read retry loop returns or errors on its final attempt")
    }

    /// Stream an object into a new local staging file without buffering the
    /// full object in memory. The destination is exclusive-create and any
    /// partial file is removed after a read, limit, or disk error.
    pub async fn read_to_path(
        &self,
        key: &str,
        destination: impl AsRef<Path>,
        max_bytes: u64,
    ) -> CoreResult<ObjectReceipt> {
        self.read_to_path_cancellable(key, destination, max_bytes, &CancellationToken::new()).await
    }

    /// Stream an object to a new local staging file with bounded retries for
    /// temporary provider read failures. Cancellation interrupts provider
    /// reads and retry backoff; failed attempts remove their partial file.
    pub async fn read_to_path_cancellable(
        &self,
        key: &str,
        destination: impl AsRef<Path>,
        max_bytes: u64,
        cancellation: &CancellationToken,
    ) -> CoreResult<ObjectReceipt> {
        let path = self.object_key(key)?;
        let destination = destination.as_ref();
        for attempt in 0..MAX_OBJECT_READ_ATTEMPTS {
            if cancellation.is_cancelled() {
                return Err(CoreError::Cancelled);
            }

            match read_to_path_attempt(&self.operator, &path, destination, max_bytes, cancellation)
                .await
            {
                Ok(receipt) => return Ok(receipt),
                Err(ReadAttemptError::Cancelled) => return Err(CoreError::Cancelled),
                Err(ReadAttemptError::ResourceLimit) => {
                    return Err(CoreError::ResourceLimit(format!(
                        "object exceeds configured limit {max_bytes}"
                    )));
                }
                Err(ReadAttemptError::Local(error)) => return Err(error),
                Err(ReadAttemptError::Provider { retryable: true, .. })
                    if attempt + 1 < MAX_OBJECT_READ_ATTEMPTS =>
                {
                    tokio::select! {
                        biased;
                        _ = cancellation.cancelled() => return Err(CoreError::Cancelled),
                        _ = tokio::time::sleep(read_retry_delay(attempt)) => {}
                    }
                }
                Err(ReadAttemptError::Provider { operation, message, .. }) => {
                    return Err(object_read_error(operation, message));
                }
            }
        }
        unreachable!("the object read retry loop returns or errors on its final attempt")
    }

    pub async fn read_optional(
        &self,
        key: &str,
        max_bytes: u64,
    ) -> CoreResult<Option<(Vec<u8>, ObjectReceipt)>> {
        let path = self.object_key(key)?;
        let exists = self.operator.exists(&path).await.map_err(|error| {
            CoreError::Transport(format!("object existence check failed: {error}"))
        })?;
        if !exists {
            return Ok(None);
        }
        self.read(key, max_bytes).await.map(Some)
    }

    pub async fn write_chunks(
        &self,
        key: &str,
        chunks: Vec<Vec<u8>>,
        max_bytes: u64,
    ) -> CoreResult<ObjectReceipt> {
        let stream = futures_util::stream::iter(
            chunks.into_iter().map(|chunk| Ok::<_, CoreError>(Bytes::from(chunk))),
        );
        self.write_stream(key, stream, max_bytes).await
    }

    /// Upload a local staged artifact incrementally without reading the whole
    /// file into memory. The receipt hashes the exact bytes sent to the object
    /// writer, and the stream enforces the limit even if the file grows after
    /// its initial metadata check.
    pub async fn write_path(
        &self,
        key: &str,
        source: impl AsRef<Path>,
        max_bytes: u64,
    ) -> CoreResult<ObjectReceipt> {
        self.write_path_cancellable(key, source, max_bytes, &CancellationToken::new()).await
    }

    /// Upload a local artifact incrementally and stop between bounded chunks
    /// when the operation is cancelled. Any unfinished provider writer is
    /// aborted by `write_stream` before the cancellation is returned.
    pub async fn write_path_cancellable(
        &self,
        key: &str,
        source: impl AsRef<Path>,
        max_bytes: u64,
        cancellation: &CancellationToken,
    ) -> CoreResult<ObjectReceipt> {
        const CHUNK_BYTES: usize = 64 * 1024;

        if cancellation.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let source = source.as_ref();
        let file = tokio::fs::File::open(source)
            .await
            .map_err(|_| CoreError::Temporary("object source file could not be opened".into()))?;
        let size = file
            .metadata()
            .await
            .map_err(|_| CoreError::Temporary("object source file metadata is unavailable".into()))?
            .len();
        if size > max_bytes {
            return Err(CoreError::ResourceLimit(format!(
                "object exceeds configured limit {max_bytes}"
            )));
        }

        let token = cancellation.clone();
        let chunks =
            futures_util::stream::try_unfold((file, token), |(mut file, token)| async move {
                if token.is_cancelled() {
                    return Err(CoreError::Cancelled);
                }
                let mut chunk = vec![0_u8; CHUNK_BYTES];
                let read = tokio::select! {
                    biased;
                    _ = token.cancelled() => return Err(CoreError::Cancelled),
                    read = file.read(&mut chunk) => read,
                };
                match read {
                    Ok(0) => Ok(None),
                    Ok(length) => {
                        chunk.truncate(length);
                        Ok(Some((Bytes::from(chunk), (file, token))))
                    }
                    Err(_) => Err(CoreError::Temporary("object source file read failed".into())),
                }
            });
        self.write_stream(key, chunks, max_bytes).await
    }

    /// Write an object from a bounded stream of chunks.
    ///
    /// The stream is consumed incrementally, and its chunks are never collected
    /// into a single object-sized buffer. On a stream, size-limit, write, or
    /// close error the OpenDAL writer is aborted before the error is returned.
    pub async fn write_stream<S>(
        &self,
        key: &str,
        stream: S,
        max_bytes: u64,
    ) -> CoreResult<ObjectReceipt>
    where
        S: Stream<Item = CoreResult<Bytes>> + Send,
    {
        let path = self.object_key(key)?;
        let mut writer =
            self.operator.writer(&path).await.map_err(|error| {
                CoreError::Storage(format!("object writer setup failed: {error}"))
            })?;
        write_stream_to_writer(&mut writer, stream, max_bytes, |operation, error| {
            CoreError::Storage(format!("object {operation} failed: {error}"))
        })
        .await
    }

    pub async fn write_chunks_once(
        &self,
        key: &str,
        chunks: Vec<Vec<u8>>,
        max_bytes: u64,
    ) -> CoreResult<ObjectReceipt> {
        let stream = futures_util::stream::iter(
            chunks.into_iter().map(|chunk| Ok::<_, CoreError>(Bytes::from(chunk))),
        );
        self.write_stream_once(key, stream, max_bytes).await
    }

    /// Create an object only if it does not already exist, consuming chunks
    /// incrementally and enforcing `max_bytes` before each chunk is written.
    pub async fn write_stream_once<S>(
        &self,
        key: &str,
        stream: S,
        max_bytes: u64,
    ) -> CoreResult<ObjectReceipt>
    where
        S: Stream<Item = CoreResult<Bytes>> + Send,
    {
        let path = self.object_key(key)?;
        // Conditional creation is enforced by the storage provider at publish
        // time. An exists() probe followed by a normal write is not atomic.
        let mut writer = self
            .operator
            .writer_with(&path)
            .if_not_exists(true)
            .await
            .map_err(|error| write_once_error(key, error))?;
        write_stream_to_writer(&mut writer, stream, max_bytes, |_, error| {
            write_once_error(key, error)
        })
        .await
    }

    pub async fn delete(&self, key: &str) -> CoreResult<()> {
        let path = self.object_key(key)?;
        self.operator
            .delete(&path)
            .await
            .map_err(|error| CoreError::Storage(format!("object delete failed: {error}")))
    }

    fn object_key(&self, key: &str) -> CoreResult<String> {
        let key = clean_key(key)?;
        if self.prefix.is_empty() { Ok(key) } else { Ok(format!("{}/{}", self.prefix, key)) }
    }
}

enum ReadAttemptError {
    Provider { operation: &'static str, message: String, retryable: bool },
    ResourceLimit,
    Cancelled,
    Local(CoreError),
}

fn read_retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(25_u64 * (1_u64 << attempt))
}

fn object_read_error(operation: &'static str, message: String) -> CoreError {
    let message = match operation {
        "read setup" => format!("object read setup failed: {message}"),
        "read stream setup" => format!("object read stream failed: {message}"),
        _ => format!("object read failed: {message}"),
    };
    CoreError::Transport(message)
}

fn stream_read_error_is_temporary(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<opendal::Error>())
        .is_some_and(opendal::Error::is_temporary)
}

async fn read_to_path_attempt(
    operator: &Operator,
    path: &str,
    destination: &Path,
    max_bytes: u64,
    cancellation: &CancellationToken,
) -> Result<ObjectReceipt, ReadAttemptError> {
    let reader = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(ReadAttemptError::Cancelled),
        result = operator.reader(path) => result.map_err(|error| ReadAttemptError::Provider {
            operation: "read setup",
            message: error.to_string(),
            retryable: error.is_temporary(),
        })?,
    };
    let mut stream = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(ReadAttemptError::Cancelled),
        result = reader.into_bytes_stream(..) => result.map_err(|error| ReadAttemptError::Provider {
            operation: "read stream setup",
            message: error.to_string(),
            retryable: error.is_temporary(),
        })?,
    };
    if cancellation.is_cancelled() {
        return Err(ReadAttemptError::Cancelled);
    }
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .await
        .map_err(|_| {
            ReadAttemptError::Local(CoreError::Temporary(
                "object staging file could not be created".into(),
            ))
        })?;
    let staged = match tempfile::TempPath::try_from_path(destination.to_path_buf()) {
        Ok(path) => path,
        Err(_) => {
            drop(output);
            let _ = tokio::fs::remove_file(destination).await;
            return Err(ReadAttemptError::Local(CoreError::Temporary(
                "object staging file could not be retained".into(),
            )));
        }
    };

    let result = async {
        let mut digest = Sha256::new();
        let mut size_bytes = 0_u64;
        loop {
            let item = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(ReadAttemptError::Cancelled),
                item = stream.next() => item,
            };
            let Some(item) = item else {
                break;
            };
            let chunk = item.map_err(|error| ReadAttemptError::Provider {
                operation: "read",
                message: error.to_string(),
                retryable: stream_read_error_is_temporary(&error),
            })?;
            let chunk_size = chunk.len() as u64;
            if chunk_size > max_bytes.saturating_sub(size_bytes) {
                return Err(ReadAttemptError::ResourceLimit);
            }
            let write = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(ReadAttemptError::Cancelled),
                result = output.write_all(&chunk) => result,
            };
            write.map_err(|_| {
                ReadAttemptError::Local(CoreError::Temporary("object staging write failed".into()))
            })?;
            digest.update(&chunk);
            size_bytes += chunk_size;
        }
        let sync = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ReadAttemptError::Cancelled),
            result = output.sync_all() => result,
        };
        sync.map_err(|_| {
            ReadAttemptError::Local(CoreError::Temporary("object staging flush failed".into()))
        })?;
        Ok(ObjectReceipt { size_bytes, sha256: hex::encode(digest.finalize()) })
    }
    .await;
    drop(output);

    match result {
        Ok(receipt) => {
            staged.keep().map_err(|_| {
                ReadAttemptError::Local(CoreError::Temporary(
                    "object staging file could not be retained".into(),
                ))
            })?;
            Ok(receipt)
        }
        Err(error) => Err(error),
    }
}

async fn read_object_attempt(
    operator: &Operator,
    path: &str,
    max_bytes: u64,
) -> Result<(Vec<u8>, ObjectReceipt), ReadAttemptError> {
    let reader = operator.reader(path).await.map_err(|error| ReadAttemptError::Provider {
        operation: "read setup",
        message: error.to_string(),
        retryable: error.is_temporary(),
    })?;
    let mut stream =
        reader.into_bytes_stream(..).await.map_err(|error| ReadAttemptError::Provider {
            operation: "read stream setup",
            message: error.to_string(),
            retryable: error.is_temporary(),
        })?;
    let mut payload = Vec::new();
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    loop {
        let chunk = stream.try_next().await.map_err(|error| ReadAttemptError::Provider {
            operation: "read",
            message: error.to_string(),
            retryable: true,
        })?;
        let Some(chunk) = chunk else {
            break;
        };
        let chunk_size = chunk.len() as u64;
        if chunk_size > max_bytes.saturating_sub(size_bytes) {
            return Err(ReadAttemptError::ResourceLimit);
        }
        digest.update(&chunk);
        payload.extend_from_slice(&chunk);
        size_bytes += chunk_size;
    }
    let receipt = ObjectReceipt { size_bytes, sha256: hex::encode(digest.finalize()) };
    Ok((payload, receipt))
}

async fn write_stream_to_writer<S, F>(
    writer: &mut opendal::Writer,
    stream: S,
    max_bytes: u64,
    mut map_error: F,
) -> CoreResult<ObjectReceipt>
where
    S: Stream<Item = CoreResult<Bytes>> + Send,
    F: FnMut(&'static str, opendal::Error) -> CoreError + Send,
{
    let mut stream = Box::pin(stream);
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    while let Some(item) = stream.next().await {
        let chunk = match item {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = writer.abort().await;
                return Err(error);
            }
        };

        let chunk_size = chunk.len() as u64;
        if chunk_size > max_bytes.saturating_sub(size_bytes) {
            let _ = writer.abort().await;
            return Err(CoreError::ResourceLimit(format!(
                "object exceeds configured limit {max_bytes}"
            )));
        }
        if let Err(error) = writer.write(chunk.clone()).await {
            let _ = writer.abort().await;
            return Err(map_error("write", error));
        }
        size_bytes += chunk_size;
        digest.update(&chunk);
    }

    if let Err(error) = writer.close().await {
        let _ = writer.abort().await;
        return Err(map_error("close", error));
    }
    Ok(ObjectReceipt { size_bytes, sha256: hex::encode(digest.finalize()) })
}

fn write_once_error(key: &str, error: opendal::Error) -> CoreError {
    if matches!(error.kind(), ErrorKind::AlreadyExists | ErrorKind::ConditionNotMatch) {
        CoreError::Storage(format!("object already exists: {key}"))
    } else {
        CoreError::Storage(format!("conditional object write failed: {error}"))
    }
}

fn clean_prefix(prefix: &str) -> CoreResult<String> {
    if prefix.is_empty() {
        return Ok(String::new());
    }
    clean_key(prefix)
}

fn clean_key(key: &str) -> CoreResult<String> {
    let parts = key.split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if key.is_empty()
        || key.starts_with('/')
        || parts.iter().any(|part| matches!(*part, "." | ".."))
    {
        return Err(CoreError::Storage("object key must be a safe relative path".into()));
    }
    Ok(parts.join("/"))
}
