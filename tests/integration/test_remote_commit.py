from __future__ import annotations

import asyncio
import hashlib
import json

import pytest
from radiust.errors import IntegrityError, OutputConflict
from radiust.identity import digest
from radiust.storage.commit import (
    CommitOutcomeUnknown,
    InMemoryRemoteBackend,
    RemoteCommitRequest,
    RemoteCommitter,
)


def _request(*, overwrite: bool = False, payload: bytes = b"frame-data") -> RemoteCommitRequest:
    processing_spec = {"schema_version": 1, "output_kind": "decoded", "format": "netcdf"}
    return RemoteCommitRequest(
        pointer_key="source=my/frame.manifest.json",
        logical_id=hashlib.sha256(b"logical").hexdigest(),
        revision=hashlib.sha256(b"revision").hexdigest(),
        output_id=hashlib.sha256(b"output").hexdigest(),
        processing_spec=processing_spec,
        processing_hash=digest(processing_spec),
        artifacts={"frame.nc": payload},
        overwrite=overwrite,
    )


class _FailGenerationArtifactBackend(InMemoryRemoteBackend):
    async def put(self, key: str, payload: bytes, *, overwrite: bool) -> None:
        if key.startswith("_generations/") and key.endswith("/frame.nc"):
            raise OSError("injected generation artifact write failure")
        await super().put(key, payload, overwrite=overwrite)


class _CorruptReadbackBackend(InMemoryRemoteBackend):
    async def get(self, key: str) -> bytes | None:
        payload = await super().get(key)
        if key.startswith("_generations/") and key.endswith("/frame.nc") and payload is not None:
            return payload + b"corruption"
        return payload


class _LoseGenerationManifestResponse(InMemoryRemoteBackend):
    async def put(self, key: str, payload: bytes, *, overwrite: bool) -> None:
        await super().put(key, payload, overwrite=overwrite)
        if key.startswith("_generations/") and key.endswith("/manifest.json"):
            raise TimeoutError("injected response loss after generation manifest write")


@pytest.mark.asyncio
async def test_remote_commit_is_manifest_last_and_overwrite_keeps_history() -> None:
    backend = InMemoryRemoteBackend()
    committer = RemoteCommitter(backend)

    first = await committer.commit(_request(payload=b"old-frame-data"))
    second = await committer.commit(_request(overwrite=True, payload=b"new-frame-data"))

    assert first.status == "committed"
    assert second.status == "committed"
    assert first.generation != second.generation
    pointer = json.loads((await backend.get("source=my/frame.manifest.json")).decode())
    assert pointer["generation"] == second.generation
    assert pointer["supersedes"]["generation"] == first.generation
    assert pointer["artifacts"][0]["sha256"] == hashlib.sha256(b"new-frame-data").hexdigest()
    first_artifact = first.manifest.artifacts[0]["relative_uri"]
    second_artifact = second.manifest.artifacts[0]["relative_uri"]
    assert await backend.get(first_artifact) == b"old-frame-data"
    assert await backend.get(second_artifact) == b"new-frame-data"


@pytest.mark.asyncio
async def test_cancelled_commit_never_publishes_pointer() -> None:
    backend = InMemoryRemoteBackend()
    committer = RemoteCommitter(backend)
    calls = 0

    def cancel_before_pointer() -> None:
        nonlocal calls
        calls += 1
        if calls >= 2:
            raise asyncio.CancelledError

    with pytest.raises(asyncio.CancelledError):
        await committer.commit(_request(), cancellation=cancel_before_pointer)

    assert await backend.get("source=my/frame.manifest.json") is None


@pytest.mark.asyncio
async def test_lost_pointer_response_is_reconciled_when_manifest_is_visible() -> None:
    backend = InMemoryRemoteBackend(lose_response_after_write={"source=my/frame.manifest.json"})

    result = await RemoteCommitter(backend).commit(_request())

    assert result.status == "committed"


@pytest.mark.asyncio
async def test_lost_pointer_response_without_visibility_is_unknown() -> None:
    backend = InMemoryRemoteBackend(lose_response_before_write={"source=my/frame.manifest.json"})

    with pytest.raises(CommitOutcomeUnknown, match="commit outcome is unknown"):
        await RemoteCommitter(backend).commit(_request())


@pytest.mark.asyncio
async def test_generation_stage_failure_never_publishes_pointer() -> None:
    backend = _FailGenerationArtifactBackend()

    with pytest.raises(OSError, match="generation artifact write failure"):
        await RemoteCommitter(backend).commit(_request())

    assert await backend.get("source=my/frame.manifest.json") is None


@pytest.mark.asyncio
async def test_generation_readback_mismatch_never_publishes_pointer() -> None:
    backend = _CorruptReadbackBackend()

    with pytest.raises(IntegrityError, match="verification failed"):
        await RemoteCommitter(backend).commit(_request())

    assert await backend.get("source=my/frame.manifest.json") is None


@pytest.mark.asyncio
async def test_generation_manifest_response_loss_never_publishes_pointer() -> None:
    backend = _LoseGenerationManifestResponse()

    with pytest.raises(TimeoutError, match="generation manifest write"):
        await RemoteCommitter(backend).commit(_request())

    assert await backend.get("source=my/frame.manifest.json") is None
    assert any(key.startswith("_generations/") and key.endswith("/manifest.json") for key in backend.objects)


@pytest.mark.asyncio
@pytest.mark.parametrize("damage", ["missing", "same_size_corruption", "wrong_size"])
async def test_retry_repairs_incomplete_published_artifacts(damage: str) -> None:
    backend = InMemoryRemoteBackend()
    committer = RemoteCommitter(backend)
    first = await committer.commit(_request())
    artifact_key = first.manifest.artifacts[0]["relative_uri"]
    if damage == "missing":
        del backend.objects[artifact_key]
    elif damage == "same_size_corruption":
        backend.objects[artifact_key] = b"frame-datA"
    else:
        backend.objects[artifact_key] = b"short"

    repaired = await committer.commit(_request())

    assert repaired.status == "committed"
    assert repaired.generation == first.generation
    assert await backend.get(_request().pointer_key) == json.dumps(first.manifest.as_dict(), ensure_ascii=False, sort_keys=True, indent=2).encode("utf-8")
    repaired_key = repaired.manifest.artifacts[0]["relative_uri"]
    assert await backend.get(repaired_key) == b"frame-data"
    assert (await committer.commit(_request())).status == "skipped"


@pytest.mark.asyncio
async def test_concurrent_repair_does_not_replace_published_pointer() -> None:
    backend = InMemoryRemoteBackend()
    committer = RemoteCommitter(backend)
    first = await committer.commit(_request())
    pointer_before = await backend.get(_request().pointer_key)
    del backend.objects[first.manifest.artifacts[0]["relative_uri"]]

    results = await asyncio.gather(committer.commit(_request()), committer.commit(_request()))

    assert all(item.generation == first.generation for item in results)
    assert await backend.get(_request().pointer_key) == pointer_before
    assert await backend.get(first.manifest.artifacts[0]["relative_uri"]) == b"frame-data"


@pytest.mark.asyncio
async def test_repair_refuses_different_bytes_without_overwrite() -> None:
    backend = InMemoryRemoteBackend()
    committer = RemoteCommitter(backend)
    first = await committer.commit(_request())
    del backend.objects[first.manifest.artifacts[0]["relative_uri"]]

    with pytest.raises(OutputConflict, match="does not match published manifest"):
        await committer.commit(_request(payload=b"different-data"))

    assert (await backend.get(_request().pointer_key)) is not None
    assert await backend.get(first.manifest.artifacts[0]["relative_uri"]) is None


@pytest.mark.asyncio
async def test_complete_remote_output_is_skipped_after_artifact_verification() -> None:
    backend = InMemoryRemoteBackend()
    committer = RemoteCommitter(backend)
    first = await committer.commit(_request())
    second = await committer.commit(_request())
    assert second.status == "skipped"
    assert second.generation == first.generation
