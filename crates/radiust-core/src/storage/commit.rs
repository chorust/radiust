//! Transactional local output commits with a manifest-last visibility fence.

use crate::errors::{CoreError, CoreResult};
use crate::identity::{
    ProcessingSpec, logical_id, output_id, processing_hash, processing_identity,
};
use crate::limits::Limits;
use crate::model::FrameRef;
use crate::storage::local::RootLock;
use crate::storage::manifest::{
    Manifest, ManifestArtifact, ManifestFields, hash_file, is_complete, is_safe_relative_path,
    new_manifest, read_manifest,
};
use parking_lot::Mutex;
use serde_json::json;
use sha2::Digest;
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use tokio_util::sync::CancellationToken;

static PROCESS_ROOT_GATES: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

fn process_root_gate(root: &Path) -> Arc<Mutex<()>> {
    let gates = PROCESS_ROOT_GATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut gates = gates.lock();
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(root).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(Mutex::new(()));
    gates.insert(root.to_path_buf(), Arc::downgrade(&gate));
    gate
}

#[derive(Clone, Debug)]
pub struct StagedArtifact {
    pub name: String,
    pub relative_uri: String,
    pub role: String,
    pub media_type: String,
    pub source: PathBuf,
}

#[derive(Clone, Debug)]
pub struct LocalCommitRequest {
    pub frame: FrameRef,
    /// Resolved, content-addressed source revision.
    pub revision: String,
    pub processing_spec: ProcessingSpec,
    /// The stable output path used to derive the adjacent v1 manifest path.
    pub output_name: String,
    pub artifacts: Vec<StagedArtifact>,
    pub raw_complete: bool,
    pub overwrite: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalCommitStatus {
    Written,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct LocalCommitResult {
    pub status: LocalCommitStatus,
    pub manifest: Manifest,
}

#[derive(Clone, Debug)]
pub struct LocalStore {
    root: PathBuf,
    limits: Limits,
    process_gate: Arc<Mutex<()>>,
}

impl LocalStore {
    pub fn new(root: impl AsRef<Path>, limits: Limits) -> CoreResult<Self> {
        fs::create_dir_all(root.as_ref()).map_err(|error| {
            CoreError::Storage(format!("output root could not be created: {error}"))
        })?;
        let root = fs::canonicalize(root.as_ref()).map_err(|error| {
            CoreError::Storage(format!("output root could not be resolved: {error}"))
        })?;
        let process_gate = process_root_gate(&root);
        Ok(Self { root, limits, process_gate })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn commit(&self, request: LocalCommitRequest) -> CoreResult<LocalCommitResult> {
        self.commit_inner(request, None, |_| Ok(()))
    }

    pub fn commit_cancellable(
        &self,
        request: LocalCommitRequest,
        cancellation: &CancellationToken,
    ) -> CoreResult<LocalCommitResult> {
        self.commit_inner(request, Some(cancellation), |_| Ok(()))
    }

    fn commit_inner<F>(
        &self,
        request: LocalCommitRequest,
        cancellation: Option<&CancellationToken>,
        mut hook: F,
    ) -> CoreResult<LocalCommitResult>
    where
        F: FnMut(CommitPhase) -> CoreResult<()>,
    {
        // Queue Rust writers in this process before taking the cross-process
        // lock. The flock remains non-blocking so external writers still fail
        // closed when they own the root.
        let _process_guard = self.process_gate.lock();
        let _lock = RootLock::acquire(&self.root)?;
        validate_request(&request)?;

        let logical =
            logical_id(&request.frame).map_err(|error| CoreError::Storage(error.to_string()))?;
        if !request.frame.logical_id.is_empty() && request.frame.logical_id != logical {
            return Err(CoreError::Storage("frame logical_id does not match its identity".into()));
        }
        let output = output_id(&request.frame, &request.revision, &request.processing_spec)
            .map_err(|error| CoreError::Storage(error.to_string()))?;
        let processing_spec = processing_identity(&request.processing_spec);
        let processing_hash = processing_hash(&request.processing_spec)
            .map_err(|error| CoreError::Storage(error.to_string()))?;

        let manifest_relative = format!("{}.manifest.json", request.output_name);
        if !is_safe_relative_path(&manifest_relative) {
            return Err(CoreError::Storage("manifest path is invalid".into()));
        }
        reject_path_overlap(
            &manifest_relative,
            request.artifacts.iter().map(|item| item.relative_uri.as_str()),
        )?;
        ensure_safe_parent(&self.root, &manifest_relative, false)?;
        for artifact in &request.artifacts {
            ensure_safe_parent(&self.root, &artifact.relative_uri, false)?;
        }

        let manifest_path = self.root.join(&manifest_relative);
        let previous = read_manifest(&manifest_path)?;
        let current = previous.filter(|manifest| is_complete(&self.root, manifest));
        let mut supersedes = None;
        if let Some(existing) = current.as_ref() {
            supersedes = Some(json!({
                "output_id": existing.output_id,
                "generation": existing.generation,
                "manifest": file_name(&manifest_relative),
            }));
            if !request.overwrite && existing.output_id == output {
                if existing.raw_complete || !request.raw_complete {
                    return Ok(LocalCommitResult {
                        status: LocalCommitStatus::Skipped,
                        manifest: existing.clone(),
                    });
                }
            } else if !request.overwrite {
                return Err(CoreError::OutputConflict);
            }
        }

        let stage = tempfile::Builder::new()
            .prefix(".radiust-stage-")
            .tempdir_in(&self.root)
            .map_err(|error| {
                CoreError::Temporary(format!(
                    "output staging directory could not be created: {error}"
                ))
            })?;
        let payload_root = stage.path().join("payload");
        fs::create_dir(&payload_root).map_err(|error| {
            CoreError::Temporary(format!("output staging directory could not be created: {error}"))
        })?;
        let mut total_bytes = 0_u64;
        let mut artifacts = Vec::with_capacity(request.artifacts.len());
        for artifact in &request.artifacts {
            check_cancelled(cancellation)?;
            let staged_path = payload_root.join(&artifact.relative_uri);
            create_parent_dirs(&payload_root, &artifact.relative_uri, None)?;
            let (size_bytes, sha256) = copy_and_hash(
                &artifact.source,
                &staged_path,
                self.limits.max_artifact_bytes,
                self.limits.max_frame_bytes.saturating_sub(total_bytes),
                self.limits.max_temp_bytes.saturating_sub(total_bytes),
                cancellation,
            )?;
            total_bytes = total_bytes
                .checked_add(size_bytes)
                .ok_or_else(|| CoreError::ResourceLimit("output size overflow".into()))?;
            artifacts.push(ManifestArtifact {
                name: artifact.name.clone(),
                relative_uri: artifact.relative_uri.clone(),
                role: artifact.role.clone(),
                media_type: artifact.media_type.clone(),
                size_bytes,
                sha256,
            });
        }

        if let Some(existing) = current.as_ref()
            && existing.output_id == output
            && request.raw_complete
            && !existing.raw_complete
        {
            merge_retained_artifacts(&mut artifacts, &existing.artifacts)?;
        }
        artifacts.sort_by(|left, right| left.relative_uri.cmp(&right.relative_uri));
        validate_manifest_paths(&artifacts)?;
        let manifest = new_manifest(ManifestFields {
            logical_id: logical,
            revision: request.revision.clone(),
            output_id: output.clone(),
            processing_spec,
            processing_hash,
            artifacts,
            raw_complete: request.raw_complete,
            generation: uuid::Uuid::new_v4().simple().to_string(),
            supersedes,
        })?;
        let manifest_stage = stage.path().join("manifest.json");
        let mut staged_manifest =
            OpenOptions::new().write(true).create_new(true).open(&manifest_stage).map_err(
                |error| {
                    CoreError::Temporary(format!("staged manifest could not be created: {error}"))
                },
            )?;
        staged_manifest
            .write_all(&manifest.bytes()?)
            .and_then(|()| staged_manifest.sync_all())
            .map_err(|error| {
                CoreError::Temporary(format!("staged manifest could not be flushed: {error}"))
            })?;
        drop(staged_manifest);
        sync_directory(stage.path())?;
        check_cancelled(cancellation)?;

        let requested_paths = request
            .artifacts
            .iter()
            .map(|artifact| artifact.relative_uri.as_str())
            .collect::<BTreeSet<_>>();
        let mut affected = BTreeSet::new();
        for artifact in &request.artifacts {
            affected.insert(artifact.relative_uri.clone());
        }
        if let Some(existing) = current.as_ref() {
            for artifact in &existing.artifacts {
                let retained = manifest.artifacts.iter().any(|new| {
                    new.relative_uri == artifact.relative_uri
                        && new.name == artifact.name
                        && new.size_bytes == artifact.size_bytes
                        && new.sha256 == artifact.sha256
                        && !requested_paths.contains(artifact.relative_uri.as_str())
                });
                if !retained && !requested_paths.contains(artifact.relative_uri.as_str()) {
                    affected.insert(artifact.relative_uri.clone());
                }
            }
        }
        reject_path_overlaps(affected.iter().map(String::as_str))?;
        let mut affected = affected.into_iter().collect::<Vec<_>>();
        affected.sort_by_key(|path| std::cmp::Reverse(path_depth(path)));
        for relative in &affected {
            ensure_safe_parent(&self.root, relative, false)?;
            validate_existing_target(&self.root.join(relative), false)?;
        }
        validate_existing_target(&manifest_path, true)?;

        let backups_root = stage.path().join("backups");
        fs::create_dir(&backups_root).map_err(|error| {
            CoreError::Temporary(format!("rollback directory could not be created: {error}"))
        })?;
        let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut installed: Vec<PathBuf> = Vec::new();
        let mut created_directories = Vec::new();

        let transaction = (|| -> CoreResult<()> {
            backup_if_present(&manifest_path, &backups_root.join("manifest"), &mut backups)?;
            hook(CommitPhase::ManifestWithdrawn)?;

            for (index, relative) in affected.iter().enumerate() {
                let target = self.root.join(relative);
                let backup = backups_root.join(format!("artifact-{index:08}"));
                backup_if_present(&target, &backup, &mut backups)?;
            }

            let mut publish = request
                .artifacts
                .iter()
                .map(|artifact| {
                    manifest
                        .artifacts
                        .iter()
                        .find(|receipt| receipt.relative_uri == artifact.relative_uri)
                        .ok_or_else(|| {
                            CoreError::Storage("staged artifact has no manifest receipt".into())
                        })
                })
                .collect::<CoreResult<Vec<_>>>()?;
            publish.sort_by(|left, right| left.relative_uri.cmp(&right.relative_uri));
            for artifact in &publish {
                check_cancelled(cancellation)?;
                let target = self.root.join(&artifact.relative_uri);
                create_parent_dirs(
                    &self.root,
                    &artifact.relative_uri,
                    Some(&mut created_directories),
                )?;
                let staged_path = payload_root.join(&artifact.relative_uri);
                fs::rename(&staged_path, &target).map_err(|error| {
                    CoreError::Storage(format!("artifact publish failed: {error}"))
                })?;
                installed.push(target.clone());
                sync_directory(target.parent().unwrap_or(&self.root))?;
                hook(CommitPhase::ArtifactInstalled)?;
            }
            for artifact in &manifest.artifacts {
                let target = self.root.join(&artifact.relative_uri);
                let (size, sha256) = hash_file(&target, self.limits.max_artifact_bytes)?;
                if size != artifact.size_bytes || sha256 != artifact.sha256 {
                    return Err(CoreError::Storage(
                        "published artifact verification failed".into(),
                    ));
                }
            }

            check_cancelled(cancellation)?;
            rename_manifest(
                &self.root,
                &manifest_stage,
                &manifest_path,
                &manifest,
                |source, destination| fs::rename(source, destination),
            )?;
            installed.push(manifest_path.clone());
            sync_directory(manifest_path.parent().unwrap_or(&self.root))?;
            Ok(())
        })();

        match transaction {
            Ok(()) => Ok(LocalCommitResult { status: LocalCommitStatus::Written, manifest }),
            Err(error) => {
                if let Err(rollback_error) =
                    rollback(&installed, &backups, &mut created_directories)
                {
                    let retained = stage.keep();
                    return Err(CoreError::Storage(format!(
                        "commit failed ({error}); rollback incomplete, recovery data retained at {} ({rollback_error})",
                        retained.display()
                    )));
                }
                Err(error)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitPhase {
    ManifestWithdrawn,
    ArtifactInstalled,
}

fn rename_manifest<F>(
    root: &Path,
    staged: &Path,
    destination: &Path,
    manifest: &Manifest,
    rename: F,
) -> CoreResult<()>
where
    F: FnOnce(&Path, &Path) -> std::io::Result<()>,
{
    if let Err(error) = rename(staged, destination)
        && !read_manifest(destination)?.is_some_and(|visible| {
            visible.generation == manifest.generation
                && visible.output_id == manifest.output_id
                && is_complete(root, &visible)
        })
    {
        return Err(CoreError::Storage(format!("manifest publish failed: {error}")));
    }
    Ok(())
}

fn validate_request(request: &LocalCommitRequest) -> CoreResult<()> {
    if !is_safe_relative_path(&request.output_name) {
        return Err(CoreError::Storage("output name must be a safe relative path".into()));
    }
    if !is_sha256(&request.revision) {
        return Err(CoreError::Storage("revision must be a lowercase SHA-256 digest".into()));
    }
    if request.artifacts.is_empty() {
        return Err(CoreError::Storage("commit requires at least one artifact".into()));
    }
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut includes_output = false;
    for artifact in &request.artifacts {
        if !is_safe_relative_path(&artifact.name)
            || !is_safe_relative_path(&artifact.relative_uri)
            || artifact.role.trim().is_empty()
            || artifact.media_type.trim().is_empty()
            || !names.insert(artifact.name.as_str())
            || !paths.insert(artifact.relative_uri.as_str())
        {
            return Err(CoreError::Storage(
                "commit contains an invalid or duplicate artifact".into(),
            ));
        }
        if artifact.relative_uri == request.output_name
            || artifact
                .relative_uri
                .strip_prefix(&request.output_name)
                .is_some_and(|suffix| suffix.starts_with('/') && suffix.len() > 1)
        {
            includes_output = true;
        }
        let first = artifact.relative_uri.split('/').next().unwrap_or_default();
        if first == ".radiust.lock" || first.starts_with(".radiust-stage-") {
            return Err(CoreError::Storage("artifact path is reserved".into()));
        }
        let metadata = fs::symlink_metadata(&artifact.source).map_err(|error| {
            CoreError::Storage(format!("staged artifact could not be inspected: {error}"))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CoreError::Storage(
                "staged artifact must be a regular non-symlink file".into(),
            ));
        }
    }
    reject_path_overlaps(request.artifacts.iter().map(|artifact| artifact.relative_uri.as_str()))?;
    if !includes_output {
        return Err(CoreError::Storage("artifact set must include output_name".into()));
    }
    Ok(())
}

fn merge_retained_artifacts(
    new_artifacts: &mut Vec<ManifestArtifact>,
    previous: &[ManifestArtifact],
) -> CoreResult<()> {
    let replaced_names =
        new_artifacts.iter().map(|item| item.name.clone()).collect::<BTreeSet<_>>();
    let replaced_paths =
        new_artifacts.iter().map(|item| item.relative_uri.clone()).collect::<Vec<_>>();
    for artifact in previous {
        if replaced_names.contains(artifact.name.as_str())
            || replaced_paths.iter().any(|path| path_overlaps(path, &artifact.relative_uri))
        {
            continue;
        }
        new_artifacts.push(artifact.clone());
    }
    validate_manifest_paths(new_artifacts)
}

fn validate_manifest_paths(artifacts: &[ManifestArtifact]) -> CoreResult<()> {
    let names = artifacts.iter().map(|item| item.name.as_str()).collect::<BTreeSet<_>>();
    let paths = artifacts.iter().map(|item| item.relative_uri.as_str()).collect::<BTreeSet<_>>();
    if names.len() != artifacts.len() || paths.len() != artifacts.len() {
        return Err(CoreError::Storage("manifest artifact paths must be unique".into()));
    }
    reject_path_overlaps(paths.iter().copied())
}

fn reject_path_overlap<'a>(base: &str, paths: impl Iterator<Item = &'a str>) -> CoreResult<()> {
    for path in paths {
        if path_overlaps(base, path) {
            return Err(CoreError::Storage("manifest path overlaps an artifact path".into()));
        }
    }
    Ok(())
}

fn reject_path_overlaps<'a>(paths: impl Iterator<Item = &'a str>) -> CoreResult<()> {
    let paths = paths.collect::<Vec<_>>();
    for (index, path) in paths.iter().enumerate() {
        if paths.iter().skip(index + 1).any(|other| path_overlaps(path, other)) {
            return Err(CoreError::Storage("artifact paths overlap".into()));
        }
    }
    Ok(())
}

fn path_overlaps(left: &str, right: &str) -> bool {
    left == right
        || left.strip_prefix(right).is_some_and(|suffix| suffix.starts_with('/'))
        || right.strip_prefix(left).is_some_and(|suffix| suffix.starts_with('/'))
}

fn path_depth(path: &str) -> usize {
    path.split('/').count()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn ensure_safe_parent(root: &Path, relative: &str, create: bool) -> CoreResult<()> {
    let components = relative.split('/').collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(CoreError::Storage("artifact parent is not a safe directory".into()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(&current).map_err(|error| {
                    CoreError::Storage(format!(
                        "artifact parent directory could not be created: {error}"
                    ))
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "artifact parent could not be inspected: {error}"
                )));
            }
        }
    }
    Ok(())
}

fn create_parent_dirs(
    root: &Path,
    relative: &str,
    mut created: Option<&mut Vec<PathBuf>>,
) -> CoreResult<()> {
    let components = relative.split('/').collect::<Vec<_>>();
    let mut current = root.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(CoreError::Storage("artifact parent is not a safe directory".into()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|error| {
                    CoreError::Storage(format!(
                        "artifact parent directory could not be created: {error}"
                    ))
                })?;
                if let Some(created) = created.as_deref_mut() {
                    created.push(current.clone());
                }
            }
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "artifact parent could not be inspected: {error}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_existing_target(path: &Path, allow_directory: bool) -> CoreResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(CoreError::Storage("output target cannot be a symlink".into()))
        }
        Ok(metadata) if metadata.is_file() => Ok(()),
        Ok(metadata) if allow_directory && metadata.is_dir() => {
            Err(CoreError::Storage("manifest target cannot be a directory".into()))
        }
        Ok(_) => Err(CoreError::Storage("output target has an unsupported file type".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(CoreError::Storage(format!("output target could not be inspected: {error}")))
        }
    }
}

fn backup_if_present(
    target: &Path,
    backup: &Path,
    backups: &mut Vec<(PathBuf, PathBuf)>,
) -> CoreResult<()> {
    match fs::symlink_metadata(target) {
        Ok(_) => {
            if let Some(parent) = backup.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    CoreError::Temporary(format!(
                        "rollback directory could not be created: {error}"
                    ))
                })?;
            }
            fs::rename(target, backup).map_err(|error| {
                CoreError::Storage(format!("existing output could not be protected: {error}"))
            })?;
            backups.push((target.to_path_buf(), backup.to_path_buf()));
            if let Some(parent) = target.parent() {
                sync_directory(parent)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CoreError::Storage(format!(
                "existing output could not be inspected: {error}"
            )));
        }
    }
    Ok(())
}

fn rollback(
    installed: &[PathBuf],
    backups: &[(PathBuf, PathBuf)],
    created_directories: &mut [PathBuf],
) -> Result<(), String> {
    for target in installed.iter().rev() {
        match fs::symlink_metadata(target) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                fs::remove_file(target).map_err(|error| error.to_string())?;
            }
            Ok(_) => return Err("published path changed type during rollback".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    for (target, backup) in backups.iter().rev() {
        match fs::symlink_metadata(backup) {
            Ok(_) => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                match fs::symlink_metadata(target) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Ok(_) => return Err("rollback target unexpectedly exists".into()),
                    Err(error) => return Err(error.to_string()),
                }
                fs::rename(backup, target).map_err(|error| error.to_string())?;
                if let Some(parent) = target.parent() {
                    sync_directory(parent).map_err(|error| error.to_string())?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    for directory in created_directories.iter().rev() {
        match fs::remove_dir(directory) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

fn copy_and_hash(
    source: &Path,
    destination: &Path,
    max_artifact: u64,
    max_frame_remaining: u64,
    max_temp_remaining: u64,
    cancellation: Option<&CancellationToken>,
) -> CoreResult<(u64, String)> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        CoreError::Storage(format!("staged artifact could not be inspected: {error}"))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CoreError::Storage(
            "staged artifact must be a regular non-symlink file".into(),
        ));
    }
    let mut input = File::open(source).map_err(|error| {
        CoreError::Storage(format!("staged artifact could not be opened: {error}"))
    })?;
    let mut output =
        OpenOptions::new().write(true).create_new(true).open(destination).map_err(|error| {
            CoreError::Temporary(format!("staged artifact copy could not be created: {error}"))
        })?;
    let mut size = 0_u64;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancelled(cancellation)?;
        let count = input.read(&mut buffer).map_err(|error| {
            CoreError::Storage(format!("staged artifact could not be read: {error}"))
        })?;
        if count == 0 {
            break;
        }
        let next = size
            .checked_add(count as u64)
            .ok_or_else(|| CoreError::ResourceLimit("artifact size overflow".into()))?;
        if next > max_artifact || next > max_frame_remaining || next > max_temp_remaining {
            return Err(CoreError::ResourceLimit(
                "staged output exceeds configured byte limit".into(),
            ));
        }
        output.write_all(&buffer[..count]).map_err(|error| {
            CoreError::Temporary(format!("staged artifact copy failed: {error}"))
        })?;
        hasher.update(&buffer[..count]);
        size = next;
    }
    output.sync_all().map_err(|error| {
        CoreError::Temporary(format!("staged artifact copy could not be flushed: {error}"))
    })?;
    Ok((size, hex::encode(hasher.finalize())))
}

fn check_cancelled(cancellation: Option<&CancellationToken>) -> CoreResult<()> {
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        Err(CoreError::Cancelled)
    } else {
        Ok(())
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sync_directory(path: &Path) -> CoreResult<()> {
    File::open(path).and_then(|directory| directory.sync_all()).map_err(|error| {
        CoreError::Storage(format!("output directory could not be synced: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame() -> FrameRef {
        FrameRef {
            source: "fixture".into(),
            product: "reflectivity".into(),
            station: None,
            valid_time: "2025-01-01T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "1".into(),
            locator: json!({"fixture": "sample"}),
        }
    }

    fn request(
        source: &Path,
        content: &[u8],
        revision: &str,
        overwrite: bool,
    ) -> LocalCommitRequest {
        fs::write(source, content).unwrap();
        LocalCommitRequest {
            frame: frame(),
            revision: revision.into(),
            processing_spec: ProcessingSpec::default(),
            output_name: "frame.bin".into(),
            artifacts: vec![StagedArtifact {
                name: "frame.bin".into(),
                relative_uri: "frame.bin".into(),
                role: "data".into(),
                media_type: "application/octet-stream".into(),
                source: source.to_path_buf(),
            }],
            raw_complete: false,
            overwrite,
        }
    }

    fn store(root: &Path) -> LocalStore {
        LocalStore::new(root, Limits::default()).unwrap()
    }

    #[test]
    fn writes_manifest_last_and_skips_only_verified_identical_output() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let store = store(root.path());
        let request = request(&source, b"first", &"a".repeat(64), false);

        let written = store.commit(request.clone()).unwrap();
        assert_eq!(written.status, LocalCommitStatus::Written);
        let manifest_path = root.path().join("frame.bin.manifest.json");
        assert!(read_manifest(&manifest_path).unwrap().is_some());
        assert!(is_complete(root.path(), &written.manifest));
        assert_eq!(fs::read(root.path().join("frame.bin")).unwrap(), b"first");

        let skipped = store.commit(request).unwrap();
        assert_eq!(skipped.status, LocalCommitStatus::Skipped);
        assert_eq!(skipped.manifest.generation, written.manifest.generation);
    }

    #[test]
    fn reads_and_skips_a_python_v1_manifest_fixture() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/rust-migration/output/legacy-v1");
        fs::copy(fixture.join("frame.bin"), root.path().join("frame.bin")).unwrap();
        fs::copy(
            fixture.join("frame.bin.manifest.json"),
            root.path().join("frame.bin.manifest.json"),
        )
        .unwrap();
        let store = store(root.path());

        let result =
            store.commit(request(&source, b"legacy-v1-bytes\n", &"a".repeat(64), false)).unwrap();
        assert_eq!(result.status, LocalCommitStatus::Skipped);
        assert_eq!(result.manifest.generation.as_deref(), Some("legacy-generation"));
    }

    #[test]
    fn conflict_preserves_previous_group_and_overwrite_records_supersedes() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let store = store(root.path());
        let first = store.commit(request(&source, b"first", &"a".repeat(64), false)).unwrap();
        let conflict = store.commit(request(&source, b"second", &"b".repeat(64), false));
        assert!(conflict.is_err());
        assert_eq!(fs::read(root.path().join("frame.bin")).unwrap(), b"first");
        assert!(is_complete(root.path(), &first.manifest));

        let overwritten = store.commit(request(&source, b"second", &"b".repeat(64), true)).unwrap();
        assert_eq!(overwritten.status, LocalCommitStatus::Written);
        assert_eq!(fs::read(root.path().join("frame.bin")).unwrap(), b"second");
        assert_eq!(
            overwritten.manifest.supersedes.as_ref().unwrap()["output_id"],
            first.manifest.output_id
        );
    }

    #[test]
    fn damaged_manifest_group_is_repaired_without_accepting_old_bytes() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let store = store(root.path());
        let first = store.commit(request(&source, b"first", &"a".repeat(64), false)).unwrap();
        fs::write(root.path().join("frame.bin"), b"damaged").unwrap();
        let repaired = store.commit(request(&source, b"fixed", &"a".repeat(64), false)).unwrap();
        assert_eq!(repaired.status, LocalCommitStatus::Written);
        assert_eq!(fs::read(root.path().join("frame.bin")).unwrap(), b"fixed");
        assert!(is_complete(root.path(), &repaired.manifest));
        assert_ne!(first.manifest.generation, repaired.manifest.generation);
    }

    #[test]
    fn raw_completeness_upgrade_keeps_verified_artifacts_not_rewritten() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let raw_source = inputs.path().join("raw.bin");
        let store = store(root.path());

        let mut initial = request(&source, b"decoded-v1", &"a".repeat(64), false);
        fs::write(&raw_source, b"raw-v1").unwrap();
        initial.artifacts.push(StagedArtifact {
            name: "raw.bin".into(),
            relative_uri: "raw.bin".into(),
            role: "raw".into(),
            media_type: "application/octet-stream".into(),
            source: raw_source.clone(),
        });
        let first = store.commit(initial).unwrap();

        let mut upgrade = request(&source, b"decoded-v2", &"a".repeat(64), false);
        upgrade.raw_complete = true;
        let completed = store.commit(upgrade).unwrap();
        assert_eq!(completed.status, LocalCommitStatus::Written);
        assert!(completed.manifest.raw_complete);
        assert_eq!(completed.manifest.artifacts.len(), 2);
        assert_eq!(fs::read(root.path().join("raw.bin")).unwrap(), b"raw-v1");
        assert_eq!(fs::read(root.path().join("frame.bin")).unwrap(), b"decoded-v2");
        assert!(is_complete(root.path(), &completed.manifest));
        assert!(completed.manifest.supersedes.is_some());
        assert_ne!(first.manifest.generation, completed.manifest.generation);
    }

    #[test]
    fn failed_publish_restores_old_artifacts_before_restoring_manifest() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let store = store(root.path());
        let first = store.commit(request(&source, b"first", &"a".repeat(64), false)).unwrap();
        let failure =
            store.commit_inner(request(&source, b"second", &"b".repeat(64), true), None, |phase| {
                if phase == CommitPhase::ArtifactInstalled {
                    Err(CoreError::Storage("injected publish failure".into()))
                } else {
                    Ok(())
                }
            });
        assert!(failure.is_err());
        assert_eq!(fs::read(root.path().join("frame.bin")).unwrap(), b"first");
        assert!(is_complete(root.path(), &first.manifest));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 3); // artifact, manifest, persistent lock
    }

    #[test]
    fn reconciles_manifest_publish_response_loss_after_atomic_rename() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let store = store(root.path());
        let first = store.commit(request(&source, b"first", &"a".repeat(64), false)).unwrap();
        let mut replacement = first.manifest.clone();
        replacement.generation = Some("response-loss-generation".into());
        let staged = inputs.path().join("manifest.stage");
        fs::write(&staged, replacement.bytes().unwrap()).unwrap();
        let destination = root.path().join("frame.bin.manifest.json");

        let result = rename_manifest(
            root.path(),
            &staged,
            &destination,
            &replacement,
            |source, destination| {
                fs::rename(source, destination)?;
                Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "injected response loss after rename",
                ))
            },
        );

        assert!(result.is_ok());
        let visible = read_manifest(&destination).unwrap().unwrap();
        assert_eq!(visible.generation.as_deref(), Some("response-loss-generation"));
        assert!(is_complete(root.path(), &visible));
    }

    #[test]
    fn rejects_traversal_and_symlinked_output_targets() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let store = store(root.path());
        let mut invalid = request(&source, b"data", &"a".repeat(64), false);
        invalid.artifacts[0].relative_uri = "../outside".into();
        assert!(store.commit(invalid).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = inputs.path().join("outside");
            fs::write(&outside, b"outside").unwrap();
            symlink(&outside, root.path().join("frame.bin")).unwrap();
            assert!(store.commit(request(&source, b"inside", &"a".repeat(64), true)).is_err());
            assert_eq!(fs::read(outside).unwrap(), b"outside");
        }
    }

    #[test]
    fn rejects_overlapping_file_paths_before_mutating_output() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let store = store(root.path());
        let mut request = request(&source, b"data", &"a".repeat(64), false);
        request.artifacts.push(StagedArtifact {
            name: "frame.bin/child".into(),
            relative_uri: "frame.bin/child".into(),
            role: "data".into(),
            media_type: "application/octet-stream".into(),
            source,
        });
        assert!(store.commit(request).is_err());
        assert!(!root.path().join("frame.bin").exists());
    }

    #[test]
    fn respects_the_cross_process_root_lock_while_another_writer_holds_it() {
        let root = tempfile::tempdir().unwrap();
        let inputs = tempfile::tempdir().unwrap();
        let source = inputs.path().join("source.bin");
        let store = store(root.path());
        let lock = RootLock::acquire(root.path()).unwrap();
        assert!(store.commit(request(&source, b"second", &"b".repeat(64), false)).is_err());
        drop(lock);
        assert!(store.commit(request(&source, b"first", &"a".repeat(64), false)).is_ok());
        assert_eq!(fs::read(root.path().join("frame.bin")).unwrap(), b"first");
    }
}
