//! Stable cross-language identity rules for frames, revisions, processing and outputs.

use crate::model::{FrameRef, RadarField};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub const IDENTITY_SCHEMA: u8 = 1;
const VOLATILE_LOCATOR_KEYS: &[&str] = &[
    "url",
    "uri",
    "signed_url",
    "signature",
    "sig",
    "token",
    "expires",
    "headers",
    "cookies",
    "artifact_host",
    "artifact_path",
];

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProcessingSpec {
    #[serde(default = "default_output_kind")]
    pub output_kind: String,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub variable: Option<String>,
    #[serde(default = "default_grid")]
    pub grid: String,
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
    #[serde(default)]
    pub resolution: Option<f64>,
    #[serde(default = "default_resampling")]
    pub resampling: String,
    #[serde(default = "default_version")]
    pub decoder_version: String,
    #[serde(default = "default_version")]
    pub resource_version: String,
    #[serde(default = "default_version")]
    pub encoder_version: String,
    #[serde(default)]
    pub options: Value,
}

impl Default for ProcessingSpec {
    fn default() -> Self {
        Self {
            output_kind: default_output_kind(),
            format: default_format(),
            variable: None,
            grid: default_grid(),
            bbox: None,
            resolution: None,
            resampling: default_resampling(),
            decoder_version: default_version(),
            resource_version: default_version(),
            encoder_version: default_version(),
            options: Value::Object(Map::new()),
        }
    }
}

fn default_output_kind() -> String {
    "decoded".into()
}
fn default_format() -> String {
    "netcdf".into()
}
fn default_grid() -> String {
    "native".into()
}
fn default_resampling() -> String {
    "nearest".into()
}
fn default_version() -> String {
    "1".into()
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("identity timestamp must be ISO-8601 with a timezone")]
    InvalidTimestamp,
    #[error("identity value could not be serialized as canonical JSON")]
    InvalidJson,
}

#[derive(Clone, Copy)]
pub struct ArtifactBytes<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
    pub source_revision: Option<&'a str>,
}

pub fn normalize_time(value: &str) -> Result<String, IdentityError> {
    let parsed =
        DateTime::parse_from_rfc3339(value).map_err(|_| IdentityError::InvalidTimestamp)?;
    Ok(parsed.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::Micros, true))
}

pub fn canonical_json(value: &Value) -> Result<String, IdentityError> {
    fn append(value: &Value, output: &mut String) -> Result<(), IdentityError> {
        match value {
            Value::Null => output.push_str("null"),
            Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
            Value::Number(value) => {
                output.push_str(
                    &serde_json::to_string(value).map_err(|_| IdentityError::InvalidJson)?,
                );
            }
            Value::String(value) => {
                output.push_str(
                    &serde_json::to_string(value).map_err(|_| IdentityError::InvalidJson)?,
                );
            }
            Value::Array(values) => {
                output.push('[');
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    append(value, output)?;
                }
                output.push(']');
            }
            Value::Object(values) => {
                output.push('{');
                let mut entries = values.iter().collect::<Vec<_>>();
                entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
                for (index, (key, value)) in entries.into_iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    output.push_str(
                        &serde_json::to_string(key).map_err(|_| IdentityError::InvalidJson)?,
                    );
                    output.push(':');
                    append(value, output)?;
                }
                output.push('}');
            }
        }
        Ok(())
    }

    let mut canonical = String::new();
    append(value, &mut canonical)?;
    Ok(canonical)
}

pub fn digest(value: &Value) -> Result<String, IdentityError> {
    let canonical = canonical_json(value)?;
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    Ok(hex::encode(hasher.finalize()))
}

pub fn safe_locator(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut result = Map::new();
            for (key, value) in object {
                if VOLATILE_LOCATOR_KEYS.iter().any(|blocked| key.eq_ignore_ascii_case(blocked)) {
                    continue;
                }
                result.insert(key.clone(), safe_locator(value));
            }
            Value::Object(result)
        }
        Value::Array(values) => Value::Array(values.iter().map(safe_locator).collect()),
        value => value.clone(),
    }
}

pub fn frame_identity(frame: &FrameRef) -> Result<Value, IdentityError> {
    let legacy_tw_locator = legacy_tw_identity_locator(frame);
    let locator_version =
        if legacy_tw_locator.is_some() { "tw-legacy-v1" } else { frame.locator_version.as_str() };
    let locator = legacy_tw_locator.as_ref().unwrap_or(&frame.locator);
    Ok(json!({
        "identity_schema": IDENTITY_SCHEMA,
        "source": frame.source,
        "product": frame.product,
        "station": frame.station,
        "valid_time": normalize_time(&frame.valid_time)?,
        "base_time": frame.base_time.as_deref().map(normalize_time).transpose()?,
        "locator_version": locator_version,
        "locator": safe_locator(locator),
    }))
}

fn legacy_tw_identity_locator(frame: &FrameRef) -> Option<Value> {
    if frame.source != "tw"
        || !matches!(frame.locator_version.as_str(), "tw-legacy-v1" | "tw-cwa-v2")
        || !matches!(frame.product.as_str(), "grid" | "observation")
    {
        return None;
    }

    let mut locator = Map::new();
    locator.insert("artifacts".into(), json!([]));
    for key in ["station", "revision"] {
        if let Some(value) = frame.locator.get(key) {
            locator.insert(key.into(), value.clone());
        }
    }
    if frame.product == "observation" {
        let metadata_url = frame
            .locator
            .get("metadata_url")
            .or_else(|| frame.locator.get("artifacts")?.as_array()?.first()?.get("url"));
        if let Some(value) = metadata_url {
            locator.insert("metadata_url".into(), value.clone());
        }
    }
    Some(Value::Object(locator))
}

pub fn logical_id(frame: &FrameRef) -> Result<String, IdentityError> {
    digest(&frame_identity(frame)?)
}

pub fn artifact_receipt(artifact: ArtifactBytes<'_>) -> Value {
    let mut hasher = Sha256::new();
    hasher.update(artifact.bytes);
    json!({
        "name": artifact.name,
        "size_bytes": artifact.bytes.len(),
        "sha256": hex::encode(hasher.finalize()),
        "source_revision": artifact.source_revision,
    })
}

pub fn resolved_revision(
    artifacts: &[ArtifactBytes<'_>],
    upstream_revision: Option<&str>,
) -> Result<String, IdentityError> {
    if let Some(revision) = upstream_revision.filter(|revision| !revision.is_empty()) {
        return digest(&json!({"namespace": "upstream", "revision": revision}));
    }
    let mut receipts = artifacts.iter().copied().map(artifact_receipt).collect::<Vec<_>>();
    receipts.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    digest(&json!({"identity_schema": IDENTITY_SCHEMA, "artifacts": receipts}))
}

pub fn processing_identity(spec: &ProcessingSpec) -> Value {
    json!({
        "identity_schema": IDENTITY_SCHEMA,
        "output_kind": spec.output_kind,
        "format": spec.format,
        "variable": spec.variable,
        "grid": spec.grid,
        "bbox": spec.bbox,
        "resolution": spec.resolution,
        "resampling": spec.resampling,
        "decoder_version": spec.decoder_version,
        "resource_version": spec.resource_version,
        "encoder_version": spec.encoder_version,
        "options": spec.options,
    })
}

pub(crate) fn apply_science_versions(spec: &mut ProcessingSpec, field: &RadarField) {
    if field.provenance.iter().any(|entry| entry == "source=rdcap") {
        spec.decoder_version = "rdcap-csr-v1".into();
        spec.resource_version = "rdcap-reflectivity-v1+rdcap-annotation-v1".into();
    }
}

pub fn processing_hash(spec: &ProcessingSpec) -> Result<String, IdentityError> {
    digest(&processing_identity(spec))
}

pub fn output_id(
    frame: &FrameRef,
    revision: &str,
    spec: &ProcessingSpec,
) -> Result<String, IdentityError> {
    digest(&json!({
        "identity_schema": IDENTITY_SCHEMA,
        "logical_id": logical_id(frame)?,
        "resolved_revision": revision,
        "processing_hash": processing_hash(spec)?,
    }))
}

pub fn cache_key(
    frame: &FrameRef,
    revision: Option<&str>,
    acquisition_version: &str,
    mosaic_version: &str,
) -> Result<String, IdentityError> {
    digest(&json!({
        "logical_id": logical_id(frame)?,
        "revision": revision,
        "acquisition_version": acquisition_version,
        "mosaic_version": mosaic_version,
    }))
}

pub fn variant_id(output: &str) -> &str {
    &output[..output.len().min(12)]
}

pub fn safe_ref(frame: &FrameRef) -> Result<Value, IdentityError> {
    Ok(json!({
        "source": frame.source,
        "product": frame.product,
        "station": frame.station,
        "valid_time": normalize_time(&frame.valid_time)?,
        "base_time": frame.base_time.as_deref().map(normalize_time).transpose()?,
        "locator": safe_locator(&frame.locator),
        "locator_version": frame.locator_version,
        "revision": frame.revision,
        "logical_id": logical_id(frame)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(time: &str, locator: Value) -> FrameRef {
        FrameRef {
            source: "my".into(),
            product: "composite".into(),
            station: Some("east".into()),
            valid_time: time.into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "1".into(),
            locator,
        }
    }

    #[test]
    fn normalizes_utc_and_excludes_volatile_locator_values() {
        let first = frame(
            "2025-01-01T00:00:00.000000Z",
            json!({"path": {"name": "a"}, "signature": "one", "url": "https://a.invalid/?token=x"}),
        );
        let second = frame(
            "2025-01-01T08:00:00+08:00",
            json!({"path": {"name": "a"}, "signature": "two", "url": "https://b.invalid/?token=y"}),
        );
        assert_eq!(logical_id(&first).unwrap(), logical_id(&second).unwrap());
        assert_eq!(normalize_time(&second.valid_time).unwrap(), "2025-01-01T00:00:00.000000Z");
    }

    #[test]
    fn tw_native_locator_preserves_the_legacy_python_logical_identity() {
        let legacy = FrameRef {
            source: "tw".into(),
            product: "grid".into(),
            station: Some("CV1_3600".into()),
            valid_time: "2026-09-20T04:30:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("O-A0059-001-1789878600".into()),
            locator_version: "tw-legacy-v1".into(),
            locator: json!({
                "url": "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json",
                "artifacts": [],
                "station": "CV1_3600",
                "revision": "O-A0059-001-1789878600",
            }),
        };
        let native = FrameRef {
            locator_version: "tw-cwa-v2".into(),
            locator: json!({
                "url": "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json",
                "name": "O-A0059-001.json",
                "media_type": "application/json",
                "artifacts": [],
                "station": "CV1_3600",
                "revision": "O-A0059-001-1789878600",
                "time_semantics": "provider_grid_time",
                "geometry_status": "provider_native_twd67",
                "native_crs": "EPSG:3821",
                "grid_dimension": [881, 921],
                "grid_origin": [115.0, 18.0],
                "grid_resolution": 0.0125,
            }),
            ..legacy.clone()
        };

        assert_eq!(logical_id(&native).unwrap(), logical_id(&legacy).unwrap());
        assert_eq!(
            frame_identity(&native).unwrap()["locator"],
            frame_identity(&legacy).unwrap()["locator"]
        );
    }

    #[test]
    fn artifact_revision_is_order_independent_and_processing_is_versioned() {
        let a = ArtifactBytes { name: "a.bin", bytes: b"a", source_revision: None };
        let b = ArtifactBytes { name: "b.bin", bytes: b"b", source_revision: None };
        assert_eq!(
            resolved_revision(&[a, b], None).unwrap(),
            resolved_revision(&[b, a], None).unwrap()
        );
        let frame = frame("2025-01-01T00:00:00Z", Value::Null);
        let decoded = ProcessingSpec::default();
        let raw = ProcessingSpec { output_kind: "raw-only".into(), ..ProcessingSpec::default() };
        assert_ne!(processing_hash(&decoded).unwrap(), processing_hash(&raw).unwrap());
        assert_ne!(
            output_id(&frame, "revision", &decoded).unwrap(),
            output_id(&frame, "revision", &raw).unwrap()
        );
    }

    #[test]
    fn canonical_json_rejects_timestamp_without_timezone() {
        assert_eq!(normalize_time("2025-01-01T00:00:00"), Err(IdentityError::InvalidTimestamp));
        assert_eq!(canonical_json(&json!({"b": 2, "a": 1})).unwrap(), r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn matches_the_python_identity_golden() {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/rust-migration/output/identity.json"
        ))
        .unwrap();
        let frame = FrameRef {
            source: "my".into(),
            product: "composite".into(),
            station: Some("east".into()),
            valid_time: "2025-01-01T08:00:00+08:00".into(),
            base_time: Some("2025-01-01T07:55:00+08:00".into()),
            logical_id: String::new(),
            revision: Some("upstream-42".into()),
            locator_version: "2".into(),
            locator: json!({
                "path": {"name": "radar/雪.png", "sequence": 7},
                "url": "https://example.invalid/image?token=volatile",
                "auth": {"signature": "volatile", "scope": "image"},
            }),
        };
        let artifacts = [
            ArtifactBytes {
                name: "image.png",
                bytes: b"sample-image-bytes",
                source_revision: Some("upstream-42"),
            },
            ArtifactBytes {
                name: "metadata.json",
                bytes: b"{\"version\":1}",
                source_revision: Some("upstream-42"),
            },
        ];
        let spec = ProcessingSpec {
            options: json!({"palette": "rainbow", "levels": [0, 10, 20]}),
            ..ProcessingSpec::default()
        };
        let receipts: Vec<Value> = artifacts.iter().copied().map(artifact_receipt).collect();
        let revision = resolved_revision(&artifacts, None).unwrap();

        assert_eq!(frame_identity(&frame).unwrap(), golden["frame_identity"]);
        assert_eq!(logical_id(&frame).unwrap(), golden["logical_id"]);
        assert_eq!(safe_ref(&frame).unwrap(), golden["safe_ref"]);
        assert_eq!(Value::Array(receipts), golden["artifact_receipts"]);
        assert_eq!(revision, golden["resolved_revision_from_artifacts"]);
        assert_eq!(
            resolved_revision(&artifacts, Some("upstream-42")).unwrap(),
            golden["resolved_revision_upstream"]
        );
        assert_eq!(processing_identity(&spec), golden["processing_identity"]);
        assert_eq!(processing_hash(&spec).unwrap(), golden["processing_hash"]);
        assert_eq!(output_id(&frame, &revision, &spec).unwrap(), golden["output_id"]);
        assert_eq!(cache_key(&frame, Some(&revision), "1", "1").unwrap(), golden["cache_key"]);
    }
}
