//! Small, deliberately boring PyO3 boundary for the Python package.
//!
//! Rust implementation types stay on the Rust side of this module.  The
//! extension exposes primitive values, `Path` strings, and Python awaitables;
//! the Python facade turns those primitives into the public SDK errors and
//! `pathlib.Path` values.

use crate::errors::CoreError;
use crate::limits::Limits;
use crate::storage::object::{ObjectStore, ObjectStoreConfig};
use crate::transport::FtpTransport;
use pyo3::exceptions::{PyInterruptedError, PyOSError, PyPermissionError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn core_error_to_py(error: CoreError) -> PyErr {
    let message = error.to_string();
    match error {
        CoreError::NetworkDisabled(_) => PyPermissionError::new_err(message),
        CoreError::ResourceLimit(_) => PyValueError::new_err(message),
        CoreError::Cancelled => PyInterruptedError::new_err(message),
        CoreError::Transport(_)
        | CoreError::Temporary(_)
        | CoreError::Cache(_)
        | CoreError::Storage(_) => PyOSError::new_err(message),
    }
}

fn canonical_managed_path(root: &str, candidate: &str) -> Result<PathBuf, CoreError> {
    let root = Path::new(root)
        .canonicalize()
        .map_err(|error| CoreError::Storage(format!("managed root is unavailable: {error}")))?;
    let candidate = Path::new(candidate).canonicalize().map_err(|error| {
        CoreError::Storage(format!("managed candidate is unavailable: {error}"))
    })?;
    if !candidate.starts_with(&root) {
        return Err(CoreError::Storage(format!(
            "managed path escapes root: {}",
            candidate.display()
        )));
    }
    Ok(candidate)
}

#[pyfunction]
fn version() -> &'static str {
    crate::VERSION
}

#[pyfunction]
fn sha256(data: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(data);
    hex::encode(digest.finalize())
}

#[pyfunction]
fn validate_size(size: u64, limit: u64) -> PyResult<u64> {
    if size > limit {
        return Err(core_error_to_py(CoreError::ResourceLimit(format!(
            "payload size {size} exceeds configured limit {limit}"
        ))));
    }
    Ok(size)
}

#[pyfunction]
fn managed_path(root: &str, candidate: &str) -> PyResult<String> {
    canonical_managed_path(root, candidate)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(core_error_to_py)
}

#[pyfunction]
fn path_is_within(root: &str, candidate: &str) -> PyResult<bool> {
    let root = Path::new(root)
        .canonicalize()
        .map_err(|error| core_error_to_py(CoreError::Storage(error.to_string())))?;
    let candidate = Path::new(candidate)
        .canonicalize()
        .map_err(|error| core_error_to_py(CoreError::Storage(error.to_string())))?;
    Ok(candidate.starts_with(root))
}

/// Return a Python awaitable backed by a Tokio future.
///
/// `future_into_py` ties cancellation of the Python future to dropping the
/// Rust future, so no detached worker survives a cancelled await.
#[pyfunction]
fn sleep(py: Python<'_>, seconds: f64) -> PyResult<Bound<'_, PyAny>> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(PyValueError::new_err("sleep duration must be finite and non-negative"));
    }
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        Ok(())
    })
}

fn object_store(
    provider: String,
    bucket: String,
    prefix: String,
    endpoint: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    anonymous: bool,
) -> PyResult<ObjectStore> {
    ObjectStore::from_config(ObjectStoreConfig {
        provider,
        bucket,
        prefix,
        endpoint,
        region,
        access_key_id,
        secret_access_key,
        anonymous,
    })
    .map_err(core_error_to_py)
}

#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn object_read(
    py: Python<'_>,
    provider: String,
    bucket: String,
    prefix: String,
    key: String,
    endpoint: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    anonymous: bool,
    max_bytes: u64,
) -> PyResult<Bound<'_, PyAny>> {
    let store = object_store(
        provider,
        bucket,
        prefix,
        endpoint,
        region,
        access_key_id,
        secret_access_key,
        anonymous,
    )?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let result = store.read_optional(&key, max_bytes).await.map_err(core_error_to_py)?;
        Ok(Python::attach(|py| {
            result.map(|(payload, receipt)| {
                (PyBytes::new(py, &payload).unbind(), receipt.size_bytes, receipt.sha256)
            })
        }))
    })
}

#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn object_write(
    py: Python<'_>,
    provider: String,
    bucket: String,
    prefix: String,
    key: String,
    chunks: Vec<Vec<u8>>,
    endpoint: Option<String>,
    region: Option<String>,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    anonymous: bool,
    overwrite: bool,
    max_bytes: u64,
) -> PyResult<Bound<'_, PyAny>> {
    let store = object_store(
        provider,
        bucket,
        prefix,
        endpoint,
        region,
        access_key_id,
        secret_access_key,
        anonymous,
    )?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let receipt = if overwrite {
            store.write_chunks(&key, chunks, max_bytes).await
        } else {
            store.write_chunks_once(&key, chunks, max_bytes).await
        }
        .map_err(core_error_to_py)?;
        Ok((receipt.size_bytes, receipt.sha256))
    })
}

fn ftp_transport(
    allow_public: bool,
    max_bytes: u64,
    request_timeout_secs: u64,
    frame_deadline_secs: u64,
) -> FtpTransport {
    let mut limits = Limits::default();
    limits.max_artifact_bytes = max_bytes;
    limits.max_frame_bytes = max_bytes;
    limits.request_timeout_secs = request_timeout_secs;
    limits.frame_deadline_secs = frame_deadline_secs;
    FtpTransport::new(limits, allow_public)
}

#[pyfunction]
fn ftp_list(
    py: Python<'_>,
    address: String,
    username: String,
    password: String,
    allow_public: bool,
    max_bytes: u64,
    request_timeout_secs: u64,
    frame_deadline_secs: u64,
) -> PyResult<Bound<'_, PyAny>> {
    let transport =
        ftp_transport(allow_public, max_bytes, request_timeout_secs, frame_deadline_secs);
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        transport.list(&address, &username, &password).await.map_err(core_error_to_py)
    })
}

#[pyfunction]
fn ftp_nlst(
    py: Python<'_>,
    address: String,
    username: String,
    password: String,
    allow_public: bool,
    max_bytes: u64,
    request_timeout_secs: u64,
    frame_deadline_secs: u64,
) -> PyResult<Bound<'_, PyAny>> {
    let transport =
        ftp_transport(allow_public, max_bytes, request_timeout_secs, frame_deadline_secs);
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        transport.names(&address, &username, &password).await.map_err(core_error_to_py)
    })
}

#[pyfunction]
fn ftp_read(
    py: Python<'_>,
    address: String,
    username: String,
    password: String,
    allow_public: bool,
    max_bytes: u64,
    request_timeout_secs: u64,
    frame_deadline_secs: u64,
) -> PyResult<Bound<'_, PyAny>> {
    let transport =
        ftp_transport(allow_public, max_bytes, request_timeout_secs, frame_deadline_secs);
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let object =
            transport.get_bytes(&address, &username, &password).await.map_err(core_error_to_py)?;
        Ok(object.bytes)
    })
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(sha256, m)?)?;
    m.add_function(wrap_pyfunction!(validate_size, m)?)?;
    m.add_function(wrap_pyfunction!(managed_path, m)?)?;
    m.add_function(wrap_pyfunction!(path_is_within, m)?)?;
    m.add_function(wrap_pyfunction!(sleep, m)?)?;
    m.add_function(wrap_pyfunction!(object_read, m)?)?;
    m.add_function(wrap_pyfunction!(object_write, m)?)?;
    m.add_function(wrap_pyfunction!(ftp_list, m)?)?;
    m.add_function(wrap_pyfunction!(ftp_nlst, m)?)?;
    m.add_function(wrap_pyfunction!(ftp_read, m)?)?;
    m.add("VERSION", crate::VERSION)?;
    Ok(())
}
