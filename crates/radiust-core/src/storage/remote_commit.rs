//! Remote immutable-generation commits with a manifest-last visibility fence.
//!
//! Object-store commits assume one writer per logical output. Providers do not
//! expose a portable compare-and-swap operation through `ObjectStore`, so an
//! overwrite rechecks the pointer immediately before publishing but cannot
//! exclude a concurrent writer between that check and the write.

use crate::errors::{CoreError, CoreResult};
use crate::identity::{logical_id, output_id, processing_hash, processing_identity};
use crate::storage::commit::{LocalCommitRequest, LocalCommitResult, LocalCommitStatus};
use crate::storage::manifest::{
    Manifest, ManifestArtifact, ManifestFields, is_safe_relative_path, new_manifest,
};
use crate::storage::object::{ObjectReceipt, ObjectStore};
use parking_lot::Mutex as ParkingMutex;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
static REMOTE_POINTER_GATES: OnceLock<ParkingMutex<HashMap<String, Weak<AsyncMutex<()>>>>> =
    OnceLock::new();

fn process_pointer_gate(pointer_key: &str) -> Arc<AsyncMutex<()>> {
    let gates = REMOTE_POINTER_GATES.get_or_init(|| ParkingMutex::new(HashMap::new()));
    let mut gates = gates.lock();
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(pointer_key).and_then(Weak::upgrade) {
        return gate;
    }
    let gate = Arc::new(AsyncMutex::new(()));
    gates.insert(pointer_key.to_owned(), Arc::downgrade(&gate));
    gate
}

#[derive(Clone)]
pub struct RemoteStore {
    objects: ObjectStore,
    max_artifact_bytes: u64,
    max_frame_bytes: u64,
}

impl RemoteStore {
    pub fn new(objects: ObjectStore, max_artifact_bytes: u64, max_frame_bytes: u64) -> Self {
        Self { objects, max_artifact_bytes, max_frame_bytes }
    }

    /// Commit a request as an immutable generation and publish its manifest at
    /// the stable output pointer only after all generation objects verify.
    ///
    /// An ambiguous pointer write is represented by the non-retryable
    /// `CoreError::CommitOutcomeUnknown` variant.
    pub async fn commit_cancellable(
        &self,
        request: LocalCommitRequest,
        cancellation: CancellationToken,
    ) -> CoreResult<LocalCommitResult> {
        check_cancelled(&cancellation)?;
        validate_request(&request, self.max_artifact_bytes, self.max_frame_bytes)?;
        let pointer_key = format!("{}.manifest.json", request.output_name);
        let pointer_gate = process_pointer_gate(&pointer_key);
        let _pointer_guard = pointer_gate.lock().await;

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

        let current_bytes = self
            .objects
            .read_optional(&pointer_key, MAX_MANIFEST_BYTES)
            .await?
            .map(|(bytes, _)| bytes);
        let current = current_bytes.as_deref().map(decode_remote_manifest).transpose()?;
        if let Some(manifest) = current.as_ref() {
            validate_generation_manifest(manifest)?;
        }

        let current_complete = if let Some(manifest) = current.as_ref() {
            self.manifest_artifacts_complete(manifest, &cancellation).await?
        } else {
            false
        };

        if let Some(existing) = current.as_ref()
            && current_complete
            && !request.overwrite
        {
            if existing.output_id == output {
                let supplementing_raw = request.raw_complete && !existing.raw_complete;
                if !supplementing_raw {
                    return Ok(LocalCommitResult {
                        status: LocalCommitStatus::Skipped,
                        manifest: existing.clone(),
                    });
                }
            } else {
                return Err(CoreError::OutputConflict);
            }
        }

        let supersedes = current.as_ref().filter(|_| current_complete).map(|manifest| {
            json!({
                "output_id": manifest.output_id,
                "generation": manifest.generation,
                "pointer_key": pointer_key,
            })
        });
        let retaining_previous = current.as_ref().filter(|manifest| {
            current_complete
                && manifest.output_id == output
                && request.raw_complete
                && !manifest.raw_complete
        });

        let generation = uuid::Uuid::new_v4().simple().to_string();
        let generation_prefix = format!("_generations/{output}/{generation}");
        let mut uploads = request
            .artifacts
            .iter()
            .map(|artifact| UploadArtifact {
                name: artifact.name.clone(),
                relative_path: artifact.relative_uri.clone(),
                role: artifact.role.clone(),
                media_type: artifact.media_type.clone(),
                source: UploadSource::Local(artifact.source.clone()),
                expected: None,
            })
            .collect::<Vec<_>>();

        if let Some(previous) = retaining_previous {
            append_retained_artifacts(previous, &request, &mut uploads)?;
        }
        validate_uploads(&uploads, self.max_artifact_bytes, self.max_frame_bytes)?;

        let mut total_bytes = 0_u64;
        let mut artifacts = Vec::with_capacity(uploads.len());
        let mut generation_keys = Vec::with_capacity(uploads.len() + 1);
        let retained_temp = tempfile::tempdir().map_err(|error| {
            CoreError::Temporary(format!("remote staging directory could not be created: {error}"))
        })?;

        let generation_manifest_key = format!("{generation_prefix}/manifest.json");
        let prepared: CoreResult<(Manifest, Vec<u8>)> = async {
            for (index, upload) in uploads.iter().enumerate() {
                check_cancelled(&cancellation)?;
                let (source, expected) = match &upload.source {
                    UploadSource::Local(path) => (path.clone(), upload.expected.as_ref()),
                    UploadSource::Remote(previous) => {
                        let local_path = retained_temp.path().join(format!("retained-{index:08}"));
                        let receipt = self
                            .objects
                            .read_to_path_cancellable(
                                &previous.relative_uri,
                                &local_path,
                                self.max_artifact_bytes,
                                &cancellation,
                            )
                            .await?;
                        if !receipt_matches(&receipt, previous) {
                            return Err(CoreError::Storage(
                                "retained remote artifact failed verification".into(),
                            ));
                        }
                        (local_path, Some(previous))
                    }
                };

                let remaining_frame = self.max_frame_bytes.saturating_sub(total_bytes);
                let byte_limit = self.max_artifact_bytes.min(remaining_frame);
                let key = format!("{generation_prefix}/{}", upload.relative_path);
                // The generation id is unique, so recording before the request
                // also lets cleanup remove an object after an ambiguous write error.
                generation_keys.push(key.clone());
                let receipt = self.write_file(&key, &source, byte_limit, &cancellation).await?;
                // Cancellation is checked between bounded source-file chunks and
                // again before readback and the final pointer fence.
                check_cancelled(&cancellation)?;
                if let Some(expected) = expected
                    && !receipt_matches(&receipt, expected)
                {
                    return Err(CoreError::Storage(
                        "retained remote artifact changed while copying generations".into(),
                    ));
                }
                self.verify_uploaded_artifact(&key, &receipt, &cancellation).await?;
                total_bytes = total_bytes
                    .checked_add(receipt.size_bytes)
                    .ok_or_else(|| CoreError::ResourceLimit("frame size overflow".into()))?;
                if total_bytes > self.max_frame_bytes {
                    return Err(CoreError::ResourceLimit(format!(
                        "frame exceeds configured limit {}",
                        self.max_frame_bytes
                    )));
                }
                artifacts.push(ManifestArtifact {
                    name: upload.name.clone(),
                    relative_uri: key,
                    role: upload.role.clone(),
                    media_type: upload.media_type.clone(),
                    size_bytes: receipt.size_bytes,
                    sha256: receipt.sha256,
                });
            }

            artifacts.sort_by(|left, right| left.relative_uri.cmp(&right.relative_uri));
            let manifest = new_manifest(ManifestFields {
                logical_id: logical,
                revision: request.revision.clone(),
                output_id: output,
                processing_spec,
                processing_hash,
                artifacts,
                raw_complete: request.raw_complete,
                generation: generation.clone(),
                supersedes,
            })?;
            let manifest_bytes = manifest.bytes()?;
            if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
                return Err(CoreError::ResourceLimit(format!(
                    "generation manifest exceeds configured limit {MAX_MANIFEST_BYTES}"
                )));
            }

            generation_keys.push(generation_manifest_key.clone());
            self.objects
                .write_chunks_once(
                    &generation_manifest_key,
                    vec![manifest_bytes.clone()],
                    MAX_MANIFEST_BYTES,
                )
                .await?;
            let (stored_manifest, manifest_receipt) =
                self.objects.read(&generation_manifest_key, MAX_MANIFEST_BYTES).await?;
            if stored_manifest != manifest_bytes
                || manifest_receipt.size_bytes != manifest_bytes.len() as u64
                || manifest_receipt.sha256 != sha256(&manifest_bytes)
            {
                return Err(CoreError::Storage(
                    "remote generation manifest failed read-back verification".into(),
                ));
            }

            // This is the commit fence. Once the pointer request starts, cancellation
            // is not accepted; an error is reconciled by reading the stable pointer.
            check_cancelled(&cancellation)?;
            let latest_bytes = self
                .objects
                .read_optional(&pointer_key, MAX_MANIFEST_BYTES)
                .await?
                .map(|(bytes, _)| bytes);
            if latest_bytes != current_bytes {
                return Err(CoreError::OutputConflict);
            }
            check_cancelled(&cancellation)?;
            Ok((manifest, manifest_bytes))
        }
        .await;
        let (manifest, manifest_bytes) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.cleanup_generation(&generation_keys).await;
                return Err(error);
            }
        };

        let pointer_write = if current_bytes.is_some() {
            self.objects
                .write_chunks(&pointer_key, vec![manifest_bytes.clone()], MAX_MANIFEST_BYTES)
                .await
        } else {
            self.objects
                .write_chunks_once(&pointer_key, vec![manifest_bytes.clone()], MAX_MANIFEST_BYTES)
                .await
        };
        if let Err(_publish_error) = pointer_write {
            let reconciled = self.objects.read_optional(&pointer_key, MAX_MANIFEST_BYTES).await;
            if matches!(reconciled, Ok(Some((visible, _))) if visible == manifest_bytes) {
                return Ok(LocalCommitResult { status: LocalCommitStatus::Written, manifest });
            }
            return Err(CoreError::CommitOutcomeUnknown);
        }

        Ok(LocalCommitResult { status: LocalCommitStatus::Written, manifest })
    }

    async fn write_file(
        &self,
        key: &str,
        source: &Path,
        max_bytes: u64,
        cancellation: &CancellationToken,
    ) -> CoreResult<ObjectReceipt> {
        check_cancelled(cancellation)?;
        self.objects.write_path_cancellable(key, source, max_bytes, cancellation).await
    }

    async fn cleanup_generation(&self, keys: &[String]) {
        for key in keys.iter().rev() {
            let _ = self.objects.delete(key).await;
        }
    }

    async fn verify_uploaded_artifact(
        &self,
        key: &str,
        expected: &ObjectReceipt,
        cancellation: &CancellationToken,
    ) -> CoreResult<()> {
        let temp = tempfile::tempdir().map_err(|error| {
            CoreError::Temporary(format!(
                "artifact verification directory could not be created: {error}"
            ))
        })?;
        let actual = self
            .objects
            .read_to_path_cancellable(
                key,
                temp.path().join("artifact"),
                self.max_artifact_bytes,
                cancellation,
            )
            .await?;
        if actual != *expected {
            return Err(CoreError::Storage(format!("remote artifact verification failed: {key}")));
        }
        Ok(())
    }

    async fn manifest_artifacts_complete(
        &self,
        manifest: &Manifest,
        cancellation: &CancellationToken,
    ) -> CoreResult<bool> {
        let mut total_bytes = 0_u64;
        for artifact in &manifest.artifacts {
            check_cancelled(cancellation)?;
            if artifact.size_bytes > self.max_artifact_bytes {
                return Ok(false);
            }
            total_bytes = match total_bytes.checked_add(artifact.size_bytes) {
                Some(total) if total <= self.max_frame_bytes => total,
                _ => return Ok(false),
            };
            let temp = tempfile::tempdir().map_err(|error| {
                CoreError::Temporary(format!(
                    "artifact verification directory could not be created: {error}"
                ))
            })?;
            let readback = self
                .objects
                .read_to_path_cancellable(
                    &artifact.relative_uri,
                    temp.path().join("artifact"),
                    self.max_artifact_bytes,
                    cancellation,
                )
                .await;
            let Ok(receipt) = readback else {
                // A missing or unreadable old object is incomplete. It must never
                // qualify for skip, but a fresh immutable generation can repair it.
                return Ok(false);
            };
            if !receipt_matches(&receipt, artifact) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[derive(Clone)]
struct UploadArtifact {
    name: String,
    relative_path: String,
    role: String,
    media_type: String,
    source: UploadSource,
    expected: Option<ManifestArtifact>,
}

#[derive(Clone)]
enum UploadSource {
    Local(PathBuf),
    Remote(ManifestArtifact),
}

fn validate_request(
    request: &LocalCommitRequest,
    max_artifact_bytes: u64,
    max_frame_bytes: u64,
) -> CoreResult<()> {
    if max_artifact_bytes == 0 || max_frame_bytes == 0 {
        return Err(CoreError::Storage("remote commit limits must be positive".into()));
    }
    if !is_safe_relative_path(&request.output_name) {
        return Err(CoreError::Storage("output name must be a safe relative path".into()));
    }
    if !is_sha256(&request.revision) {
        return Err(CoreError::Storage("revision must be a lowercase SHA-256 digest".into()));
    }
    if request.artifacts.is_empty() {
        return Err(CoreError::Storage("commit requires at least one artifact".into()));
    }

    let pointer_key = format!("{}.manifest.json", request.output_name);
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut includes_output = false;
    let mut planned_bytes = 0_u64;
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
        if path_overlaps(&pointer_key, &artifact.relative_uri) {
            return Err(CoreError::Storage("manifest path overlaps an artifact path".into()));
        }
        let metadata = std::fs::symlink_metadata(&artifact.source).map_err(|error| {
            CoreError::Storage(format!("staged artifact could not be inspected: {error}"))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CoreError::Storage(
                "staged artifact must be a regular non-symlink file".into(),
            ));
        }
        if metadata.len() > max_artifact_bytes {
            return Err(CoreError::ResourceLimit(format!(
                "artifact exceeds configured limit {max_artifact_bytes}"
            )));
        }
        planned_bytes = planned_bytes
            .checked_add(metadata.len())
            .ok_or_else(|| CoreError::ResourceLimit("frame size overflow".into()))?;
        if planned_bytes > max_frame_bytes {
            return Err(CoreError::ResourceLimit(format!(
                "frame exceeds configured limit {max_frame_bytes}"
            )));
        }
    }
    reject_path_overlaps(paths.iter().copied())?;
    if !includes_output {
        return Err(CoreError::Storage("artifact set must include output_name".into()));
    }
    Ok(())
}

fn append_retained_artifacts(
    previous: &Manifest,
    request: &LocalCommitRequest,
    uploads: &mut Vec<UploadArtifact>,
) -> CoreResult<()> {
    let prefix = generation_prefix(previous)?;
    for artifact in &previous.artifacts {
        let relative_path = artifact
            .relative_uri
            .strip_prefix(&format!("{prefix}/"))
            .ok_or_else(|| CoreError::Storage("remote artifact is outside its generation".into()))?
            .to_owned();
        if request.artifacts.iter().any(|requested| {
            requested.name == artifact.name
                || path_overlaps(&requested.relative_uri, &relative_path)
        }) {
            continue;
        }
        uploads.push(UploadArtifact {
            name: artifact.name.clone(),
            relative_path,
            role: artifact.role.clone(),
            media_type: artifact.media_type.clone(),
            source: UploadSource::Remote(artifact.clone()),
            expected: Some(artifact.clone()),
        });
    }
    Ok(())
}

fn validate_uploads(
    uploads: &[UploadArtifact],
    max_artifact_bytes: u64,
    max_frame_bytes: u64,
) -> CoreResult<()> {
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();
    let mut total = 0_u64;
    for upload in uploads {
        if !is_safe_relative_path(&upload.name)
            || !is_safe_relative_path(&upload.relative_path)
            || upload.role.trim().is_empty()
            || upload.media_type.trim().is_empty()
            || !names.insert(upload.name.as_str())
            || !paths.insert(upload.relative_path.as_str())
        {
            return Err(CoreError::Storage(
                "remote generation artifacts overlap or duplicate".into(),
            ));
        }
        let size = match &upload.source {
            UploadSource::Local(path) => std::fs::metadata(path)
                .map_err(|error| {
                    CoreError::Storage(format!("staged artifact could not be inspected: {error}"))
                })?
                .len(),
            UploadSource::Remote(artifact) => artifact.size_bytes,
        };
        if size > max_artifact_bytes {
            return Err(CoreError::ResourceLimit(format!(
                "artifact exceeds configured limit {max_artifact_bytes}"
            )));
        }
        total = total
            .checked_add(size)
            .ok_or_else(|| CoreError::ResourceLimit("frame size overflow".into()))?;
        if total > max_frame_bytes {
            return Err(CoreError::ResourceLimit(format!(
                "frame exceeds configured limit {max_frame_bytes}"
            )));
        }
    }
    reject_path_overlaps(paths.iter().copied())
}

fn decode_remote_manifest(bytes: &[u8]) -> CoreResult<Manifest> {
    let manifest: Manifest = serde_json::from_slice(bytes)
        .map_err(|_| CoreError::Storage("remote output pointer is not a valid manifest".into()))?;
    manifest.validate()?;
    Ok(manifest)
}

fn validate_generation_manifest(manifest: &Manifest) -> CoreResult<()> {
    let prefix = generation_prefix(manifest)?;
    if manifest
        .artifacts
        .iter()
        .any(|artifact| !artifact.relative_uri.starts_with(&format!("{prefix}/")))
    {
        return Err(CoreError::Storage(
            "remote manifest artifact is outside its immutable generation".into(),
        ));
    }
    Ok(())
}

fn generation_prefix(manifest: &Manifest) -> CoreResult<String> {
    let generation = manifest
        .generation
        .as_deref()
        .filter(|value| is_safe_relative_path(value))
        .ok_or_else(|| CoreError::Storage("remote manifest generation is invalid".into()))?;
    Ok(format!("_generations/{}/{generation}", manifest.output_id))
}

fn receipt_matches(receipt: &ObjectReceipt, artifact: &ManifestArtifact) -> bool {
    receipt.size_bytes == artifact.size_bytes && receipt.sha256 == artifact.sha256
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
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

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn check_cancelled(cancellation: &CancellationToken) -> CoreResult<()> {
    if cancellation.is_cancelled() { Err(CoreError::Cancelled) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ProcessingSpec;
    use crate::model::FrameRef;
    use crate::storage::commit::StagedArtifact;
    use opendal::raw::{
        Layer, OpCompose, OpCopy, OpCreateDir, OpList, OpPresign, OpRead, OpRename, OpRestore,
        OpStat, OpWrite, RpCreateDir, RpPresign, RpRename, RpRestore, RpStat, Service, ServiceInfo,
        Servicer,
    };
    use opendal::{
        Buffer, Capability, Error, ErrorKind, OperationContext, Operator, raw, services,
    };
    use std::fmt::{Debug, Formatter};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn frame() -> FrameRef {
        FrameRef {
            source: "test".into(),
            product: "radar".into(),
            station: Some("KXYZ".into()),
            valid_time: "2026-09-26T00:00:00Z".into(),
            base_time: None,
            logical_id: String::new(),
            revision: None,
            locator_version: "1".into(),
            locator: json!({}),
        }
    }

    fn request(source: &Path, overwrite: bool) -> LocalCommitRequest {
        LocalCommitRequest {
            frame: frame(),
            revision: "a".repeat(64),
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

    fn memory_store() -> (RemoteStore, Operator) {
        let operator = Operator::new(services::Memory::default()).unwrap();
        let objects = ObjectStore::from_operator(operator.clone(), "").unwrap();
        (RemoteStore::new(objects, 1024 * 1024, 2 * 1024 * 1024), operator)
    }

    fn fault_store(action: FaultAction) -> (RemoteStore, Operator, Arc<FaultLog>) {
        let memory = Operator::new(services::Memory::default()).unwrap();
        let log = Arc::new(FaultLog::new(action));
        let operator = memory.clone().layer(FaultLayer { log: log.clone() });
        let objects = ObjectStore::from_operator(operator, "").unwrap();
        (RemoteStore::new(objects, 1024 * 1024, 2 * 1024 * 1024), memory, log)
    }

    #[tokio::test]
    async fn remote_manifest_is_published_last_and_verified_repeat_is_skipped() {
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("frame.bin");
        std::fs::write(&source, b"manifest-last-data").unwrap();
        let (store, operator, log) = fault_store(FaultAction::Observe);

        let first = store
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap();
        let second = store
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(first.status, LocalCommitStatus::Written);
        assert_eq!(second.status, LocalCommitStatus::Skipped);
        assert_eq!(first.manifest.generation, second.manifest.generation);
        let pointer_key = "frame.bin.manifest.json";
        let pointer = operator.read(pointer_key).await.unwrap().to_vec();
        let generation = first.manifest.generation.as_deref().unwrap();
        let generation_manifest_key =
            format!("_generations/{}/{generation}/manifest.json", first.manifest.output_id);
        assert_eq!(operator.read(&generation_manifest_key).await.unwrap().to_vec(), pointer);
        assert!(first.manifest.artifacts.iter().all(|artifact| {
            artifact
                .relative_uri
                .starts_with(&format!("_generations/{}/{generation}/", first.manifest.output_id))
        }));

        let events = log.events();
        let artifact_read = events
            .iter()
            .position(|event| {
                event.starts_with("read:_generations/") && event.ends_with("/frame.bin")
            })
            .unwrap();
        let generation_manifest_write = events
            .iter()
            .position(|event| {
                event.starts_with("close:_generations/") && event.ends_with("/manifest.json")
            })
            .unwrap();
        let pointer_write =
            events.iter().position(|event| event == "close:frame.bin.manifest.json").unwrap();
        assert!(artifact_read < generation_manifest_write);
        assert!(generation_manifest_write < pointer_write);
    }

    #[tokio::test]
    async fn incomplete_remote_generation_is_repaired_without_overwrite() {
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("frame.bin");
        std::fs::write(&source, b"repairable-output").unwrap();
        let (store, operator) = memory_store();
        let first = store
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap();
        let missing_artifact = &first.manifest.artifacts[0].relative_uri;
        operator.delete(missing_artifact).await.unwrap();

        let repaired = store
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(repaired.status, LocalCommitStatus::Written);
        assert_ne!(repaired.manifest.generation, first.manifest.generation);
        assert_eq!(
            operator.read(&repaired.manifest.artifacts[0].relative_uri).await.unwrap().to_vec(),
            b"repairable-output"
        );
        let pointer = operator.read("frame.bin.manifest.json").await.unwrap();
        assert_eq!(pointer.to_vec(), repaired.manifest.bytes().unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_commits_for_one_pointer_are_serialized() {
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("frame.bin");
        std::fs::write(&source, b"concurrent-output").unwrap();
        let (store, _) = memory_store();
        let barrier = Arc::new(tokio::sync::Barrier::new(8));
        let jobs = (0..8).map(|_| {
            let store = store.clone();
            let request = request(&source, false);
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                store.commit_cancellable(request, CancellationToken::new()).await.unwrap().status
            })
        });
        let outcomes = futures_util::future::join_all(jobs)
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<_>>();

        assert_eq!(
            outcomes.iter().filter(|status| **status == LocalCommitStatus::Written).count(),
            1
        );
        assert_eq!(
            outcomes.iter().filter(|status| **status == LocalCommitStatus::Skipped).count(),
            7
        );
    }

    #[tokio::test]
    async fn artifact_readback_corruption_or_failure_never_publishes_pointer() {
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("frame.bin");
        std::fs::write(&source, b"verify-me").unwrap();

        let (corrupt, corrupt_operator, _) = fault_store(FaultAction::CorruptArtifact);
        let error = corrupt
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            matches!(error, CoreError::Storage(message) if message.contains("artifact verification failed"))
        );
        assert!(!corrupt_operator.exists("frame.bin.manifest.json").await.unwrap());
        assert!(corrupt_operator.list("_generations/").await.unwrap().is_empty());

        let (fail, fail_operator, _) = fault_store(FaultAction::FailArtifactReadback);
        let error = fail
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::Transport(_)));
        assert!(!fail_operator.exists("frame.bin.manifest.json").await.unwrap());
        assert!(fail_operator.list("_generations/").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn generation_manifest_write_failure_never_publishes_pointer() {
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("frame.bin");
        std::fs::write(&source, b"manifest-write-failure").unwrap();
        let (store, operator, _) = fault_store(FaultAction::FailGenerationManifest);

        let error = store
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap_err();

        assert!(matches!(error, CoreError::Storage(_)));
        assert!(!operator.exists("frame.bin.manifest.json").await.unwrap());
        assert!(operator.list("_generations/").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancellation_after_generation_manifest_does_not_publish_pointer() {
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("frame.bin");
        std::fs::write(&source, b"cancel-before-pointer").unwrap();
        let cancellation = CancellationToken::new();
        let (store, operator, _) =
            fault_store(FaultAction::CancelAfterGenerationManifest(cancellation.clone()));

        let error =
            store.commit_cancellable(request(&source, false), cancellation).await.unwrap_err();

        assert!(matches!(error, CoreError::Cancelled));
        assert!(!operator.exists("frame.bin.manifest.json").await.unwrap());
        assert!(operator.list("_generations/").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancellation_during_pointer_write_does_not_interrupt_commit_fence() {
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("frame.bin");
        std::fs::write(&source, b"cancel-during-pointer-write").unwrap();
        let cancellation = CancellationToken::new();
        let (store, operator, _) =
            fault_store(FaultAction::CancelAfterPointerWrite(cancellation.clone()));

        let result =
            store.commit_cancellable(request(&source, false), cancellation.clone()).await.unwrap();

        assert!(cancellation.is_cancelled());
        assert_eq!(result.status, LocalCommitStatus::Written);
        assert_eq!(
            operator.read("frame.bin.manifest.json").await.unwrap().to_vec(),
            result.manifest.bytes().unwrap()
        );
    }

    #[tokio::test]
    async fn lost_pointer_responses_are_reconciled_or_reported_unknown() {
        let input = tempfile::tempdir().unwrap();
        let source = input.path().join("frame.bin");
        std::fs::write(&source, b"pointer-response").unwrap();

        let (after, _, _) = fault_store(FaultAction::LosePointerAfterWrite);
        let result = after
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.status, LocalCommitStatus::Written);

        let (before, _, _) = fault_store(FaultAction::LosePointerBeforeWrite);
        let error = before
            .commit_cancellable(request(&source, false), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::CommitOutcomeUnknown));
    }

    #[tokio::test]
    async fn overwrite_publishes_a_new_generation_and_keeps_superseded_objects() {
        let input = tempfile::tempdir().unwrap();
        let old_source = input.path().join("old.bin");
        let new_source = input.path().join("new.bin");
        std::fs::write(&old_source, b"old-generation").unwrap();
        std::fs::write(&new_source, b"new-generation").unwrap();
        let (store, operator) = memory_store();

        let first = store
            .commit_cancellable(request(&old_source, false), CancellationToken::new())
            .await
            .unwrap();
        let second = store
            .commit_cancellable(request(&new_source, true), CancellationToken::new())
            .await
            .unwrap();

        assert_ne!(first.manifest.generation, second.manifest.generation);
        assert_eq!(
            second.manifest.supersedes.as_ref().unwrap()["generation"],
            first.manifest.generation.as_deref().unwrap()
        );
        assert_eq!(
            operator.read(&first.manifest.artifacts[0].relative_uri).await.unwrap().to_vec(),
            b"old-generation"
        );
        assert_eq!(
            operator.read(&second.manifest.artifacts[0].relative_uri).await.unwrap().to_vec(),
            b"new-generation"
        );
    }

    #[tokio::test]
    async fn raw_completion_copies_retained_artifacts_into_the_new_generation() {
        let input = tempfile::tempdir().unwrap();
        let output_source = input.path().join("frame.bin");
        let metadata_source = input.path().join("metadata.bin");
        let raw_source = input.path().join("raw.bin");
        std::fs::write(&output_source, b"decoded-output").unwrap();
        std::fs::write(&metadata_source, b"retained-metadata").unwrap();
        std::fs::write(&raw_source, b"raw-source").unwrap();
        let (store, operator) = memory_store();

        let mut first_request = request(&output_source, false);
        first_request.artifacts.push(StagedArtifact {
            name: "metadata.bin".into(),
            relative_uri: "metadata.bin".into(),
            role: "metadata".into(),
            media_type: "application/octet-stream".into(),
            source: metadata_source,
        });
        let first =
            store.commit_cancellable(first_request, CancellationToken::new()).await.unwrap();

        let mut raw_request = request(&output_source, false);
        raw_request.raw_complete = true;
        raw_request.artifacts.push(StagedArtifact {
            name: "raw.bin".into(),
            relative_uri: "raw/raw.bin".into(),
            role: "raw".into(),
            media_type: "application/octet-stream".into(),
            source: raw_source,
        });
        let completed =
            store.commit_cancellable(raw_request, CancellationToken::new()).await.unwrap();

        assert_eq!(completed.status, LocalCommitStatus::Written);
        assert_eq!(
            completed.manifest.supersedes.as_ref().unwrap()["generation"],
            first.manifest.generation.as_deref().unwrap()
        );
        let retained = completed
            .manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.name == "metadata.bin")
            .unwrap();
        let generation = completed.manifest.generation.as_deref().unwrap();
        assert!(
            retained.relative_uri.starts_with(&format!(
                "_generations/{}/{generation}/",
                completed.manifest.output_id
            ))
        );
        assert_eq!(
            operator.read(&retained.relative_uri).await.unwrap().to_vec(),
            b"retained-metadata"
        );
        let old_retained = first
            .manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.name == "metadata.bin")
            .unwrap();
        assert_eq!(
            operator.read(&old_retained.relative_uri).await.unwrap().to_vec(),
            b"retained-metadata"
        );
    }

    #[derive(Clone, Debug)]
    enum FaultAction {
        Observe,
        CorruptArtifact,
        FailArtifactReadback,
        FailGenerationManifest,
        CancelAfterGenerationManifest(CancellationToken),
        CancelAfterPointerWrite(CancellationToken),
        LosePointerAfterWrite,
        LosePointerBeforeWrite,
    }

    struct FaultLog {
        action: FaultAction,
        fired: AtomicBool,
        events: Mutex<Vec<String>>,
    }

    impl FaultLog {
        fn new(action: FaultAction) -> Self {
            Self { action, fired: AtomicBool::new(false), events: Mutex::new(Vec::new()) }
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        fn record(&self, event: String) {
            self.events.lock().unwrap().push(event);
        }

        fn claim(&self, predicate: impl FnOnce(&FaultAction) -> bool) -> bool {
            if predicate(&self.action) { !self.fired.swap(true, Ordering::SeqCst) } else { false }
        }

        fn is_artifact_path(path: &str) -> bool {
            path.starts_with("_generations/") && !path.ends_with("/manifest.json")
        }
    }

    impl Debug for FaultLog {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FaultLog").finish_non_exhaustive()
        }
    }

    #[derive(Clone)]
    struct FaultLayer {
        log: Arc<FaultLog>,
    }

    impl Debug for FaultLayer {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FaultLayer").finish_non_exhaustive()
        }
    }

    impl Layer for FaultLayer {
        fn apply_service(&self, service: Servicer) -> Servicer {
            Arc::new(FaultService { inner: service, log: self.log.clone() })
        }
    }

    struct FaultService {
        inner: Servicer,
        log: Arc<FaultLog>,
    }

    impl Debug for FaultService {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("FaultService").finish_non_exhaustive()
        }
    }

    impl Service for FaultService {
        type Reader = raw::oio::Reader;
        type Writer = raw::oio::Writer;
        type Lister = raw::oio::Lister;
        type Deleter = raw::oio::Deleter;
        type Copier = raw::oio::Copier;
        type Composer = raw::oio::Composer;

        fn info(&self) -> ServiceInfo {
            self.inner.info()
        }

        fn capability(&self) -> Capability {
            self.inner.capability()
        }

        async fn create_dir(
            &self,
            ctx: &OperationContext,
            path: &str,
            args: OpCreateDir,
        ) -> opendal::Result<RpCreateDir> {
            self.inner.create_dir(ctx, path, args).await
        }

        async fn stat(
            &self,
            ctx: &OperationContext,
            path: &str,
            args: OpStat,
        ) -> opendal::Result<RpStat> {
            self.inner.stat(ctx, path, args).await
        }

        fn read(
            &self,
            ctx: &OperationContext,
            path: &str,
            args: OpRead,
        ) -> opendal::Result<Self::Reader> {
            self.log.record(format!("read:{path}"));
            if FaultLog::is_artifact_path(path)
                && self.log.claim(|action| matches!(action, FaultAction::FailArtifactReadback))
            {
                return Err(Error::new(ErrorKind::Unexpected, "injected read-back failure"));
            }
            self.inner.read(ctx, path, args)
        }

        fn write(
            &self,
            ctx: &OperationContext,
            path: &str,
            args: OpWrite,
        ) -> opendal::Result<Self::Writer> {
            let inner = self.inner.write(ctx, path, args)?;
            Ok(Box::new(FaultWriter { inner, path: path.to_owned(), log: self.log.clone() }))
        }

        fn delete(&self, ctx: &OperationContext) -> opendal::Result<Self::Deleter> {
            self.inner.delete(ctx)
        }

        fn list(
            &self,
            ctx: &OperationContext,
            path: &str,
            args: OpList,
        ) -> opendal::Result<Self::Lister> {
            self.inner.list(ctx, path, args)
        }

        fn copy(
            &self,
            ctx: &OperationContext,
            from: &str,
            to: &str,
            args: OpCopy,
        ) -> opendal::Result<Self::Copier> {
            self.inner.copy(ctx, from, to, args)
        }

        fn compose(
            &self,
            ctx: &OperationContext,
            to: &str,
            args: OpCompose,
        ) -> opendal::Result<Self::Composer> {
            self.inner.compose(ctx, to, args)
        }

        async fn rename(
            &self,
            ctx: &OperationContext,
            from: &str,
            to: &str,
            args: OpRename,
        ) -> opendal::Result<RpRename> {
            self.inner.rename(ctx, from, to, args).await
        }

        async fn restore(
            &self,
            ctx: &OperationContext,
            path: &str,
            args: OpRestore,
        ) -> opendal::Result<RpRestore> {
            self.inner.restore(ctx, path, args).await
        }

        async fn presign(
            &self,
            ctx: &OperationContext,
            path: &str,
            args: OpPresign,
        ) -> opendal::Result<RpPresign> {
            self.inner.presign(ctx, path, args).await
        }
    }

    struct FaultWriter {
        inner: raw::oio::Writer,
        path: String,
        log: Arc<FaultLog>,
    }

    impl raw::oio::Write for FaultWriter {
        async fn write(&mut self, buffer: Buffer) -> opendal::Result<()> {
            if FaultLog::is_artifact_path(&self.path)
                && self.log.claim(|action| matches!(action, FaultAction::CorruptArtifact))
            {
                let mut bytes = buffer.to_vec();
                if let Some(first) = bytes.first_mut() {
                    *first ^= 0x01;
                }
                return self.inner.write(Buffer::from(bytes)).await;
            }
            self.inner.write(buffer).await
        }

        async fn copy_from(
            &mut self,
            path: &str,
            args: OpRead,
            range: opendal::BytesRange,
        ) -> opendal::Result<()> {
            self.inner.copy_from(path, args, range).await
        }

        async fn close(&mut self) -> opendal::Result<opendal::Metadata> {
            if self.path.starts_with("_generations/")
                && self.path.ends_with("/manifest.json")
                && self.log.claim(|action| matches!(action, FaultAction::FailGenerationManifest))
            {
                let _ = self.inner.abort().await;
                return Err(Error::new(
                    ErrorKind::Unexpected,
                    "injected generation manifest write failure",
                ));
            }
            if self.path == "frame.bin.manifest.json"
                && self.log.claim(|action| matches!(action, FaultAction::LosePointerBeforeWrite))
            {
                let _ = self.inner.abort().await;
                return Err(Error::new(
                    ErrorKind::Unexpected,
                    "injected loss before pointer write",
                ));
            }
            let metadata = self.inner.close().await?;
            self.log.record(format!("close:{}", self.path));
            if self.path == "frame.bin.manifest.json"
                && self.log.claim(|action| matches!(action, FaultAction::LosePointerAfterWrite))
            {
                return Err(Error::new(ErrorKind::Unexpected, "injected loss after pointer write"));
            }
            if self.path == "frame.bin.manifest.json" {
                let _ = self.log.claim(|action| {
                    if let FaultAction::CancelAfterPointerWrite(token) = action {
                        token.cancel();
                        true
                    } else {
                        false
                    }
                });
            }
            if self.path.starts_with("_generations/") && self.path.ends_with("/manifest.json") {
                let _ = self.log.claim(|action| {
                    if let FaultAction::CancelAfterGenerationManifest(token) = action {
                        token.cancel();
                        true
                    } else {
                        false
                    }
                });
            }
            Ok(metadata)
        }

        async fn abort(&mut self) -> opendal::Result<()> {
            self.inner.abort().await
        }
    }
}
