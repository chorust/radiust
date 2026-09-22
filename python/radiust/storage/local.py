"""Manifest-last local storage with an advisory root lock."""

from __future__ import annotations

import hashlib
import json
import os
import secrets
import shutil
import tempfile
import uuid
from collections.abc import Callable
from dataclasses import dataclass
from dataclasses import field as dc_field
from pathlib import Path
from typing import Any

from ..errors import OutputConflict, OutputLockedError, StorageError
from ..identity import artifact_bytes
from ..models import Artifact, FrameRef, ProcessingSpec
from .manifest import Manifest, inspect_manifest, load_manifest, new_manifest
from .naming import output_location


@dataclass
class StagedGroup:
    root: Path
    directory: Path
    final_path: Path
    output_id: str
    files: list[dict[str, Any]] = dc_field(default_factory=list)


def local_output_path(root: str | Path) -> Path:
    """Resolve a local output root and reject URI targets until an adapter exists."""
    value = os.fspath(root)
    if value.lower().startswith(("s3://", "oss://", "http://", "https://")):
        raise StorageError("remote output targets require an installed storage adapter; use a local path")
    return Path(root).expanduser().resolve()


class LocalStore:
    def __init__(self, root: str | Path) -> None:
        self.root = local_output_path(root)

    def stage(self, ref: FrameRef, output_id: str, spec: ProcessingSpec, *, template: str | None = None) -> StagedGroup:
        self.root.mkdir(parents=True, exist_ok=True)
        directory = Path(tempfile.mkdtemp(prefix="radiust-stage-", dir=self.root))
        final = output_location(self.root, ref, output_id, spec, template)
        return StagedGroup(self.root, directory, final, output_id)

    def write_bytes(self, staged: StagedGroup, name: str, payload: bytes, *, role: str = "data", media_type: str = "application/octet-stream") -> Path:
        if Path(name).name != name or name in {"", ".", ".."}:
            raise StorageError("artifact name must be a simple file name")
        target = staged.directory / name
        target.write_bytes(payload)
        staged.files.append({"path": target, "name": name, "role": role, "media_type": media_type})
        return target

    def write_path(self, staged: StagedGroup, name: str, source: Path, *, role: str = "data", media_type: str = "application/octet-stream") -> Path:
        return self.write_bytes(staged, name, source.read_bytes(), role=role, media_type=media_type)

    def track_directory(self, staged: StagedGroup, name: str, source: Path, *, role: str = "data", media_type: str = "application/octet-stream") -> Path:
        if Path(name).name != name or not source.is_dir():
            raise StorageError("directory artifact must have a simple name and exist")
        staged.files.append({"path": source, "name": name, "role": role, "media_type": media_type, "directory": True})
        return source

    def _lock(self):
        lock_path = self.root / ".radiust.lock"
        handle = lock_path.open("a+")
        try:
            import fcntl

            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except (ImportError, BlockingIOError, OSError) as exc:
            handle.close()
            raise OutputLockedError(f"output root is locked: {self.root}") from exc
        return handle

    def commit(
        self,
        staged: StagedGroup,
        *,
        logical_id: str,
        revision: str,
        processing_spec: dict[str, Any],
        processing_hash: str,
        raw_complete: bool,
        raw_manifest: dict[str, Any] | None = None,
        raw_artifacts: tuple[Artifact, ...] = (),
        overwrite: bool = False,
        cancellation: Callable[[], None] | None = None,
    ) -> tuple[str, Manifest, bool]:
        lock = self._lock()
        try:
            final = staged.final_path
            manifest_path = final.with_name(final.name + ".manifest.json")
            current: dict[str, Any] | None = None
            base_artifacts: list[dict[str, Any]] = []
            supersedes: dict[str, Any] | None = None
            if manifest_path.exists() and inspect_manifest(manifest_path) == "complete":
                current = json.loads(manifest_path.read_text(encoding="utf-8"))
                supersedes = {
                    "output_id": current.get("output_id"),
                    "generation": current.get("generation"),
                    "manifest": manifest_path.name,
                }
                if not overwrite and current.get("output_id") == staged.output_id and current.get("raw_complete", False) >= raw_complete:
                    current_manifest = load_manifest(manifest_path)
                    self.abort(staged)
                    return "skipped", current_manifest, True
                if current.get("output_id") == staged.output_id and raw_complete and not current.get("raw_complete", False):
                    # A decoded result may be completed with the original raw artifacts.
                    overwrite = True
                    base_artifacts = [dict(item) for item in current.get("artifacts", [])]
                if not overwrite:
                    raise OutputConflict(f"complete output already exists: {manifest_path}")

            # Read and hash every staged byte before withdrawing a previous
            # commit marker. A staging/verification failure therefore leaves
            # the old complete group untouched.
            staged_artifacts: list[dict[str, Any]] = base_artifacts
            moves: list[tuple[Path, Path]] = []
            for file_info in staged.files:
                source = Path(file_info["path"])
                name = str(file_info["name"])
                if Path(name).name != name or name in {"", ".", ".."}:
                    raise StorageError("artifact name must be a simple file name")
                target = final.parent / name
                if file_info.get("directory"):
                    if source.is_symlink() or not source.is_dir():
                        raise StorageError("staged directory artifact is missing or is a symlink")
                    children = [child for child in sorted(source.rglob("*")) if child.is_file()]
                    if not children:
                        raise StorageError("staged directory artifact is empty")
                    moves.append((source, target))
                    for child in children:
                        if child.is_symlink():
                            raise StorageError("staged directory contains a symlink")
                        data = child.read_bytes()
                        relative = (Path(name) / child.relative_to(source)).as_posix()
                        staged_artifacts.append({"name": relative, "relative_uri": relative, "role": file_info["role"], "media_type": file_info["media_type"], "size_bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
                else:
                    if source.is_symlink() or not source.is_file():
                        raise StorageError("staged file artifact is missing or is a symlink")
                    moves.append((source, target))
                    data = source.read_bytes()
                    staged_artifacts.append({"name": name, "relative_uri": name, "role": file_info["role"], "media_type": file_info["media_type"], "size_bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})

            raw_stage: Path | None = None
            if raw_manifest is not None:
                raw_stage = staged.directory / "raw" / final.stem
                raw_stage.mkdir(parents=True, exist_ok=True)
                for artifact in raw_artifacts:
                    raw_target = raw_stage / artifact.name
                    raw_target.write_bytes(artifact_bytes(artifact))
                    raw_bytes = raw_target.read_bytes()
                    staged_artifacts.append({"name": f"raw/{final.stem}/{artifact.name}", "relative_uri": f"raw/{final.stem}/{artifact.name}", "role": "data", "media_type": artifact.media_type, "size_bytes": len(raw_bytes), "sha256": hashlib.sha256(raw_bytes).hexdigest()})
                (raw_stage / "raw-manifest.json").write_text(json.dumps(raw_manifest, ensure_ascii=False, sort_keys=True, indent=2), encoding="utf-8")
                raw_bytes = (raw_stage / "raw-manifest.json").read_bytes()
                staged_artifacts.append({"name": "raw-manifest.json", "relative_uri": f"raw/{final.stem}/raw-manifest.json", "role": "metadata", "media_type": "application/json", "size_bytes": len(raw_bytes), "sha256": hashlib.sha256(raw_bytes).hexdigest()})
            # A raw supplement may rewrite the decoded file while retaining
            # previous manifest entries. Keep the latest receipt per artifact.
            staged_artifacts = list({str(item["name"]): item for item in staged_artifacts}.values())
            manifest = new_manifest(logical_id=logical_id, revision=revision, output_id=staged.output_id, processing_spec=processing_spec, processing_hash=processing_hash, artifacts=staged_artifacts, raw_complete=raw_complete, generation=uuid.uuid4().hex, supersedes=supersedes)

            # Stage and flush the new manifest before mutating any published
            # files. It remains invisible until the final atomic rename.
            temp_manifest = staged.directory / f".radiust-manifest-{secrets.token_hex(6)}.tmp"
            temp_manifest.write_text(json.dumps(manifest.as_dict(), ensure_ascii=False, sort_keys=True, indent=2), encoding="utf-8")
            with temp_manifest.open("rb") as handle:
                os.fsync(handle.fileno())

            if cancellation is not None:
                cancellation()
            final.parent.mkdir(parents=True, exist_ok=True)

            # Remove the public marker from its final location before replacing
            # any of its artifacts. A reader can see the old complete group, no
            # group, or the new complete group, never an old marker over new bytes.
            if manifest_path.exists():
                os.replace(manifest_path, staged.directory / ".previous-manifest.json")

            if current is not None and current.get("output_id") != staged.output_id:
                for item in current.get("artifacts", []):
                    relative = Path(str(item.get("relative_uri", "")))
                    if relative.is_absolute() or ".." in relative.parts:
                        raise StorageError("existing manifest contains an unsafe artifact path")
                    old_path = final.parent / relative
                    if old_path.is_dir():
                        shutil.rmtree(old_path)
                    else:
                        old_path.unlink(missing_ok=True)
            elif final.exists() and current is None:
                # A missing or invalid manifest makes this group repairable.
                if final.is_dir():
                    shutil.rmtree(final)
                else:
                    final.unlink()

            for source, target in moves:
                if target.exists():
                    if target.is_dir():
                        shutil.rmtree(target)
                    else:
                        target.unlink()
                target.parent.mkdir(parents=True, exist_ok=True)
                os.replace(source, target)

            if raw_stage is not None:
                raw_target_root = final.parent / "raw" / final.stem
                if raw_target_root.exists():
                    shutil.rmtree(raw_target_root)
                raw_target_root.parent.mkdir(parents=True, exist_ok=True)
                os.replace(raw_stage, raw_target_root)

            # Verify the bytes at their published paths before the manifest
            # becomes visible. The manifest is the single commit marker.
            for item in manifest.artifacts:
                relative = Path(str(item["relative_uri"]))
                target = final.parent / relative
                if relative.is_absolute() or ".." in relative.parts or not target.is_file():
                    raise StorageError("published artifact is missing or has an unsafe path")
                data = target.read_bytes()
                if len(data) != int(item["size_bytes"]) or hashlib.sha256(data).hexdigest() != item["sha256"]:
                    raise StorageError(f"published artifact verification failed: {item['name']}")

            if cancellation is not None:
                cancellation()
            try:
                os.replace(temp_manifest, manifest_path)
            except OSError:
                try:
                    visible = load_manifest(manifest_path)
                    if inspect_manifest(manifest_path) == "complete" and visible.generation == manifest.generation and visible.output_id == staged.output_id:
                        shutil.rmtree(staged.directory, ignore_errors=True)
                        return "written", visible, False
                except Exception:
                    pass
                raise
            shutil.rmtree(staged.directory, ignore_errors=True)
            return "written", manifest, False
        finally:
            try:
                import fcntl

                fcntl.flock(lock.fileno(), fcntl.LOCK_UN)
            finally:
                lock.close()

    def abort(self, staged: StagedGroup) -> None:
        shutil.rmtree(staged.directory, ignore_errors=True)
