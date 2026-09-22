from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
import radiust.pipeline as pipeline
from radiust import Client, Query
from radiust.cache import CacheStore
from radiust.config import load_config
from radiust.identity import cache_key
from radiust.models import Artifact, FrameRef
from radiust.raw import RawFrame
from radiust.raw_replay import load_raw, replay_decode

QUERY = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))


def test_raw_only_skips_decode_and_can_be_replayed(tmp_path: Path) -> None:
    with Client() as client:
        report = client.download(QUERY, output=tmp_path, raw_only=True)
    assert report.counts["written"] == 1
    assert not list(tmp_path.rglob("*.nc"))
    raw_manifests = list(tmp_path.rglob("raw-manifest.json"))
    assert len(raw_manifests) == 1
    raw = load_raw(raw_manifests[0])
    try:
        assert raw.bytes()
    finally:
        raw.close()
    field = replay_decode(raw_manifests[0])
    assert field.data.shape[0] > 0


def test_decoded_output_can_be_completed_with_raw(tmp_path: Path) -> None:
    with Client() as client:
        first = client.download(QUERY, output=tmp_path)
        manifest = next(tmp_path.rglob("*.manifest.json"))
        first_manifest = json.loads(manifest.read_text())
        second = client.download(QUERY, output=tmp_path, raw=True)
    assert first.counts["written"] == 1
    assert second.counts["written"] == 1
    second_manifest = json.loads(manifest.read_text())
    assert second_manifest["raw_complete"] is True
    assert second_manifest["revision"] == first_manifest["revision"]
    assert second_manifest["output_id"] == first_manifest["output_id"]
    assert first.items[0].output_uri == second.items[0].output_uri


class _RevisionedTileSource:
    calls: list[str | None]

    def __init__(self) -> None:
        self.calls = []

    async def download(self, ref: FrameRef, _context) -> RawFrame:
        self.calls.append(ref.revision)
        revision = ref.revision or "missing"
        return RawFrame(
            ref,
            (Artifact("tile.png", "tile", "image/png", f"tile:{revision}".encode(), source_revision=revision),),
        )

    def decode(self, _raw, _context):
        raise AssertionError("raw-only and raw-tile acquisition must not decode a mosaic")


def test_mosaic_only_cache_refetches_raw_tiles_and_never_mixes_revisions(tmp_path: Path, monkeypatch) -> None:
    source = _RevisionedTileSource()
    monkeypatch.setattr(pipeline, "get_source", lambda _source_id: source)
    cache_dir = tmp_path / "cache"
    output_dir = tmp_path / "formal"
    config = load_config(
        {"cache": {"dir": str(cache_dir)}, "storage": {"output": str(output_dir)}},
        environ={},
    )
    first_ref = FrameRef(
        "tile-cache-test",
        "reflectivity",
        datetime(2026, 1, 1, tzinfo=timezone.utc),
        station="global",
        locator={"frame": "same"},
        revision="revision-1",
    )
    next_ref = FrameRef(
        "tile-cache-test",
        "reflectivity",
        first_ref.valid_time,
        station="global",
        locator={"frame": "same"},
        revision="revision-2",
    )
    cache = CacheStore(cache_dir, output_root=output_dir)
    cache.put(cache_key(first_ref, first_ref.revision), b"cached mosaic only", kind="mosaic", validator="revision-1")

    with Client(config=config) as client:
        with client.acquire(first_ref) as first_raw:
            assert first_raw.bytes() == b"tile:revision-1"
            assert first_raw.artifacts[0].source_revision == "revision-1"
        with client.acquire(next_ref) as next_raw:
            assert next_raw.bytes() == b"tile:revision-2"
            assert next_raw.artifacts[0].source_revision == "revision-2"

    assert source.calls == ["revision-1", "revision-2"]


def test_raw_only_skips_decode_and_scientific_processing(tmp_path: Path, monkeypatch) -> None:
    source = _RevisionedTileSource()
    monkeypatch.setattr(pipeline, "get_source", lambda _source_id: source)
    monkeypatch.setattr(pipeline, "_apply_processing", lambda *_args, **_kwargs: pytest.fail("processing called for raw-only"))
    ref = FrameRef(
        "raw-only-test",
        "reflectivity",
        datetime(2026, 1, 1, tzinfo=timezone.utc),
        station="global",
        revision="revision-1",
    )
    config = load_config({"cache": {"enabled": False}}, environ={})

    with Client(config=config) as client:
        report = client.download([ref], output=tmp_path / "output", raw_only=True)

    assert report.counts["written"] == 1
    assert source.calls == ["revision-1"]
    assert len(list((tmp_path / "output").rglob("raw-manifest.json"))) == 1


def test_clearing_cache_does_not_remove_formal_raw_artifacts(tmp_path: Path) -> None:
    output_dir = tmp_path / "output"
    cache_dir = tmp_path / "cache"
    config = load_config(
        {"cache": {"dir": str(cache_dir)}, "storage": {"output": str(output_dir)}},
        environ={},
    )
    with Client(config=config) as client:
        report = client.download(QUERY, output=output_dir, raw=True)

    assert report.counts["written"] == 1
    raw_path = next(output_dir.rglob("raw-manifest.json"))
    before = load_raw(raw_path)
    try:
        payload = before.bytes()
    finally:
        before.close()

    CacheStore(cache_dir, output_root=output_dir).clear()

    after = load_raw(raw_path)
    try:
        assert after.bytes() == payload
    finally:
        after.close()
