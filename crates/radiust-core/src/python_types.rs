//! Explicit PyO3 wrappers for serialization-friendly core domain values.

use crate::config::CoreConfig;
use crate::download::{
    DecodedFetchBatchReport, DecodedFetchItem, DecodedFetchStream, DecodedGrid, DecodedProcessing,
    DownloadBatchReport, DownloadStatus, FetchBatchReport, FetchErrorPolicy, FetchStatus,
};
use crate::engine::Engine;
use crate::grid::Resampling;
use crate::model::{DiscoveryReport, FrameRef, Grid, Query, RadarDataset, RadarField, RawFrame};
use crate::runtime::RuntimeEventReceiver;
use crate::source::SourceRegistry;
use pyo3::exceptions::{PyOSError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyType};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

fn parse_json<T: serde::de::DeserializeOwned>(value: &str, label: &str) -> PyResult<T> {
    serde_json::from_str(value)
        .map_err(|_| PyValueError::new_err(format!("{label} JSON is invalid")))
}

fn to_json<T: Serialize>(value: &T) -> PyResult<String> {
    serde_json::to_string(value)
        .map_err(|_| PyValueError::new_err("core value could not be serialized"))
}

#[pyclass(name = "ResolvedConfig", module = "radiust._core", frozen)]
pub struct PyResolvedConfig {
    config: CoreConfig,
    origins: BTreeMap<String, String>,
}

#[pymethods]
impl PyResolvedConfig {
    fn values_json(&self) -> PyResult<String> {
        sdk_config_json(&self.config)
    }

    fn origins_json(&self) -> PyResult<String> {
        to_json(&self.origins)
    }

    fn redacted_json(&self) -> PyResult<String> {
        sdk_config_json(&self.config.redacted())
    }
}

fn sdk_config_json(config: &CoreConfig) -> PyResult<String> {
    let mut value = serde_json::to_value(config)
        .map_err(|_| PyValueError::new_err("core value could not be serialized"))?;
    if let Some(output) = value.get_mut("output").and_then(Value::as_object_mut) {
        // This field belongs to the native CoreConfig model but was not part of
        // the Python EffectiveConfig.values mapping.
        output.remove("resampling");
    }
    serde_json::to_string(&value)
        .map_err(|_| PyValueError::new_err("core value could not be serialized"))
}

#[pyfunction]
fn resolve_config(
    overrides_json: Option<&str>,
    path: Option<&str>,
    environment_json: &str,
) -> PyResult<PyResolvedConfig> {
    let environment: Vec<(String, String)> = serde_json::from_str(environment_json)
        .map_err(|_| PyValueError::new_err("configuration environment is invalid"))?;
    let parsed = CoreConfig::load_with_ordered_environment(
        path.map(Path::new),
        &environment,
        overrides_json,
    )
    .map_err(|error| PyValueError::new_err(error.to_string()))?;
    let (mut config, origins) = parsed;
    config.normalize_sdk_paths().map_err(|error| PyValueError::new_err(error.to_string()))?;
    Ok(PyResolvedConfig { config, origins })
}

#[pyfunction]
fn redact_config_values_json(value_json: &str) -> PyResult<String> {
    let mut value: Value = parse_json(value_json, "configuration values")?;
    crate::config::redact_json_value(&mut value);
    to_json(&value)
}

fn parse_decoded_processing(value: Option<&str>) -> PyResult<DecodedProcessing> {
    let Some(value) = value else {
        return Ok(DecodedProcessing::default());
    };
    let options: Value = parse_json(value, "processing options")?;
    let options = options
        .as_object()
        .ok_or_else(|| PyValueError::new_err("processing options must be a JSON object"))?;
    const ALLOWED: &[&str] = &["variable", "grid", "bbox", "resolution", "resampling"];
    if let Some(unknown) = options.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(PyValueError::new_err(format!("unsupported processing option: {unknown}")));
    }

    let variable = match options.get("variable") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(_) => return Err(PyValueError::new_err("variable must be a non-empty string")),
    };
    let grid = match options.get("grid") {
        None => "native",
        Some(Value::String(value)) => value.as_str(),
        Some(_) => return Err(PyValueError::new_err("grid must be native or geographic")),
    };
    let bbox = match options.get("bbox") {
        None | Some(Value::Null) => None,
        Some(Value::Array(values)) if values.len() == 4 => {
            let mut parsed = [0.0; 4];
            for (index, value) in values.iter().enumerate() {
                parsed[index] =
                    value.as_f64().filter(|number| number.is_finite()).ok_or_else(|| {
                        PyValueError::new_err("bbox must contain four finite numbers")
                    })?;
            }
            Some(parsed)
        }
        Some(_) => return Err(PyValueError::new_err("bbox must contain four finite numbers")),
    };
    let resolution = match options.get("resolution") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_f64()
                .filter(|number| number.is_finite() && *number > 0.0)
                .ok_or_else(|| PyValueError::new_err("resolution must be finite and positive"))?,
        ),
    };
    let resampling = match options.get("resampling") {
        None => "nearest",
        Some(Value::String(value)) => value.as_str(),
        Some(_) => return Err(PyValueError::new_err("resampling must be nearest or bilinear")),
    };
    let resampling = Resampling::parse(resampling)
        .ok_or_else(|| PyValueError::new_err("resampling must be nearest or bilinear"))?;

    let grid = match (grid, bbox, resolution) {
        ("native", None, None) => DecodedGrid::Native,
        ("native", _, _) => {
            return Err(PyValueError::new_err("bbox and resolution require grid=geographic"));
        }
        ("geographic", Some([west, south, east, north]), Some(resolution)) => {
            if !(-180.0..=180.0).contains(&west)
                || !(-180.0..=180.0).contains(&east)
                || !(-90.0..=90.0).contains(&south)
                || !(-90.0..=90.0).contains(&north)
                || west >= east
                || south >= north
            {
                return Err(PyValueError::new_err(
                    "bbox must have positive area within geographic bounds",
                ));
            }
            DecodedGrid::Geographic { bbox: [west, south, east, north], resolution }
        }
        ("geographic", _, _) => {
            return Err(PyValueError::new_err("grid=geographic requires bbox and resolution"));
        }
        _ => return Err(PyValueError::new_err("grid must be native or geographic")),
    };
    Ok(DecodedProcessing { variable, grid, resampling })
}

#[pyclass(name = "Query", module = "radiust._core", frozen)]
pub struct PyQuery {
    inner: Query,
}

#[pymethods]
impl PyQuery {
    #[new]
    fn new(query_json: &str) -> PyResult<Self> {
        let query: Query = parse_json(query_json, "query")?;
        let multi = query.source.as_deref() == Some("all") || !query.sources.is_empty();
        query.validate(multi).map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self { inner: query })
    }

    #[getter]
    fn source(&self) -> Option<String> {
        self.inner.source.clone()
    }

    #[getter]
    fn sources(&self) -> Vec<String> {
        self.inner.sources.clone()
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&self.inner)
    }
}

#[pyclass(name = "FrameRef", module = "radiust._core", frozen)]
pub struct PyFrameRef {
    inner: FrameRef,
}

#[pymethods]
impl PyFrameRef {
    #[new]
    fn new(frame_json: &str) -> PyResult<Self> {
        Ok(Self { inner: parse_json(frame_json, "frame")? })
    }

    #[getter]
    fn source(&self) -> String {
        self.inner.source.clone()
    }

    #[getter]
    fn product(&self) -> String {
        self.inner.product.clone()
    }

    #[getter]
    fn station(&self) -> Option<String> {
        self.inner.station.clone()
    }

    #[getter]
    fn valid_time(&self) -> String {
        self.inner.valid_time.clone()
    }

    #[getter]
    fn base_time(&self) -> Option<String> {
        self.inner.base_time.clone()
    }

    #[getter]
    fn revision(&self) -> Option<String> {
        self.inner.revision.clone()
    }

    #[getter]
    fn locator_version(&self) -> String {
        self.inner.locator_version.clone()
    }

    #[getter]
    fn logical_id(&self) -> String {
        self.inner.logical_id.clone()
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&self.inner)
    }
}

#[pyclass(name = "RadarField", module = "radiust._core", frozen)]
pub struct PyRadarField {
    backing: RadarFieldBacking,
}

enum RadarFieldBacking {
    Owned(Arc<RadarField>),
    Dataset { owner: Arc<RadarDataset>, index: usize },
}

impl PyRadarField {
    fn value(&self) -> &RadarField {
        match &self.backing {
            RadarFieldBacking::Owned(field) => field,
            RadarFieldBacking::Dataset { owner, index } => {
                owner.fields.get(*index).expect("dataset field index is validated")
            }
        }
    }
}

#[pymethods]
impl PyRadarField {
    #[new]
    fn new(field_json: &str) -> PyResult<Self> {
        let field: RadarField = parse_json(field_json, "radar field")?;
        field.validate().map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self { backing: RadarFieldBacking::Owned(Arc::new(field)) })
    }

    #[getter]
    fn name(&self) -> String {
        self.value().name.clone()
    }

    #[getter]
    fn shape(&self) -> Vec<usize> {
        self.value().shape.clone()
    }

    /// Explicitly materialize a Python-owned copy of the core values.
    fn copy_values(&self) -> Vec<f32> {
        self.value().values.clone()
    }

    /// Return packed little-endian float32 values for explicit NumPy conversion.
    fn values_le_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let field = self.value();
        let capacity = field.values.len().checked_mul(4).ok_or_else(|| {
            PyValueError::new_err("field value byte size overflows address space")
        })?;
        let mut bytes = Vec::with_capacity(capacity);
        for value in &field.values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Ok(PyBytes::new(py, &bytes))
    }

    /// Return packed little-endian uint16 quality flags for explicit conversion.
    fn quality_le_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let field = self.value();
        let capacity =
            field.quality.len().checked_mul(2).ok_or_else(|| {
                PyValueError::new_err("quality byte size overflows address space")
            })?;
        let mut bytes = Vec::with_capacity(capacity);
        for value in &field.quality {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        Ok(PyBytes::new(py, &bytes))
    }

    fn metadata_json(&self) -> PyResult<String> {
        let field = self.value();
        to_json(&serde_json::json!({
            "name": &field.name,
            "shape": &field.shape,
            "units": &field.units,
            "valid_time": &field.valid_time,
            "grid": &field.grid,
            "provenance": &field.provenance,
        }))
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(self.value())
    }
}

#[pyclass(name = "RadarDataset", module = "radiust._core", frozen)]
pub struct PyRadarDataset {
    inner: Arc<RadarDataset>,
}

#[pymethods]
impl PyRadarDataset {
    #[new]
    fn new(dataset_json: &str) -> PyResult<Self> {
        let dataset: RadarDataset = parse_json(dataset_json, "radar dataset")?;
        dataset.validate().map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self { inner: Arc::new(dataset) })
    }

    #[getter]
    fn field_count(&self) -> usize {
        self.inner.fields.len()
    }

    #[getter]
    fn valid_time(&self) -> String {
        self.inner.valid_time.clone()
    }

    #[getter]
    fn source(&self) -> String {
        self.inner.source.clone()
    }

    fn field(&self, index: usize) -> PyResult<PyRadarField> {
        if index >= self.inner.fields.len() {
            return Err(PyValueError::new_err("field index is out of range"));
        }
        Ok(PyRadarField {
            backing: RadarFieldBacking::Dataset { owner: self.inner.clone(), index },
        })
    }

    fn field_json(&self, index: usize) -> PyResult<String> {
        let field = self
            .inner
            .fields
            .get(index)
            .ok_or_else(|| PyValueError::new_err("field index is out of range"))?;
        to_json(field)
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(self.inner.as_ref())
    }
}

#[pyclass(name = "DiscoveryReport", module = "radiust._core", frozen)]
pub struct PyDiscoveryReport {
    inner: DiscoveryReport,
}

impl PyDiscoveryReport {
    fn from_json(value: &str) -> PyResult<Self> {
        let report: DiscoveryReport = parse_json(value, "discovery report")?;
        Self::from_report(report)
    }

    fn from_report(report: DiscoveryReport) -> PyResult<Self> {
        let checked = DiscoveryReport::from_items(
            report.query.clone(),
            report.items.clone(),
            report.interrupted,
        )
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
        if checked.counts != report.counts {
            return Err(PyValueError::new_err("discovery report counts are inconsistent"));
        }
        Ok(Self { inner: report })
    }
}

#[pymethods]
impl PyDiscoveryReport {
    #[new]
    fn new(report_json: &str) -> PyResult<Self> {
        Self::from_json(report_json)
    }

    #[getter]
    fn total(&self) -> usize {
        self.inner.counts.total
    }

    #[getter]
    fn interrupted(&self) -> bool {
        self.inner.interrupted
    }

    /// Return a frame reference retaining its private in-process locator.
    fn frame(&self, index: usize) -> PyResult<PyFrameRef> {
        let item = self
            .inner
            .items
            .get(index)
            .ok_or_else(|| PyValueError::new_err("discovery item index is out of range"))?;
        let frame = item
            .frame
            .as_ref()
            .ok_or_else(|| PyValueError::new_err("discovery item has no frame"))?;
        Ok(PyFrameRef { inner: frame.clone() })
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&self.inner)
    }
}

#[pyclass(name = "RawFrame", module = "radiust._core", frozen)]
pub struct PyRawFrame {
    inner: Arc<Mutex<Option<Arc<RawFrame>>>>,
}

impl PyRawFrame {
    fn get_inner(&self) -> PyResult<Arc<RawFrame>> {
        self.inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("RawFrame is unavailable"))?
            .clone()
            .ok_or_else(|| PyRuntimeError::new_err("RawFrame is closed"))
    }
}

#[pymethods]
impl PyRawFrame {
    #[getter]
    fn artifact_count(&self) -> PyResult<usize> {
        Ok(self.get_inner()?.artifacts.len())
    }

    fn receipt_json(&self) -> PyResult<String> {
        to_json(&self.get_inner()?.public_receipt())
    }

    fn artifact_json(&self, index: usize) -> PyResult<String> {
        let raw = self.get_inner()?;
        let artifact = raw
            .artifacts
            .get(index)
            .ok_or_else(|| PyValueError::new_err("artifact index is out of range"))?;
        to_json(&artifact.receipt)
    }

    fn artifact_path(&self, index: usize) -> PyResult<String> {
        let raw = self.get_inner()?;
        let artifact = raw
            .artifacts
            .get(index)
            .ok_or_else(|| PyValueError::new_err("artifact index is out of range"))?;
        Ok(artifact.path.to_string_lossy().into_owned())
    }

    fn artifact_bytes<'py>(&self, py: Python<'py>, index: usize) -> PyResult<Bound<'py, PyBytes>> {
        let raw = self.get_inner()?;
        let artifact = raw
            .artifacts
            .get(index)
            .ok_or_else(|| PyValueError::new_err("artifact index is out of range"))?;
        let bytes = std::fs::read(&artifact.path)
            .map_err(|_| PyOSError::new_err("raw artifact is unavailable"))?;
        Ok(PyBytes::new(py, &bytes))
    }

    fn close(&self) -> PyResult<()> {
        self.inner.lock().map_err(|_| PyRuntimeError::new_err("RawFrame is unavailable"))?.take();
        Ok(())
    }
}

struct PyFetchItemData {
    input_index: usize,
    frame: FrameRef,
    status: FetchStatus,
    raw: Option<Arc<RawFrame>>,
    data: Option<Arc<RadarField>>,
    error: Option<String>,
    error_details: Option<String>,
}

#[pyclass(name = "FetchItem", module = "radiust._core", frozen)]
pub struct PyFetchItem {
    inner: Arc<PyFetchItemData>,
}

#[pymethods]
impl PyFetchItem {
    #[getter]
    fn input_index(&self) -> usize {
        self.inner.input_index
    }

    #[getter]
    fn status(&self) -> &'static str {
        fetch_status_name(self.inner.status)
    }

    #[getter]
    fn error(&self) -> Option<String> {
        self.inner.error.clone()
    }

    fn frame(&self) -> PyFrameRef {
        PyFrameRef { inner: self.inner.frame.clone() }
    }

    fn raw(&self) -> Option<PyRawFrame> {
        self.inner
            .raw
            .as_ref()
            .map(|raw| PyRawFrame { inner: Arc::new(Mutex::new(Some(raw.clone()))) })
    }

    fn data(&self) -> Option<PyRadarField> {
        self.inner
            .data
            .as_ref()
            .map(|field| PyRadarField { backing: RadarFieldBacking::Owned(field.clone()) })
    }

    fn error_details(&self) -> Option<String> {
        self.inner.error_details.clone()
    }
}

fn py_fetch_item_data(item: DecodedFetchItem) -> Arc<PyFetchItemData> {
    let error_details =
        item.error_details.as_ref().and_then(|error| serde_json::to_string(error).ok());
    Arc::new(PyFetchItemData {
        input_index: item.input_index,
        frame: item.frame,
        status: item.status,
        raw: None,
        data: item.data.map(Arc::new),
        error: item.error,
        error_details,
    })
}

impl PyFetchItem {
    fn from_decoded_item(item: DecodedFetchItem) -> Self {
        Self { inner: py_fetch_item_data(item) }
    }
}

#[pyclass(name = "FetchBatchReport", module = "radiust._core", frozen)]
pub struct PyFetchBatchReport {
    items: Vec<Arc<PyFetchItemData>>,
    planned: usize,
    success: usize,
    failed: usize,
    cancelled: usize,
    not_started: usize,
}

impl PyFetchBatchReport {
    fn from_report(report: FetchBatchReport) -> Self {
        Self {
            items: report
                .items
                .into_iter()
                .map(|item| {
                    Arc::new(PyFetchItemData {
                        input_index: item.input_index,
                        frame: item.frame,
                        status: item.status,
                        raw: item.raw.map(Arc::new),
                        data: None,
                        error: item.error,
                        error_details: None,
                    })
                })
                .collect(),
            planned: report.planned,
            success: report.success,
            failed: report.failed,
            cancelled: report.cancelled,
            not_started: report.not_started,
        }
    }

    fn from_decoded_report(report: DecodedFetchBatchReport) -> Self {
        Self {
            items: report.items.into_iter().map(py_fetch_item_data).collect(),
            planned: report.planned,
            success: report.success,
            failed: report.failed,
            cancelled: report.cancelled,
            not_started: report.not_started,
        }
    }
}

#[pyclass(name = "FetchStream", module = "radiust._core")]
pub struct PyFetchStream {
    inner: Arc<tokio::sync::Mutex<DecodedFetchStream>>,
    cancellation: tokio_util::sync::CancellationToken,
}

#[pymethods]
impl PyFetchStream {
    /// Return (item, partial_report, cause); the latter two are set on stop-mode failure.
    fn next<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let mut stream = inner.lock().await;
            let outcome = stream.next().await;
            Python::attach(|py| -> PyResult<_> {
                match outcome {
                    Ok(Some(item)) => Ok((
                        Some(Py::new(py, PyFetchItem::from_decoded_item(item))?),
                        None::<Py<PyFetchBatchReport>>,
                        None::<String>,
                    )),
                    Ok(None) => Ok((
                        None::<Py<PyFetchItem>>,
                        None::<Py<PyFetchBatchReport>>,
                        None::<String>,
                    )),
                    Err(failure) => Ok((
                        None::<Py<PyFetchItem>>,
                        Some(Py::new(
                            py,
                            PyFetchBatchReport::from_decoded_report(failure.partial_result),
                        )?),
                        Some(failure.cause),
                    )),
                }
            })
        })
    }

    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.cancellation.cancel();
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner.lock().await.close();
            Ok(())
        })
    }
}

#[pymethods]
impl PyFetchBatchReport {
    #[getter]
    fn total(&self) -> usize {
        self.items.len()
    }

    #[getter]
    fn planned(&self) -> usize {
        self.planned
    }

    #[getter]
    fn success(&self) -> usize {
        self.success
    }

    #[getter]
    fn failed(&self) -> usize {
        self.failed
    }

    #[getter]
    fn cancelled(&self) -> usize {
        self.cancelled
    }

    #[getter]
    fn not_started(&self) -> usize {
        self.not_started
    }

    fn item(&self, index: usize) -> PyResult<PyFetchItem> {
        let item = self
            .items
            .get(index)
            .ok_or_else(|| PyValueError::new_err("fetch item index is out of range"))?;
        Ok(PyFetchItem { inner: item.clone() })
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&serde_json::json!({
            "items": self.items.iter().map(|item| serde_json::json!({
                "input_index": item.input_index,
                "frame": item.frame,
                "status": fetch_status_name(item.status),
                "error": item.error,
                "error_details": item.error_details,
                "raw_available": item.raw.is_some(),
                "data_available": item.data.is_some(),
            })).collect::<Vec<_>>(),
            "planned": self.planned,
            "success": self.success,
            "failed": self.failed,
            "cancelled": self.cancelled,
            "not_started": self.not_started,
        }))
    }
}

fn fetch_status_name(status: FetchStatus) -> &'static str {
    match status {
        FetchStatus::Planned => "planned",
        FetchStatus::Success => "success",
        FetchStatus::Failed => "failed",
        FetchStatus::Cancelled => "cancelled",
        FetchStatus::NotStarted => "not_started",
    }
}

struct PyDownloadItemData {
    report: Arc<DownloadBatchReport>,
    index: usize,
}

#[pyclass(name = "DownloadItem", module = "radiust._core", frozen)]
pub struct PyDownloadItem {
    inner: Arc<PyDownloadItemData>,
}

impl PyDownloadItem {
    fn value(&self) -> &crate::download::DownloadItem {
        &self.inner.report.items[self.inner.index]
    }
}

#[pymethods]
impl PyDownloadItem {
    #[getter]
    fn input_index(&self) -> usize {
        self.value().input_index
    }

    #[getter]
    fn status(&self) -> &'static str {
        download_status_name(self.value().status)
    }

    #[getter]
    fn output_uri(&self) -> Option<String> {
        self.value().output_uri.clone()
    }

    fn error_json(&self) -> PyResult<Option<String>> {
        self.value().error.as_ref().map(to_json).transpose()
    }

    fn frame(&self) -> PyFrameRef {
        PyFrameRef { inner: self.value().frame.clone() }
    }
}

#[pyclass(name = "DownloadBatchReport", module = "radiust._core", frozen)]
pub struct PyDownloadBatchReport {
    inner: Arc<DownloadBatchReport>,
}

impl PyDownloadBatchReport {
    fn new(report: DownloadBatchReport) -> Self {
        Self { inner: Arc::new(report) }
    }
}

#[pymethods]
impl PyDownloadBatchReport {
    #[getter]
    fn total(&self) -> usize {
        self.inner.items.len()
    }

    #[getter]
    fn interrupted(&self) -> bool {
        self.inner.interrupted
    }

    #[getter]
    fn planned(&self) -> usize {
        self.inner.planned
    }

    #[getter]
    fn written(&self) -> usize {
        self.inner.written
    }

    #[getter]
    fn skipped(&self) -> usize {
        self.inner.skipped
    }

    #[getter]
    fn failed(&self) -> usize {
        self.inner.failed
    }

    #[getter]
    fn cancelled(&self) -> usize {
        self.inner.cancelled
    }

    #[getter]
    fn not_started(&self) -> usize {
        self.inner.not_started
    }

    fn item(&self, index: usize) -> PyResult<PyDownloadItem> {
        if index >= self.inner.items.len() {
            return Err(PyValueError::new_err("download item index is out of range"));
        }
        Ok(PyDownloadItem {
            inner: Arc::new(PyDownloadItemData { report: self.inner.clone(), index }),
        })
    }

    fn to_json(&self) -> PyResult<String> {
        to_json(&*self.inner)
    }
}

fn download_status_name(status: DownloadStatus) -> &'static str {
    match status {
        DownloadStatus::Planned => "planned",
        DownloadStatus::Written => "written",
        DownloadStatus::Skipped => "skipped",
        DownloadStatus::Failed => "failed",
        DownloadStatus::Cancelled => "cancelled",
        DownloadStatus::NotStarted => "not_started",
    }
}

#[pyclass(name = "Engine", module = "radiust._core")]
pub struct PyEngine {
    inner: Arc<Engine>,
}

#[pyclass(name = "OperationEvents", module = "radiust._core")]
pub struct PyOperationEvents {
    receiver: Mutex<RuntimeEventReceiver>,
    dropped_events: AtomicU64,
}

#[pymethods]
impl PyOperationEvents {
    /// Drain currently available safe events as a JSON array without blocking.
    fn drain_json(&self) -> PyResult<String> {
        let mut receiver = self
            .receiver
            .lock()
            .map_err(|_| PyOSError::new_err("operation event receiver is unavailable"))?;
        let mut events = Vec::new();
        let mut dropped = 0_u64;
        loop {
            match receiver.try_recv() {
                Ok(event) => events.push(event),
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(count)) => {
                    dropped = dropped.saturating_add(count);
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
            }
        }
        self.dropped_events.fetch_add(dropped, Ordering::Relaxed);
        to_json(&events)
    }

    /// Number of events skipped because this bounded receiver lagged.
    #[getter]
    fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }
}

#[pymethods]
impl PyEngine {
    #[new]
    fn new(config_json: &str) -> PyResult<Self> {
        let environment = std::collections::BTreeMap::new();
        let config = CoreConfig::layered(Some(config_json), &environment, None)
            .map_err(|_| PyValueError::new_err("core configuration JSON is invalid"))?;
        let inner = Engine::new(config, SourceRegistry::default())
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self { inner: Arc::new(inner) })
    }

    #[classmethod]
    fn from_resolved_config(
        _class: &Bound<'_, PyType>,
        config: PyRef<'_, PyResolvedConfig>,
    ) -> PyResult<Self> {
        let inner = Engine::new(config.config.clone(), SourceRegistry::default())
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
        Ok(Self { inner: Arc::new(inner) })
    }

    /// Cancel operations owned by this engine. A cancelled engine is terminal;
    /// create a new Engine to begin another operation.
    fn cancel(&self) {
        self.inner.cancel();
    }

    #[getter]
    fn cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }

    /// Subscribe to bounded, sanitized lifecycle events before starting work.
    fn subscribe_events(&self) -> PyOperationEvents {
        PyOperationEvents {
            receiver: Mutex::new(self.inner.subscribe_events()),
            dropped_events: AtomicU64::new(0),
        }
    }

    fn discover<'py>(&self, py: Python<'py>, query: Py<PyQuery>) -> PyResult<Bound<'py, PyAny>> {
        let query = Python::attach(|py| query.borrow(py).inner.clone());
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let report = engine
                .discover(query)
                .await
                .map_err(|error| PyValueError::new_err(error.to_string()))?;
            let report = PyDiscoveryReport::from_report(report)?;
            Python::attach(|py| Py::new(py, report))
        })
    }

    fn fetch_raw<'py>(
        &self,
        py: Python<'py>,
        frame: Py<PyFrameRef>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let frame = Python::attach(|py| frame.borrow(py).inner.clone());
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let raw = engine.fetch_raw(frame).await.map_err(engine_error_to_py)?;
            Python::attach(|py| {
                Py::new(py, PyRawFrame { inner: Arc::new(Mutex::new(Some(Arc::new(raw)))) })
            })
        })
    }

    fn load_raw_manifest<'py>(
        &self,
        py: Python<'py>,
        manifest_path: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let raw = engine
                .load_raw_manifest(PathBuf::from(manifest_path))
                .await
                .map_err(engine_error_to_py)?;
            Python::attach(|py| {
                Py::new(py, PyRawFrame { inner: Arc::new(Mutex::new(Some(Arc::new(raw)))) })
            })
        })
    }

    fn decode_science<'py>(
        &self,
        py: Python<'py>,
        raw: Py<PyRawFrame>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let raw = Python::attach(|py| raw.borrow(py).get_inner())?;
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let field = engine.decode_science(raw).await.map_err(engine_error_to_py)?;
            Python::attach(|py| {
                Py::new(py, PyRadarField { backing: RadarFieldBacking::Owned(Arc::new(field)) })
            })
        })
    }

    fn replay_raw_manifest<'py>(
        &self,
        py: Python<'py>,
        manifest_path: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let field = engine
                .replay_raw_manifest(PathBuf::from(manifest_path))
                .await
                .map_err(engine_error_to_py)?;
            Python::attach(|py| {
                Py::new(py, PyRadarField { backing: RadarFieldBacking::Owned(Arc::new(field)) })
            })
        })
    }

    fn regrid<'py>(
        &self,
        py: Python<'py>,
        field: Py<PyRadarField>,
        target_grid_json: &str,
        method: &str,
    ) -> PyResult<Bound<'py, PyAny>> {
        let field = Python::attach(|py| field.borrow(py).value().clone());
        let target: Grid = parse_json(target_grid_json, "target grid")?;
        let method = Resampling::parse(method)
            .ok_or_else(|| PyValueError::new_err("resampling must be nearest or bilinear"))?;
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let field = engine.regrid(field, target, method).await.map_err(engine_error_to_py)?;
            Python::attach(|py| {
                Py::new(py, PyRadarField { backing: RadarFieldBacking::Owned(Arc::new(field)) })
            })
        })
    }

    #[pyo3(signature = (frame, field, format, overwrite, options_json, variable=None, output_root=None))]
    fn write_science<'py>(
        &self,
        py: Python<'py>,
        frame: Py<PyFrameRef>,
        field: Py<PyRadarField>,
        format: &str,
        overwrite: bool,
        options_json: &str,
        variable: Option<String>,
        output_root: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let frame = Python::attach(|py| frame.borrow(py).inner.clone());
        let field = Python::attach(|py| field.borrow(py).value().clone());
        let options: Value = parse_json(options_json, "writer options")?;
        let format = format.to_owned();
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let output_root = output_root
                .map(PathBuf::from)
                .unwrap_or_else(|| engine.config().storage.output.clone());
            let report = engine
                .write_science_to(frame, field, output_root, &format, overwrite, options, variable)
                .await
                .map_err(engine_error_to_py)?;
            Python::attach(|py| Py::new(py, PyDownloadBatchReport::new(report)))
        })
    }

    #[pyo3(signature = (frames, on_error, dry_run, max_concurrency=None))]
    fn fetch_many_raw<'py>(
        &self,
        py: Python<'py>,
        frames: Vec<Py<PyFrameRef>>,
        on_error: &str,
        dry_run: bool,
        max_concurrency: Option<usize>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let policy = FetchErrorPolicy::parse(on_error).ok_or_else(|| {
            PyValueError::new_err("on_error must be collect/continue or stop/raise")
        })?;
        let frames = Python::attach(|py| {
            frames.into_iter().map(|frame| frame.borrow(py).inner.clone()).collect::<Vec<_>>()
        });
        if max_concurrency == Some(0) {
            return Err(PyValueError::new_err("max_concurrency must be positive"));
        }
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let report = if let Some(concurrency) = max_concurrency {
                engine.fetch_many_raw_with_concurrency(frames, policy, dry_run, concurrency).await
            } else {
                engine.fetch_many_raw(frames, policy, dry_run).await
            };
            let report = PyFetchBatchReport::from_report(report);
            Python::attach(|py| Py::new(py, report))
        })
    }

    #[pyo3(signature = (frames, on_error, dry_run, max_concurrency=None))]
    fn fetch_many_decoded<'py>(
        &self,
        py: Python<'py>,
        frames: Vec<Py<PyFrameRef>>,
        on_error: &str,
        dry_run: bool,
        max_concurrency: Option<usize>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let policy = FetchErrorPolicy::parse(on_error).ok_or_else(|| {
            PyValueError::new_err("on_error must be collect/continue or stop/raise")
        })?;
        let frames = Python::attach(|py| {
            frames.into_iter().map(|frame| frame.borrow(py).inner.clone()).collect::<Vec<_>>()
        });
        if max_concurrency == Some(0) {
            return Err(PyValueError::new_err("max_concurrency must be positive"));
        }
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let report = if let Some(concurrency) = max_concurrency {
                engine
                    .fetch_many_decoded_with_concurrency(frames, policy, dry_run, concurrency)
                    .await
            } else {
                engine.fetch_many_decoded(frames, policy, dry_run).await
            };
            let report = PyFetchBatchReport::from_decoded_report(report);
            Python::attach(|py| Py::new(py, report))
        })
    }

    #[pyo3(signature = (frames, on_error, max_concurrency=None))]
    fn open_fetch_stream(
        &self,
        frames: Vec<Py<PyFrameRef>>,
        on_error: &str,
        max_concurrency: Option<usize>,
    ) -> PyResult<PyFetchStream> {
        let policy = FetchErrorPolicy::parse(on_error).ok_or_else(|| {
            PyValueError::new_err("on_error must be collect/continue or stop/raise")
        })?;
        if max_concurrency == Some(0) {
            return Err(PyValueError::new_err("max_concurrency must be positive"));
        }
        let frames = Python::attach(|py| {
            frames.into_iter().map(|frame| frame.borrow(py).inner.clone()).collect::<Vec<_>>()
        });
        let concurrency = max_concurrency.unwrap_or(self.inner.config().runtime.frame_concurrency);
        let stream = self.inner.fetch_decoded_stream(frames, policy, concurrency);
        let cancellation = stream.cancellation_token();
        Ok(PyFetchStream { inner: Arc::new(tokio::sync::Mutex::new(stream)), cancellation })
    }

    #[pyo3(signature = (frames, on_error, dry_run, overwrite, output_root=None))]
    fn download_raw_only<'py>(
        &self,
        py: Python<'py>,
        frames: Vec<Py<PyFrameRef>>,
        on_error: &str,
        dry_run: bool,
        overwrite: bool,
        output_root: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let policy = FetchErrorPolicy::parse(on_error).ok_or_else(|| {
            PyValueError::new_err("on_error must be collect/continue or stop/raise")
        })?;
        let frames = Python::attach(|py| {
            frames.into_iter().map(|frame| frame.borrow(py).inner.clone()).collect::<Vec<_>>()
        });
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let report = if let Some(output_root) = output_root {
                engine
                    .download_raw_only_to(
                        frames,
                        policy,
                        dry_run,
                        overwrite,
                        PathBuf::from(output_root),
                    )
                    .await
            } else {
                engine.download_raw_only(frames, policy, dry_run, overwrite).await
            };
            Python::attach(|py| Py::new(py, PyDownloadBatchReport::new(report)))
        })
    }

    #[pyo3(signature = (frames, on_error, dry_run, overwrite, output_root=None, output_template=None, include_raw=false, processing_json=None))]
    fn download_png<'py>(
        &self,
        py: Python<'py>,
        frames: Vec<Py<PyFrameRef>>,
        on_error: &str,
        dry_run: bool,
        overwrite: bool,
        output_root: Option<String>,
        output_template: Option<String>,
        include_raw: bool,
        processing_json: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let policy = FetchErrorPolicy::parse(on_error).ok_or_else(|| {
            PyValueError::new_err("on_error must be collect/continue or stop/raise")
        })?;
        let frames = Python::attach(|py| {
            frames.into_iter().map(|frame| frame.borrow(py).inner.clone()).collect::<Vec<_>>()
        });
        let processing = parse_decoded_processing(processing_json.as_deref())?;
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let output_root = output_root
                .map(PathBuf::from)
                .unwrap_or_else(|| engine.config().storage.output.clone());
            let report = engine
                .download_decoded_to_with_processing_and_raw(
                    frames,
                    policy,
                    dry_run,
                    overwrite,
                    output_root,
                    "png",
                    output_template,
                    processing,
                    include_raw,
                )
                .await
                .map_err(engine_error_to_py)?;
            Python::attach(|py| Py::new(py, PyDownloadBatchReport::new(report)))
        })
    }

    #[pyo3(signature = (frames, on_error, dry_run, overwrite, output_root=None, output_template=None, include_raw=false, processing_json=None))]
    fn download_netcdf<'py>(
        &self,
        py: Python<'py>,
        frames: Vec<Py<PyFrameRef>>,
        on_error: &str,
        dry_run: bool,
        overwrite: bool,
        output_root: Option<String>,
        output_template: Option<String>,
        include_raw: bool,
        processing_json: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let policy = FetchErrorPolicy::parse(on_error).ok_or_else(|| {
            PyValueError::new_err("on_error must be collect/continue or stop/raise")
        })?;
        let frames = Python::attach(|py| {
            frames.into_iter().map(|frame| frame.borrow(py).inner.clone()).collect::<Vec<_>>()
        });
        let processing = parse_decoded_processing(processing_json.as_deref())?;
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let output_root = output_root
                .map(PathBuf::from)
                .unwrap_or_else(|| engine.config().storage.output.clone());
            let report = engine
                .download_decoded_to_with_processing_and_raw(
                    frames,
                    policy,
                    dry_run,
                    overwrite,
                    output_root,
                    "netcdf",
                    output_template,
                    processing,
                    include_raw,
                )
                .await
                .map_err(engine_error_to_py)?;
            Python::attach(|py| Py::new(py, PyDownloadBatchReport::new(report)))
        })
    }

    #[pyo3(signature = (frames, on_error, dry_run, overwrite, output_root=None, output_template=None, include_raw=false, processing_json=None))]
    fn download_geotiff<'py>(
        &self,
        py: Python<'py>,
        frames: Vec<Py<PyFrameRef>>,
        on_error: &str,
        dry_run: bool,
        overwrite: bool,
        output_root: Option<String>,
        output_template: Option<String>,
        include_raw: bool,
        processing_json: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let policy = FetchErrorPolicy::parse(on_error).ok_or_else(|| {
            PyValueError::new_err("on_error must be collect/continue or stop/raise")
        })?;
        let frames = Python::attach(|py| {
            frames.into_iter().map(|frame| frame.borrow(py).inner.clone()).collect::<Vec<_>>()
        });
        let processing = parse_decoded_processing(processing_json.as_deref())?;
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let output_root = output_root
                .map(PathBuf::from)
                .unwrap_or_else(|| engine.config().storage.output.clone());
            let report = engine
                .download_decoded_to_with_processing_and_raw(
                    frames,
                    policy,
                    dry_run,
                    overwrite,
                    output_root,
                    "geotiff",
                    output_template,
                    processing,
                    include_raw,
                )
                .await
                .map_err(engine_error_to_py)?;
            Python::attach(|py| Py::new(py, PyDownloadBatchReport::new(report)))
        })
    }

    #[pyo3(signature = (frames, on_error, dry_run, overwrite, output_root=None, output_template=None, include_raw=false, processing_json=None))]
    fn download_zarr<'py>(
        &self,
        py: Python<'py>,
        frames: Vec<Py<PyFrameRef>>,
        on_error: &str,
        dry_run: bool,
        overwrite: bool,
        output_root: Option<String>,
        output_template: Option<String>,
        include_raw: bool,
        processing_json: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let policy = FetchErrorPolicy::parse(on_error).ok_or_else(|| {
            PyValueError::new_err("on_error must be collect/continue or stop/raise")
        })?;
        let frames = Python::attach(|py| {
            frames.into_iter().map(|frame| frame.borrow(py).inner.clone()).collect::<Vec<_>>()
        });
        let processing = parse_decoded_processing(processing_json.as_deref())?;
        let engine = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let output_root = output_root
                .map(PathBuf::from)
                .unwrap_or_else(|| engine.config().storage.output.clone());
            let report = engine
                .download_decoded_to_with_processing_and_raw(
                    frames,
                    policy,
                    dry_run,
                    overwrite,
                    output_root,
                    "zarr",
                    output_template,
                    processing,
                    include_raw,
                )
                .await
                .map_err(engine_error_to_py)?;
            Python::attach(|py| Py::new(py, PyDownloadBatchReport::new(report)))
        })
    }
}

fn engine_error_to_py(error: crate::engine::EngineError) -> PyErr {
    let message = error.to_string();
    match error {
        crate::engine::EngineError::Core(error) => super::python::core_error_to_py(error),
        crate::engine::EngineError::InvalidFrame
        | crate::engine::EngineError::InvalidQuery(_)
        | crate::engine::EngineError::InvalidConfiguration
        | crate::engine::EngineError::UnsupportedSource(_)
        | crate::engine::EngineError::UnsupportedScience(_) => PyValueError::new_err(message),
        _ => PyOSError::new_err(message),
    }
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyResolvedConfig>()?;
    m.add_function(wrap_pyfunction!(resolve_config, m)?)?;
    m.add_function(wrap_pyfunction!(redact_config_values_json, m)?)?;
    m.add_class::<PyQuery>()?;
    m.add_class::<PyFrameRef>()?;
    m.add_class::<PyRadarField>()?;
    m.add_class::<PyRadarDataset>()?;
    m.add_class::<PyDiscoveryReport>()?;
    m.add_class::<PyRawFrame>()?;
    m.add_class::<PyFetchItem>()?;
    m.add_class::<PyFetchBatchReport>()?;
    m.add_class::<PyFetchStream>()?;
    m.add_class::<PyDownloadItem>()?;
    m.add_class::<PyDownloadBatchReport>()?;
    m.add_class::<PyOperationEvents>()?;
    m.add_class::<PyEngine>()?;
    Ok(())
}
