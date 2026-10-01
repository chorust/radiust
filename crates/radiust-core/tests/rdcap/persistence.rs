//! RDCAP raw persistence, binding, cache, and replay contract tests.

use radiust_core::identity::{logical_id, safe_ref};
use radiust_core::limits::Limits;
use radiust_core::model::FrameRef;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const KEY: &str = "1790834708000";
const CONTENT: &[u8] = br#""901,901,T,117.12,19.49,126.12,28.49,int16""#;

#[derive(Serialize)]
struct Binding<'a> {
    schema_version: u8,
    source: &'a str,
    product: &'a str,
    station: &'a str,
    country: &'a str,
    station_code: &'a str,
    key: &'a str,
    valid_time: &'a str,
    logical_id: &'a str,
    content_sha256: &'a str,
    content_size_bytes: u64,
}

fn rdcap_frame() -> FrameRef {
    let mut frame = FrameRef {
        source: "rdcap".into(),
        product: "reflectivity".into(),
        station: Some("TWN/RCHL".into()),
        valid_time: "2026-10-01T06:05:08.000000Z".into(),
        base_time: None,
        logical_id: String::new(),
        revision: Some(hex::encode(Sha256::digest(CONTENT))),
        locator_version: "rdcap-csr-v1".into(),
        locator: json!({"country":"TWN", "station_code":"RCHL", "key":KEY}),
    };
    frame.logical_id = logical_id(&frame).unwrap();
    frame
}

fn write_rdcap_manifest(root: &Path) -> PathBuf {
    let payload_root = root.join("raw");
    fs::create_dir_all(&payload_root).unwrap();
    fs::write(payload_root.join("file-response.json"), CONTENT).unwrap();
    let frame = rdcap_frame();
    let digest = hex::encode(Sha256::digest(CONTENT));
    let binding = Binding {
        schema_version: 1,
        source: "rdcap",
        product: "reflectivity",
        station: "TWN/RCHL",
        country: "TWN",
        station_code: "RCHL",
        key: KEY,
        valid_time: &frame.valid_time,
        logical_id: &frame.logical_id,
        content_sha256: &digest,
        content_size_bytes: CONTENT.len() as u64,
    };
    let binding_bytes = serde_json::to_vec_pretty(&binding).unwrap();
    fs::write(payload_root.join("binding.json"), &binding_bytes).unwrap();
    let document = json!({
        "schema_version": 1,
        "raw_complete": true,
        "ref": safe_ref(&frame).unwrap(),
        "artifacts": [
            {"name":"file-response.json", "media_type":"application/json", "size_bytes":CONTENT.len(), "sha256":digest},
            {"name":"binding.json", "media_type":"application/json", "size_bytes":binding_bytes.len(), "sha256":hex::encode(Sha256::digest(&binding_bytes))},
        ],
    });
    let manifest = root.join("raw-manifest.json");
    fs::write(&manifest, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    manifest
}

#[test]
fn rdcap_raw_manifest_round_trips_exact_envelope_and_never_restores_a_ticket() {
    let root = tempfile::tempdir().unwrap();
    let manifest = write_rdcap_manifest(root.path());
    let staging = root.path().join("staging");
    let raw = radiust_core::raw_manifest::load(&manifest, &staging, &Limits::default()).unwrap();

    assert_eq!(raw.frame.revision.as_deref(), Some(hex::encode(Sha256::digest(CONTENT)).as_str()));
    assert_eq!(raw.artifacts.len(), 2);
    let content =
        raw.artifacts.iter().find(|item| item.receipt.name == "file-response.json").unwrap();
    assert_eq!(fs::read(&content.path).unwrap(), CONTENT);
    let serialized = serde_json::to_string(&raw.public_receipt()).unwrap();
    assert!(!serialized.contains("ft="));
    assert!(!serialized.contains("Referer"));
}

#[test]
fn rdcap_content_revision_changes_without_changing_logical_time_identity() {
    let original = rdcap_frame();
    let mut changed = original.clone();
    changed.revision = Some(hex::encode(Sha256::digest(br#""new valid envelope""#)));

    assert_eq!(original.logical_id, changed.logical_id);
    assert_ne!(original.revision, changed.revision);
}

#[test]
fn rdcap_raw_manifest_rejects_binding_station_time_and_content_mismatches() {
    let root = tempfile::tempdir().unwrap();
    let manifest = write_rdcap_manifest(root.path());
    let mut document: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    let binding_path = root.path().join("raw/binding.json");
    let mut binding: Value = serde_json::from_slice(&fs::read(&binding_path).unwrap()).unwrap();
    binding["station_code"] = json!("SUBI");
    let bytes = serde_json::to_vec_pretty(&binding).unwrap();
    fs::write(&binding_path, &bytes).unwrap();
    document["artifacts"][1]["size_bytes"] = json!(bytes.len());
    document["artifacts"][1]["sha256"] = json!(hex::encode(Sha256::digest(&bytes)));
    fs::write(&manifest, serde_json::to_vec_pretty(&document).unwrap()).unwrap();

    let error = radiust_core::raw_manifest::load(
        &manifest,
        &root.path().join("staging"),
        &Limits::default(),
    )
    .unwrap_err();
    assert!(matches!(error, radiust_core::errors::CoreError::Storage(_)));
}

#[cfg(unix)]
#[test]
fn rdcap_raw_manifest_rejects_symlinked_payloads() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let manifest = write_rdcap_manifest(root.path());
    let payload = root.path().join("raw/file-response.json");
    let retained = root.path().join("raw/retained-response.json");
    fs::rename(&payload, &retained).unwrap();
    symlink(&retained, &payload).unwrap();

    let error = radiust_core::raw_manifest::load(
        &manifest,
        &root.path().join("staging"),
        &Limits::default(),
    )
    .unwrap_err();
    assert!(matches!(error, radiust_core::errors::CoreError::Storage(_)));
}
