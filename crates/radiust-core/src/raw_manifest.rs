//! Safe loading of locally committed raw frames for offline replay.

use crate::errors::{CoreError, CoreResult};
use crate::limits::Limits;
use crate::model::{ArtifactReceipt, FrameRef, RawArtifact, RawFrame};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const COPY_CHUNK_BYTES: usize = 64 * 1024;

pub fn load(path: &Path, temp_root: &Path, limits: &Limits) -> CoreResult<RawFrame> {
    if path.file_name().and_then(|name| name.to_str()) != Some("raw-manifest.json") {
        return Err(invalid_manifest());
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| invalid_manifest())?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_MANIFEST_BYTES
    {
        return Err(invalid_manifest());
    }
    let manifest_path = path.canonicalize().map_err(|_| invalid_manifest())?;
    let root = manifest_path.parent().ok_or_else(invalid_manifest)?;
    let bytes = fs::read(&manifest_path).map_err(|_| invalid_manifest())?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_manifest())?;
    if document.get("schema_version").and_then(Value::as_u64) != Some(1)
        || document.get("raw_complete").and_then(Value::as_bool) != Some(true)
    {
        return Err(invalid_manifest());
    }
    let mut frame: FrameRef =
        serde_json::from_value(document.get("ref").cloned().ok_or_else(invalid_manifest)?)
            .map_err(|_| invalid_manifest())?;
    frame.validate_identity().map_err(|_| invalid_manifest())?;
    restore_static_locator(&mut frame);
    let descriptors = document
        .get("artifacts")
        .and_then(Value::as_array)
        .filter(|artifacts| !artifacts.is_empty())
        .ok_or_else(invalid_manifest)?;
    let payload_root = root.join("raw");
    let payload_metadata = fs::symlink_metadata(&payload_root).map_err(|_| invalid_manifest())?;
    if !payload_metadata.is_dir() || payload_metadata.file_type().is_symlink() {
        return Err(invalid_manifest());
    }
    let payload_root = payload_root.canonicalize().map_err(|_| invalid_manifest())?;
    fs::create_dir_all(temp_root)
        .map_err(|_| CoreError::Temporary("replay staging is unavailable".into()))?;

    let mut names = HashSet::with_capacity(descriptors.len());
    let mut artifacts = Vec::with_capacity(descriptors.len());
    let mut total_bytes = 0_u64;
    for descriptor in descriptors {
        let name = descriptor.get("name").and_then(Value::as_str).ok_or_else(invalid_manifest)?;
        if !safe_name(name) || !names.insert(name) {
            return Err(invalid_manifest());
        }
        let media_type = descriptor
            .get("media_type")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(invalid_manifest)?;
        let expected_size =
            descriptor.get("size_bytes").and_then(Value::as_u64).ok_or_else(invalid_manifest)?;
        let expected_digest = descriptor
            .get("sha256")
            .and_then(Value::as_str)
            .filter(|value| {
                value.len() == 64 && value.bytes().all(|character| character.is_ascii_hexdigit())
            })
            .ok_or_else(invalid_manifest)?;
        let source = payload_root.join(name);
        let metadata = fs::symlink_metadata(&source).map_err(|_| invalid_manifest())?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != expected_size
        {
            return Err(invalid_manifest());
        }
        let source = source.canonicalize().map_err(|_| invalid_manifest())?;
        if source.parent() != Some(payload_root.as_path()) {
            return Err(invalid_manifest());
        }
        let remaining = limits.max_frame_bytes.saturating_sub(total_bytes);
        if expected_size > remaining || expected_size > limits.max_artifact_bytes {
            return Err(CoreError::ResourceLimit(
                "raw replay exceeds configured byte limits".into(),
            ));
        }
        let (staged, copied_size, copied_digest) = copy_verified(&source, temp_root, remaining)?;
        if copied_size != expected_size || copied_digest != expected_digest {
            return Err(invalid_manifest());
        }
        total_bytes = total_bytes
            .checked_add(copied_size)
            .ok_or_else(|| CoreError::ResourceLimit("raw replay size overflow".into()))?;
        if total_bytes > limits.max_temp_bytes {
            return Err(CoreError::ResourceLimit(
                "raw replay exceeds temporary-storage limit".into(),
            ));
        }
        artifacts.push(RawArtifact {
            receipt: ArtifactReceipt {
                name: name.to_owned(),
                media_type: media_type.to_owned(),
                size_bytes: copied_size,
                sha256: copied_digest,
            },
            path: staged,
        });
    }
    Ok(RawFrame { frame, artifacts, private_locator: None })
}

fn copy_verified(
    source: &Path,
    temp_root: &Path,
    max_bytes: u64,
) -> CoreResult<(tempfile::TempPath, u64, String)> {
    let mut input = File::open(source)
        .map_err(|_| CoreError::Storage("raw replay artifact could not be opened".into()))?;
    let mut output = tempfile::Builder::new()
        .prefix("radiust-replay-")
        .tempfile_in(temp_root)
        .map_err(|_| CoreError::Temporary("raw replay staging could not be created".into()))?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; COPY_CHUNK_BYTES];
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|_| CoreError::Storage("raw replay artifact could not be read".into()))?;
        if count == 0 {
            break;
        }
        let count = count as u64;
        if count > max_bytes.saturating_sub(total) {
            return Err(CoreError::ResourceLimit(
                "raw replay exceeds configured byte limits".into(),
            ));
        }
        output
            .write_all(&buffer[..count as usize])
            .map_err(|_| CoreError::Temporary("raw replay staging write failed".into()))?;
        digest.update(&buffer[..count as usize]);
        total += count;
    }
    output
        .as_file()
        .sync_all()
        .map_err(|_| CoreError::Temporary("raw replay staging flush failed".into()))?;
    Ok((output.into_temp_path(), total, hex::encode(digest.finalize())))
}

fn restore_static_locator(frame: &mut FrameRef) {
    // `safe_ref` deliberately strips transport URLs. Restore only this
    // source's fixed public endpoint so its Rust decoder can validate the
    // frame and record provenance without trusting manifest-supplied URLs.
    if frame.source == "tw" && frame.product == "grid" {
        if let Some(locator) = frame.locator.as_object_mut() {
            locator.insert(
                "url".into(),
                Value::String(
                    "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json"
                        .into(),
                ),
            );
        }
    }
}

fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':' | '\0'))
}

fn invalid_manifest() -> CoreError {
    CoreError::Storage("raw manifest or artifact failed integrity validation".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn write_manifest(root: &Path, name: &str, payload: &[u8]) -> PathBuf {
        fs::create_dir_all(root.join("raw")).unwrap();
        fs::write(root.join("raw").join(name), payload).unwrap();
        let mut frame = FrameRef {
            source: "tw".into(),
            product: "grid".into(),
            station: Some("CV1_3600".into()),
            valid_time: "2026-09-20T04:30:00.000000Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: Some("fixture".into()),
            locator_version: "tw-cwa-v2".into(),
            locator: json!({}),
        };
        frame.logical_id = crate::identity::logical_id(&frame).unwrap();
        let manifest = json!({
            "schema_version": 1,
            "ref": crate::identity::safe_ref(&frame).unwrap(),
            "artifacts": [{
                "name": name,
                "media_type": "application/json",
                "size_bytes": payload.len(),
                "sha256": hex::encode(Sha256::digest(payload)),
            }],
            "raw_complete": true,
        });
        let path = root.join("raw-manifest.json");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        path
    }

    #[test]
    fn load_verifies_hash_and_copies_without_mutating_committed_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let payload = b"retained provider data";
        let manifest = write_manifest(root.path(), "frame.json", payload);
        let raw = load(&manifest, temp.path(), &Limits::default()).unwrap();
        assert_eq!(fs::read(&raw.artifacts[0].path).unwrap(), payload);
        assert_eq!(fs::read(root.path().join("raw/frame.json")).unwrap(), payload);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        drop(raw);
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        assert_eq!(fs::read(root.path().join("raw/frame.json")).unwrap(), payload);
    }

    #[test]
    fn load_rejects_corrupt_bytes_and_symlink_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let payload = b"retained provider data";
        let manifest = write_manifest(root.path(), "frame.json", payload);
        fs::write(root.path().join("raw/frame.json"), b"corrupt").unwrap();
        assert!(load(&manifest, temp.path(), &Limits::default()).is_err());

        let target = tempfile::NamedTempFile::new().unwrap();
        fs::write(target.path(), payload).unwrap();
        let _ = fs::remove_file(root.path().join("raw/frame.json"));
        std::os::unix::fs::symlink(target.path(), root.path().join("raw/frame.json")).unwrap();
        assert!(load(&manifest, temp.path(), &Limits::default()).is_err());
    }
}
