//! Small, deliberately boring PyO3 boundary for the Python package.
//!
//! Rust implementation types stay on the Rust side of this module.  The
//! extension exposes primitive values, `Path` strings, and Python awaitables;
//! the Python facade turns those primitives into the public SDK errors and
//! `pathlib.Path` values.

use crate::config::CoreConfig;
use crate::engine::Engine;
use crate::errors::{CoreError, ProviderError};
use crate::identity::{ArtifactBytes, ProcessingSpec};
use crate::limits::Limits;
use crate::model::{FrameRef, Query};
use crate::source::SourceRegistry;
use crate::source::catalog::SourceCatalog;
use crate::storage::object::{ObjectStore, ObjectStoreConfig};
use crate::transport::FtpTransport;
use pyo3::exceptions::{PyInterruptedError, PyOSError, PyPermissionError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(crate) fn core_error_to_py(error: CoreError) -> PyErr {
    let message = error.to_string();
    match error {
        CoreError::NetworkDisabled(_) => PyPermissionError::new_err(message),
        CoreError::ResourceLimit(_) => PyValueError::new_err(message),
        CoreError::Cancelled => PyInterruptedError::new_err(message),
        CoreError::Provider(ProviderError::AccessDenied) => PyPermissionError::new_err(message),
        CoreError::Transport(_)
        | CoreError::HttpStatus { .. }
        | CoreError::Temporary(_)
        | CoreError::Cache(_)
        | CoreError::Provider(_)
        | CoreError::OutputConflict
        | CoreError::Storage(_)
        | CoreError::CommitOutcomeUnknown => PyOSError::new_err(message),
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
fn source_catalog_json() -> PyResult<String> {
    let catalog =
        SourceCatalog::builtin().map_err(|error| PyValueError::new_err(error.to_string()))?;
    serde_json::to_string(&catalog).map_err(|error| PyValueError::new_err(error.to_string()))
}

#[pyfunction]
fn query_validate_json(query_json: &str) -> PyResult<String> {
    let query: Query = serde_json::from_str(query_json)
        .map_err(|_| PyValueError::new_err("query JSON is invalid"))?;
    let multi_source = query.source.as_deref() == Some("all") || !query.sources.is_empty();
    query.validate(multi_source).map_err(|error| PyValueError::new_err(error.to_string()))?;
    serde_json::to_string(&query).map_err(|error| PyValueError::new_err(error.to_string()))
}

fn identity_frame(frame_json: &str) -> PyResult<FrameRef> {
    let mut value: serde_json::Value = serde_json::from_str(frame_json)
        .map_err(|_| PyValueError::new_err("frame JSON is invalid"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| PyValueError::new_err("frame JSON must be an object"))?;
    object.entry("logical_id").or_insert_with(|| serde_json::Value::String(String::new()));
    serde_json::from_value(value).map_err(|_| PyValueError::new_err("frame JSON is invalid"))
}

fn identity_error(error: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

#[pyfunction]
fn identity_canonical_json(value_json: &str) -> PyResult<String> {
    let value: serde_json::Value = serde_json::from_str(value_json)
        .map_err(|_| PyValueError::new_err("identity JSON is invalid"))?;
    crate::identity::canonical_json(&value).map_err(identity_error)
}

#[pyfunction]
fn identity_digest(value_json: &str) -> PyResult<String> {
    let value: serde_json::Value = serde_json::from_str(value_json)
        .map_err(|_| PyValueError::new_err("identity JSON is invalid"))?;
    crate::identity::digest(&value).map_err(identity_error)
}

#[pyfunction]
fn identity_frame_json(frame_json: &str) -> PyResult<String> {
    let frame = identity_frame(frame_json)?;
    let identity = crate::identity::frame_identity(&frame).map_err(identity_error)?;
    serde_json::to_string(&identity).map_err(identity_error)
}

#[pyfunction]
fn identity_logical_id(frame_json: &str) -> PyResult<String> {
    let frame = identity_frame(frame_json)?;
    crate::identity::logical_id(&frame).map_err(identity_error)
}

#[pyfunction]
fn identity_safe_ref(frame_json: &str) -> PyResult<String> {
    let frame = identity_frame(frame_json)?;
    let safe = crate::identity::safe_ref(&frame).map_err(identity_error)?;
    serde_json::to_string(&safe).map_err(identity_error)
}

#[pyfunction]
fn identity_processing_json(spec_json: &str) -> PyResult<String> {
    let spec: ProcessingSpec = serde_json::from_str(spec_json)
        .map_err(|_| PyValueError::new_err("processing specification JSON is invalid"))?;
    serde_json::to_string(&crate::identity::processing_identity(&spec)).map_err(identity_error)
}

#[pyfunction]
fn identity_processing_hash(spec_json: &str) -> PyResult<String> {
    let spec: ProcessingSpec = serde_json::from_str(spec_json)
        .map_err(|_| PyValueError::new_err("processing specification JSON is invalid"))?;
    crate::identity::processing_hash(&spec).map_err(identity_error)
}

#[pyfunction]
fn identity_output_id(frame_json: &str, revision: &str, spec_json: &str) -> PyResult<String> {
    let frame = identity_frame(frame_json)?;
    let spec: ProcessingSpec = serde_json::from_str(spec_json)
        .map_err(|_| PyValueError::new_err("processing specification JSON is invalid"))?;
    crate::identity::output_id(&frame, revision, &spec).map_err(identity_error)
}

#[pyfunction]
fn identity_cache_key(
    frame_json: &str,
    revision: Option<String>,
    acquisition_version: &str,
    mosaic_version: &str,
) -> PyResult<String> {
    let frame = identity_frame(frame_json)?;
    crate::identity::cache_key(&frame, revision.as_deref(), acquisition_version, mosaic_version)
        .map_err(identity_error)
}

#[pyfunction]
fn identity_artifact_receipt_json(
    name: &str,
    payload: &[u8],
    source_revision: Option<&str>,
) -> PyResult<String> {
    let receipt =
        crate::identity::artifact_receipt(ArtifactBytes { name, bytes: payload, source_revision });
    serde_json::to_string(&receipt).map_err(identity_error)
}

#[pyfunction]
fn identity_resolved_revision(
    artifacts: Vec<(String, Vec<u8>, Option<String>)>,
    upstream_revision: Option<String>,
) -> PyResult<String> {
    let borrowed = artifacts
        .iter()
        .map(|(name, payload, source_revision)| ArtifactBytes {
            name,
            bytes: payload,
            source_revision: source_revision.as_deref(),
        })
        .collect::<Vec<_>>();
    crate::identity::resolved_revision(&borrowed, upstream_revision.as_deref())
        .map_err(identity_error)
}

#[pyfunction]
fn identity_variant_id(output: &str) -> String {
    crate::identity::variant_id(output).to_owned()
}

#[pyfunction]
fn safety_text(value: &str) -> String {
    crate::safety::safe_text(value)
}

#[pyfunction]
fn safety_value_json(value_json: &str) -> PyResult<String> {
    let value: serde_json::Value = serde_json::from_str(value_json)
        .map_err(|_| PyValueError::new_err("value JSON is invalid"))?;
    serde_json::to_string(&crate::safety::safe_value(value)).map_err(identity_error)
}

/// Execute core discovery asynchronously and return a stable JSON result.
/// Provider adapters are registered by the native source layer; no Python
/// callback or GIL-held work is involved in the core operation.
#[pyfunction]
fn discover_json<'py>(
    py: Python<'py>,
    config_json: &str,
    query_json: &str,
) -> PyResult<Bound<'py, PyAny>> {
    let environment = std::collections::BTreeMap::new();
    let config = CoreConfig::layered(Some(config_json), &environment, None)
        .map_err(|_| PyValueError::new_err("core configuration JSON is invalid"))?;
    let query: Query = serde_json::from_str(query_json)
        .map_err(|_| PyValueError::new_err("query JSON is invalid"))?;
    let engine = Engine::new(config, SourceRegistry::default())
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let report = engine
            .discover(query)
            .await
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        serde_json::to_string(&report.safe_document())
            .map_err(|error| PyValueError::new_err(error.to_string()))
    })
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
    m.add_function(wrap_pyfunction!(source_catalog_json, m)?)?;
    m.add_function(wrap_pyfunction!(query_validate_json, m)?)?;
    m.add_function(wrap_pyfunction!(identity_canonical_json, m)?)?;
    m.add_function(wrap_pyfunction!(identity_digest, m)?)?;
    m.add_function(wrap_pyfunction!(identity_frame_json, m)?)?;
    m.add_function(wrap_pyfunction!(identity_logical_id, m)?)?;
    m.add_function(wrap_pyfunction!(identity_safe_ref, m)?)?;
    m.add_function(wrap_pyfunction!(identity_processing_json, m)?)?;
    m.add_function(wrap_pyfunction!(identity_processing_hash, m)?)?;
    m.add_function(wrap_pyfunction!(identity_output_id, m)?)?;
    m.add_function(wrap_pyfunction!(identity_cache_key, m)?)?;
    m.add_function(wrap_pyfunction!(identity_artifact_receipt_json, m)?)?;
    m.add_function(wrap_pyfunction!(identity_resolved_revision, m)?)?;
    m.add_function(wrap_pyfunction!(identity_variant_id, m)?)?;
    m.add_function(wrap_pyfunction!(safety_text, m)?)?;
    m.add_function(wrap_pyfunction!(safety_value_json, m)?)?;
    m.add_function(wrap_pyfunction!(discover_json, m)?)?;
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
