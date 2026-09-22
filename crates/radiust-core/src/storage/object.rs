//! OpenDAL-backed object storage primitives.
//!
//! The Python layer owns output identity and manifest state.  This module only
//! handles safe object keys, bounded streaming I/O, and content receipts.

use crate::errors::{CoreError, CoreResult};
use bytes::Bytes;
use futures_util::TryStreamExt;
use opendal::{ErrorKind, Operator, services};
use sha2::{Digest, Sha256};

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
        let reader =
            self.operator.reader(&path).await.map_err(|error| {
                CoreError::Transport(format!("object read setup failed: {error}"))
            })?;
        let mut stream = reader
            .into_bytes_stream(..)
            .await
            .map_err(|error| CoreError::Transport(format!("object read stream failed: {error}")))?;
        let mut payload = Vec::new();
        let mut digest = Sha256::new();
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|error| CoreError::Transport(format!("object read failed: {error}")))?
        {
            if payload.len() as u64 + chunk.len() as u64 > max_bytes {
                return Err(CoreError::ResourceLimit(format!(
                    "object exceeds configured limit {max_bytes}"
                )));
            }
            digest.update(&chunk);
            payload.extend_from_slice(&chunk);
        }
        let receipt = ObjectReceipt {
            size_bytes: payload.len() as u64,
            sha256: hex::encode(digest.finalize()),
        };
        Ok((payload, receipt))
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
        let path = self.object_key(key)?;
        let mut writer =
            self.operator.writer(&path).await.map_err(|error| {
                CoreError::Storage(format!("object writer setup failed: {error}"))
            })?;
        let mut digest = Sha256::new();
        let mut size_bytes = 0_u64;
        for chunk in chunks {
            if size_bytes.saturating_add(chunk.len() as u64) > max_bytes {
                let _ = writer.abort().await;
                return Err(CoreError::ResourceLimit(format!(
                    "object exceeds configured limit {max_bytes}"
                )));
            }
            size_bytes += chunk.len() as u64;
            digest.update(&chunk);
            if let Err(error) = writer.write(Bytes::from(chunk)).await {
                let _ = writer.abort().await;
                return Err(CoreError::Storage(format!("object write failed: {error}")));
            }
        }
        if let Err(error) = writer.close().await {
            let _ = writer.abort().await;
            return Err(CoreError::Storage(format!("object close failed: {error}")));
        }
        Ok(ObjectReceipt { size_bytes, sha256: hex::encode(digest.finalize()) })
    }

    pub async fn write_chunks_once(
        &self,
        key: &str,
        chunks: Vec<Vec<u8>>,
        max_bytes: u64,
    ) -> CoreResult<ObjectReceipt> {
        let path = self.object_key(key)?;
        // Conditional creation is enforced by the storage provider at publish
        // time. An exists() probe followed by a normal write is not atomic.
        let mut writer = self
            .operator
            .writer_with(&path)
            .if_not_exists(true)
            .await
            .map_err(|error| write_once_error(key, error))?;
        let mut digest = Sha256::new();
        let mut size_bytes = 0_u64;
        for chunk in chunks {
            if size_bytes.saturating_add(chunk.len() as u64) > max_bytes {
                let _ = writer.abort().await;
                return Err(CoreError::ResourceLimit(format!(
                    "object exceeds configured limit {max_bytes}"
                )));
            }
            size_bytes += chunk.len() as u64;
            digest.update(&chunk);
            if let Err(error) = writer.write(Bytes::from(chunk)).await {
                let _ = writer.abort().await;
                return Err(write_once_error(key, error));
            }
        }
        if let Err(error) = writer.close().await {
            let _ = writer.abort().await;
            return Err(write_once_error(key, error));
        }
        Ok(ObjectReceipt { size_bytes, sha256: hex::encode(digest.finalize()) })
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
