"""Remote generation commit protocol with an explicit publish fence."""

from __future__ import annotations

import asyncio
import hashlib
import json
import uuid
from collections.abc import Callable, Mapping
from contextlib import suppress
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Protocol

from ..errors import CommitOutcomeUnknown, IntegrityError, OutputConflict, StorageError
from .manifest import Manifest, new_manifest


class RemoteBackend(Protocol):
    async def put(self, key: str, payload: bytes, *, overwrite: bool) -> None: ...

    async def get(self, key: str) -> bytes | None: ...


@dataclass(frozen=True, slots=True)
class RemoteArtifact:
    payload: bytes
    role: str = "data"
    media_type: str = "application/octet-stream"


@dataclass(frozen=True, slots=True)
class RemoteCommitRequest:
    pointer_key: str
    logical_id: str
    revision: str
    output_id: str
    processing_spec: Mapping[str, object]
    processing_hash: str
    artifacts: Mapping[str, bytes | RemoteArtifact]
    raw_complete: bool = False
    overwrite: bool = False


@dataclass(frozen=True, slots=True)
class RemoteCommitResult:
    status: str
    generation: str
    pointer_key: str
    manifest: Manifest


class RemoteCommitter:
    def __init__(self, backend: RemoteBackend) -> None:
        self.backend = backend

    async def commit(
        self,
        request: RemoteCommitRequest,
        *,
        cancellation: Callable[[], None] | None = None,
    ) -> RemoteCommitResult:
        _check_fence(cancellation)
        pointer_key = _safe_key(request.pointer_key)
        current_bytes = await self.backend.get(pointer_key)
        current = _decode_manifest(current_bytes) if current_bytes is not None else None
        supplementing_raw = False
        if current is not None and not request.overwrite:
            if current.get("output_id") == request.output_id:
                supplementing_raw = request.raw_complete and not bool(current.get("raw_complete", False))
                if not supplementing_raw:
                    manifest = _manifest_from_dict(current)
                    if await self._artifacts_complete(manifest):
                        return RemoteCommitResult("skipped", str(current["generation"]), pointer_key, manifest)
                    # The committed manifest is the integrity contract. Restore
                    # only its declared bytes; never overwrite the pointer as a
                    # side effect of a create-only retry.
                    await self._repair_artifacts(request, manifest)
                    if await self.backend.get(pointer_key) != current_bytes:
                        raise OutputConflict(f"remote pointer changed during repair: {pointer_key}")
                    return RemoteCommitResult("committed", str(current["generation"]), pointer_key, manifest)
            else:
                raise OutputConflict(f"complete remote output already exists: {pointer_key}")

        generation = uuid.uuid4().hex
        generation_prefix = f"_generations/{request.output_id}/{generation}"
        artifact_descriptors: list[dict[str, object]] = []
        for name, value in sorted(request.artifacts.items()):
            relative_name = _safe_key(name)
            artifact = value if isinstance(value, RemoteArtifact) else RemoteArtifact(value)
            object_key = f"{generation_prefix}/{relative_name}"
            await self.backend.put(object_key, bytes(artifact.payload), overwrite=False)
            stored = await self.backend.get(object_key)
            if stored is None:
                raise IntegrityError(f"remote artifact disappeared after write: {relative_name}")
            digest = hashlib.sha256(stored).hexdigest()
            expected = hashlib.sha256(artifact.payload).hexdigest()
            if stored != artifact.payload or digest != expected:
                raise IntegrityError(f"remote artifact verification failed: {relative_name}")
            artifact_descriptors.append(
                {
                    "name": relative_name,
                    "relative_uri": object_key,
                    "role": artifact.role,
                    "media_type": artifact.media_type,
                    "size_bytes": len(stored),
                    "sha256": digest,
                }
            )

        supersedes = None
        if current is not None:
            supersedes = {
                "output_id": current.get("output_id"),
                "generation": current.get("generation"),
                "pointer_key": pointer_key,
            }
        manifest = new_manifest(
            logical_id=request.logical_id,
            revision=request.revision,
            output_id=request.output_id,
            processing_spec=request.processing_spec,
            processing_hash=request.processing_hash,
            artifacts=artifact_descriptors,
            raw_complete=request.raw_complete,
            generation=generation,
            supersedes=supersedes,
        )
        generation_manifest_key = f"{generation_prefix}/manifest.json"
        manifest_bytes = _manifest_bytes(manifest)
        await self.backend.put(generation_manifest_key, manifest_bytes, overwrite=False)
        if await self.backend.get(generation_manifest_key) != manifest_bytes:
            raise IntegrityError("remote generation manifest failed read-back verification")

        # This is the linearization point.  No cancellation is accepted after
        # this check without reconciling the pointer result below.
        _check_fence(cancellation)
        try:
            await self.backend.put(pointer_key, manifest_bytes, overwrite=request.overwrite or supplementing_raw)
        except BaseException as exc:
            try:
                visible = await self.backend.get(pointer_key)
            except BaseException as probe_error:
                raise CommitOutcomeUnknown(
                    f"commit outcome is unknown for {pointer_key}: {probe_error}", cause=exc
                ) from exc
            if visible == manifest_bytes:
                return RemoteCommitResult("committed", generation, pointer_key, manifest)
            if isinstance(exc, asyncio.CancelledError):
                raise
            raise CommitOutcomeUnknown(f"commit outcome is unknown for {pointer_key}", cause=exc) from exc
        return RemoteCommitResult("committed", generation, pointer_key, manifest)

    async def _artifacts_complete(self, manifest: Manifest) -> bool:
        for artifact in manifest.artifacts:
            key = _safe_key(str(artifact["relative_uri"]))
            stored = await self.backend.get(key)
            if stored is None or len(stored) != int(artifact["size_bytes"]):
                return False
            if hashlib.sha256(stored).hexdigest() != artifact["sha256"]:
                return False
        return True

    async def _repair_artifacts(self, request: RemoteCommitRequest, manifest: Manifest) -> None:
        supplied = {
            _safe_key(name): value if isinstance(value, RemoteArtifact) else RemoteArtifact(value)
            for name, value in request.artifacts.items()
        }
        declared = {str(item["name"]): item for item in manifest.artifacts}
        if supplied.keys() != declared.keys():
            raise OutputConflict("repair artifacts do not match published manifest; explicit overwrite required")
        for name, descriptor in declared.items():
            artifact = supplied[name]
            payload = bytes(artifact.payload)
            if (
                len(payload) != int(descriptor["size_bytes"])
                or hashlib.sha256(payload).hexdigest() != descriptor["sha256"]
                or artifact.role != descriptor["role"]
                or artifact.media_type != descriptor["media_type"]
            ):
                raise OutputConflict("repair payload does not match published manifest; explicit overwrite required")

        for name, descriptor in declared.items():
            key = _safe_key(str(descriptor["relative_uri"]))
            payload = bytes(supplied[name].payload)
            current = await self.backend.get(key)
            if current is not None and current == payload:
                continue
            # A concurrent repair may already have created this same object.
            with suppress(OutputConflict):
                await self.backend.put(key, payload, overwrite=current is not None)
            restored = await self.backend.get(key)
            if restored != payload:
                raise IntegrityError(f"remote artifact repair verification failed: {name}")

        if not await self._artifacts_complete(manifest):
            raise IntegrityError("remote artifacts became incomplete during repair")


class InMemoryRemoteBackend:
    """Deterministic backend used by contract tests and fault injection."""

    def __init__(
        self,
        *,
        lose_response_after_write: set[str] | None = None,
        lose_response_before_write: set[str] | None = None,
    ) -> None:
        self.objects: dict[str, bytes] = {}
        self.lose_response_after_write = lose_response_after_write or set()
        self.lose_response_before_write = lose_response_before_write or set()

    async def put(self, key: str, payload: bytes, *, overwrite: bool) -> None:
        if key in self.lose_response_before_write:
            raise TimeoutError(f"response lost before write: {key}")
        if not overwrite and key in self.objects:
            raise OutputConflict(f"remote object already exists: {key}")
        self.objects[key] = bytes(payload)
        if key in self.lose_response_after_write:
            raise TimeoutError(f"response lost after write: {key}")

    async def get(self, key: str) -> bytes | None:
        return self.objects.get(key)


def _check_fence(cancellation: Callable[[], None] | None) -> None:
    if cancellation is not None:
        cancellation()


def _safe_key(value: str) -> str:
    path = PurePosixPath(value)
    if not value or path.is_absolute() or ".." in path.parts or "." in path.parts:
        raise StorageError("remote object key must be a safe relative path")
    return path.as_posix()


def _manifest_bytes(manifest: Manifest) -> bytes:
    return json.dumps(manifest.as_dict(), ensure_ascii=False, sort_keys=True, indent=2).encode("utf-8")


def _decode_manifest(payload: bytes) -> dict[str, object]:
    try:
        value = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise IntegrityError("remote pointer is not valid JSON") from exc
    if not isinstance(value, dict):
        raise IntegrityError("remote pointer must be an object")
    return value


def _manifest_from_dict(value: Mapping[str, object]) -> Manifest:
    manifest = Manifest(
        schema_version=int(value.get("schema_version", 0)),
        logical_id=str(value.get("logical_id", "")),
        revision=str(value.get("revision", "")),
        output_id=str(value.get("output_id", "")),
        processing_spec=value.get("processing_spec", {}),
        processing_hash=str(value.get("processing_hash", "")),
        artifacts=tuple(value.get("artifacts", [])),
        raw_complete=bool(value.get("raw_complete", False)),
        created_at=str(value.get("created_at", "")),
        generation=value.get("generation"),
        supersedes=value.get("supersedes"),
    )
    manifest.validate()
    return manifest


__all__ = [
    "CommitOutcomeUnknown",
    "InMemoryRemoteBackend",
    "RemoteArtifact",
    "RemoteCommitRequest",
    "RemoteCommitResult",
    "RemoteCommitter",
]
