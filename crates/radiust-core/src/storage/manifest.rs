//! Versioned local output manifests and artifact verification.

use crate::errors::{CoreError, CoreResult};
use crate::identity::digest;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ManifestArtifact {
    pub name: String,
    pub relative_uri: String,
    pub role: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Manifest {
    pub schema_version: u8,
    pub logical_id: String,
    pub revision: String,
    pub output_id: String,
    pub processing_spec: Value,
    pub processing_hash: String,
    pub artifacts: Vec<ManifestArtifact>,
    pub raw_complete: bool,
    pub created_at: String,
    pub generation: Option<String>,
    pub supersedes: Option<Value>,
}

impl Manifest {
    pub fn validate(&self) -> CoreResult<()> {
        if self.schema_version != 1 {
            return Err(CoreError::Storage("unsupported manifest schema version".into()));
        }
        for (name, value) in [
            ("logical_id", self.logical_id.as_str()),
            ("revision", self.revision.as_str()),
            ("output_id", self.output_id.as_str()),
            ("processing_hash", self.processing_hash.as_str()),
        ] {
            if !is_sha256(value) {
                return Err(CoreError::Storage(format!(
                    "manifest {name} must be a lowercase SHA-256 digest"
                )));
            }
        }
        if !self.processing_spec.is_object() {
            return Err(CoreError::Storage("manifest processing_spec must be an object".into()));
        }
        if self.supersedes.as_ref().is_some_and(|value| !value.is_object()) {
            return Err(CoreError::Storage("manifest supersedes must be an object".into()));
        }
        if digest(&self.processing_spec).map_err(|error| CoreError::Storage(error.to_string()))?
            != self.processing_hash
        {
            return Err(CoreError::Storage("manifest processing hash does not match".into()));
        }
        DateTime::parse_from_rfc3339(&self.created_at)
            .map_err(|_| CoreError::Storage("manifest created_at must be ISO-8601".into()))?;
        if self.artifacts.is_empty() {
            return Err(CoreError::Storage("manifest must contain artifacts".into()));
        }
        if self.generation.as_ref().is_some_and(String::is_empty) {
            return Err(CoreError::Storage("manifest generation cannot be empty".into()));
        }
        let mut names = HashSet::with_capacity(self.artifacts.len());
        let mut relative_uris = HashSet::with_capacity(self.artifacts.len());
        for artifact in &self.artifacts {
            if !is_safe_relative_path(&artifact.name)
                || !is_safe_relative_path(&artifact.relative_uri)
                || !is_sha256(&artifact.sha256)
                || artifact.role.is_empty()
                || artifact.media_type.is_empty()
            {
                return Err(CoreError::Storage(
                    "manifest contains an invalid artifact receipt".into(),
                ));
            }
            if !names.insert(&artifact.name) || !relative_uris.insert(&artifact.relative_uri) {
                return Err(CoreError::Storage("manifest artifact paths must be unique".into()));
            }
        }
        let paths = self
            .artifacts
            .iter()
            .map(|artifact| artifact.relative_uri.as_str())
            .collect::<Vec<_>>();
        for path in &paths {
            if paths.iter().any(|other| {
                path != other
                    && other.strip_prefix(path).is_some_and(|suffix| suffix.starts_with('/'))
            }) {
                return Err(CoreError::Storage("manifest artifact paths overlap".into()));
            }
        }
        Ok(())
    }

    pub fn bytes(&self) -> CoreResult<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec_pretty(self)
            .map_err(|_| CoreError::Storage("manifest could not be serialized".into()))
    }
}

pub struct ManifestFields {
    pub logical_id: String,
    pub revision: String,
    pub output_id: String,
    pub processing_spec: Value,
    pub processing_hash: String,
    pub artifacts: Vec<ManifestArtifact>,
    pub raw_complete: bool,
    pub generation: String,
    pub supersedes: Option<Value>,
}

pub fn new_manifest(fields: ManifestFields) -> CoreResult<Manifest> {
    let manifest = Manifest {
        schema_version: 1,
        logical_id: fields.logical_id,
        revision: fields.revision,
        output_id: fields.output_id,
        processing_spec: fields.processing_spec,
        processing_hash: fields.processing_hash,
        artifacts: fields.artifacts,
        raw_complete: fields.raw_complete,
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
        generation: Some(fields.generation),
        supersedes: fields.supersedes,
    };
    manifest.validate()?;
    Ok(manifest)
}

pub fn read_manifest(path: &Path) -> CoreResult<Option<Manifest>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!("manifest could not be inspected: {error}")));
        }
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CoreError::Storage(format!("manifest could not be read: {error}")));
        }
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
        return Ok(None);
    };
    if manifest.validate().is_err() {
        return Ok(None);
    }
    Ok(Some(manifest))
}

pub fn is_complete(root: &Path, manifest: &Manifest) -> bool {
    manifest.artifacts.iter().all(|artifact| {
        let Ok(path) = safe_artifact_path(root, &artifact.relative_uri) else { return false };
        if has_symlink_or_invalid_parent(root, &artifact.relative_uri) {
            return false;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else { return false };
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() != artifact.size_bytes
        {
            return false;
        }
        let Ok((size, digest)) = hash_file(&path, u64::MAX) else { return false };
        size == artifact.size_bytes && digest == artifact.sha256
    })
}

pub fn hash_file(path: &Path, max_bytes: u64) -> CoreResult<(u64, String)> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| CoreError::Storage(format!("artifact could not be inspected: {error}")))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CoreError::Storage("artifact must be a regular file".into()));
    }
    let mut file = File::open(path)
        .map_err(|error| CoreError::Storage(format!("artifact could not be opened: {error}")))?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| CoreError::Storage(format!("artifact could not be read: {error}")))?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| CoreError::ResourceLimit("artifact size overflow".into()))?;
        if size > max_bytes {
            return Err(CoreError::ResourceLimit(format!("artifact exceeds {max_bytes} bytes")));
        }
        hasher.update(&buffer[..count]);
    }
    Ok((size, hex::encode(hasher.finalize())))
}

pub fn safe_artifact_path(root: &Path, relative: &str) -> CoreResult<PathBuf> {
    if !is_safe_relative_path(relative) {
        return Err(CoreError::Storage("artifact path must stay inside the output root".into()));
    }
    Ok(root.join(relative))
}

pub fn is_safe_relative_path(value: &str) -> bool {
    if value.is_empty()
        || value.contains('\\')
        || value.contains('\0')
        || value.chars().any(char::is_control)
        || value.starts_with('/')
        || value.ends_with('/')
    {
        return false;
    }
    let path = Path::new(value);
    !path.is_absolute()
        && value.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
        && path.components().all(|component| matches!(component, Component::Normal(_)))
}

pub(crate) fn has_symlink_or_invalid_parent(root: &Path, relative: &str) -> bool {
    let mut current = root.to_path_buf();
    let components = relative.split('/').collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return true;
                }
                if index + 1 < components.len() && !metadata.is_dir() {
                    return true;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
            Err(_) => return true,
        }
    }
    false
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
