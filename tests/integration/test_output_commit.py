from __future__ import annotations

import asyncio
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
import radiust.storage.local as local_storage
from radiust import Client, Query
from radiust.errors import OutputConflict, OutputLockedError
from radiust.identity import processing_hash, processing_identity
from radiust.models import FrameRef, ProcessingSpec
from radiust.storage.local import LocalStore
from radiust.storage.manifest import inspect_manifest

QUERY = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))


def _local_commit(store: LocalStore, staged, ref: FrameRef, payload: bytes, *, overwrite: bool = False):
    spec = ProcessingSpec()
    store.write_bytes(staged, staged.final_path.name, payload, media_type="application/x-test")
    identity = processing_identity(spec)
    return store.commit(
        staged,
        logical_id=ref.logical_id,
        revision=hashlib.sha256(b"source revision").hexdigest(),
        processing_spec=identity,
        processing_hash=processing_hash(spec),
        raw_complete=False,
        overwrite=overwrite,
    )


def test_png_commit_tracks_render_sidecar_and_is_idempotent(tmp_path: Path) -> None:
    with Client() as client:
        first = client.download(QUERY, output=tmp_path, format="png")
        second = client.download(QUERY, output=tmp_path, format="png")
    assert first.counts["written"] == 1
    assert second.counts["skipped"] == 1
    image = next(tmp_path.rglob("*.png"))
    sidecar = image.with_name(image.stem + ".render.json")
    manifest = image.with_name(image.name + ".manifest.json")
    assert sidecar.exists()
    assert inspect_manifest(manifest) == "complete"
    names = {item["name"] for item in json.loads(manifest.read_text())["artifacts"]}
    assert image.name in names and sidecar.name in names


def test_template_conflict_does_not_replace_complete_output(tmp_path: Path) -> None:
    east = Query("my", at=datetime(2025, 12, 29, 6, 52, 1, tzinfo=timezone.utc))
    with Client() as client:
        client.download(QUERY, output=tmp_path, output_template="fixed.nc")
        report = client.download(east, output=tmp_path, output_template="fixed.nc")
    assert report.counts["failed"] == 1
    assert report.items[0].error["code"] == "output_conflict"


def test_incomplete_group_is_repaired(tmp_path: Path) -> None:
    with Client() as client:
        first = client.download(QUERY, output=tmp_path)
        assert first.counts["written"] == 1
        nc = next(tmp_path.rglob("*.nc"))
        nc.unlink()
        repaired = client.download(QUERY, output=tmp_path)
    assert repaired.counts["written"] == 1
    assert next(tmp_path.rglob("*.nc")).exists()


def test_explicit_overwrite_records_a_new_generation(tmp_path: Path) -> None:
    with Client() as client:
        first = client.download(QUERY, output=tmp_path)
        manifest_path = next(tmp_path.rglob("*.manifest.json"))
        before = json.loads(manifest_path.read_text())
        second = client.download(QUERY, output=tmp_path, overwrite=True)
        after = json.loads(manifest_path.read_text())

    assert first.counts["written"] == 1
    assert second.counts["written"] == 1
    assert before["generation"]
    assert after["generation"]
    assert after["generation"] != before["generation"]
    assert after["supersedes"]["output_id"] == before["output_id"]
    assert after["supersedes"]["generation"] == before["generation"]


def test_local_manifest_detects_corruption_and_next_download_repairs(tmp_path: Path) -> None:
    with Client() as client:
        first = client.download(QUERY, output=tmp_path)
        output = Path(first.items[0].output_uri)
        manifest_path = output.with_name(output.name + ".manifest.json")
        output.write_bytes(output.read_bytes() + b"corruption")
        assert inspect_manifest(manifest_path) == "incomplete"

        repaired = client.download(QUERY, output=tmp_path)

    assert repaired.counts["written"] == 1
    assert inspect_manifest(manifest_path) == "complete"


def test_local_root_lock_rejects_a_second_writer(tmp_path: Path) -> None:
    store = LocalStore(tmp_path)
    ref = FrameRef("lock-test", "reflectivity", datetime(2026, 1, 1, tzinfo=timezone.utc), station="s0")
    output_id = hashlib.sha256(b"output").hexdigest()
    spec = ProcessingSpec()
    staged = store.stage(ref, output_id, spec)
    held_lock = store._lock()
    try:
        with pytest.raises(OutputLockedError):
            _local_commit(store, staged, ref, b"payload")
    finally:
        held_lock.close()
        store.abort(staged)


def test_short_variant_hash_collision_conflicts_on_full_identity(tmp_path: Path) -> None:
    store = LocalStore(tmp_path)
    ref = FrameRef("hash-test", "reflectivity", datetime(2026, 1, 1, tzinfo=timezone.utc), station="s0")
    spec = ProcessingSpec()
    first_id = "0123456789ab" + "1" * 52
    second_id = "0123456789ab" + "2" * 52

    first_stage = store.stage(ref, first_id, spec)
    _local_commit(store, first_stage, ref, b"first")
    second_stage = store.stage(ref, second_id, spec)
    try:
        with pytest.raises(OutputConflict):
            _local_commit(store, second_stage, ref, b"second")
    finally:
        store.abort(second_stage)


def test_local_overwrite_with_move_failure_withdraws_old_manifest(tmp_path: Path, monkeypatch) -> None:
    store = LocalStore(tmp_path)
    ref = FrameRef("commit-failure-test", "reflectivity", datetime(2026, 1, 1, tzinfo=timezone.utc), station="s0")
    output_id = hashlib.sha256(b"output").hexdigest()
    first_stage = store.stage(ref, output_id, ProcessingSpec())
    _local_commit(store, first_stage, ref, b"old-data")
    output_path = first_stage.final_path
    manifest_path = output_path.with_name(output_path.name + ".manifest.json")

    second_stage = store.stage(ref, output_id, ProcessingSpec())
    original_replace = local_storage.os.replace
    marker_states: list[bool] = []

    def fail_output_move(source, destination):
        if Path(destination) == output_path:
            marker_states.append(manifest_path.exists())
            raise OSError("injected artifact move failure")
        return original_replace(source, destination)

    monkeypatch.setattr(local_storage.os, "replace", fail_output_move)
    try:
        with pytest.raises(OSError, match="artifact move failure"):
            _local_commit(store, second_stage, ref, b"new-data", overwrite=True)
    finally:
        store.abort(second_stage)

    assert marker_states == [False]
    assert not manifest_path.exists()
    assert not output_path.exists()


def test_local_manifest_replace_response_loss_is_reconciled(tmp_path: Path, monkeypatch) -> None:
    store = LocalStore(tmp_path)
    ref = FrameRef("commit-response-test", "reflectivity", datetime(2026, 1, 1, tzinfo=timezone.utc), station="s0")
    output_id = hashlib.sha256(b"output").hexdigest()
    first_stage = store.stage(ref, output_id, ProcessingSpec())
    _local_commit(store, first_stage, ref, b"old-data")
    second_stage = store.stage(ref, output_id, ProcessingSpec())
    manifest_path = second_stage.final_path.with_name(second_stage.final_path.name + ".manifest.json")
    original_replace = local_storage.os.replace

    def lose_response(source, destination):
        original_replace(source, destination)
        if Path(destination) == manifest_path:
            raise OSError("injected manifest response loss")

    monkeypatch.setattr(local_storage.os, "replace", lose_response)
    try:
        status, manifest, skipped = _local_commit(store, second_stage, ref, b"new-data", overwrite=True)
    finally:
        store.abort(second_stage)

    assert status == "written"
    assert skipped is False
    assert manifest.generation
    assert inspect_manifest(manifest_path) == "complete"


def test_local_cancel_fence_prevents_manifest_publication(tmp_path: Path) -> None:
    store = LocalStore(tmp_path)
    ref = FrameRef("commit-cancel-test", "reflectivity", datetime(2026, 1, 1, tzinfo=timezone.utc), station="s0")
    output_id = hashlib.sha256(b"output").hexdigest()
    staged = store.stage(ref, output_id, ProcessingSpec())
    calls = 0

    def cancel_at_publish_fence() -> None:
        nonlocal calls
        calls += 1
        if calls >= 2:
            raise asyncio.CancelledError

    store.write_bytes(staged, staged.final_path.name, b"payload", media_type="application/x-test")
    spec = ProcessingSpec()
    identity = processing_identity(spec)
    try:
        with pytest.raises(asyncio.CancelledError):
            store.commit(
                staged,
                logical_id=ref.logical_id,
                revision=hashlib.sha256(b"source revision").hexdigest(),
                processing_spec=identity,
                processing_hash=processing_hash(spec),
                raw_complete=False,
                cancellation=cancel_at_publish_fence,
            )
    finally:
        store.abort(staged)

    assert not staged.final_path.with_name(staged.final_path.name + ".manifest.json").exists()
