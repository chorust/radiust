from __future__ import annotations

import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
from radiust import Client, Query
from radiust.errors import IntegrityError
from radiust.raw_replay import load_raw, replay_decode

QUERY = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))


def _write_raw_manifest(
    directory: Path,
    *,
    artifact_name: str = "frame.bin",
    payload: bytes = b"verified raw frame",
    sha256: str | None = None,
    size_bytes: int | None = None,
    raw_complete: bool = True,
) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    artifact_path = directory / artifact_name
    if Path(artifact_name).name == artifact_name and not artifact_path.exists():
        artifact_path.write_bytes(payload)
    document = {
        "schema_version": 1,
        "ref": {
            "source": "my",
            "product": "composite",
            "station": "peninsular",
            "valid_time": "2025-12-29T06:50:01.000000Z",
            "base_time": None,
            "locator": {"fixture": True},
            "locator_version": "1",
            "revision": "fixture-revision",
        },
        "artifacts": [
            {
                "name": artifact_name,
                "role": "data",
                "media_type": "application/octet-stream",
                "size_bytes": len(payload) if size_bytes is None else size_bytes,
                "sha256": hashlib.sha256(payload).hexdigest() if sha256 is None else sha256,
                "source_revision": "fixture-revision",
            }
        ],
        "metadata": {"fixture": True},
        "raw_complete": raw_complete,
    }
    manifest_path = directory / "raw-manifest.json"
    manifest_path.write_text(json.dumps(document), encoding="utf-8")
    return manifest_path


def test_load_raw_restores_only_hash_verified_artifacts(tmp_path: Path) -> None:
    payload = b"the retained source bytes"
    manifest_path = _write_raw_manifest(tmp_path / "verified", payload=payload)

    raw = load_raw(manifest_path)
    try:
        assert raw.ref.source == "my"
        assert raw.ref.product == "composite"
        assert raw.bytes("frame.bin") == payload
        assert raw.receipts[0].sha256 == hashlib.sha256(payload).hexdigest()
    finally:
        raw.close()


def test_load_raw_rejects_receipt_hash_or_size_mismatch(tmp_path: Path) -> None:
    payload = b"the retained source bytes"
    manifest_path = _write_raw_manifest(
        tmp_path / "bad-receipt",
        payload=payload,
        sha256="0" * 64,
    )

    with pytest.raises(IntegrityError, match="receipt mismatch"):
        load_raw(manifest_path)

    manifest_path = _write_raw_manifest(
        tmp_path / "bad-size",
        payload=payload,
        size_bytes=len(payload) + 1,
    )
    with pytest.raises(IntegrityError, match="receipt mismatch"):
        load_raw(manifest_path)


def test_load_raw_rejects_incomplete_or_empty_manifests(tmp_path: Path) -> None:
    incomplete = _write_raw_manifest(tmp_path / "incomplete", raw_complete=False)
    with pytest.raises(IntegrityError, match="not a complete"):
        load_raw(incomplete)

    empty_dir = tmp_path / "empty"
    empty_dir.mkdir()
    empty_path = empty_dir / "raw-manifest.json"
    empty_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "ref": {"source": "my", "product": "composite", "valid_time": "2025-12-29T06:50:01Z"},
                "artifacts": [],
                "raw_complete": True,
            }
        ),
        encoding="utf-8",
    )
    with pytest.raises(IntegrityError, match="no artifacts"):
        load_raw(empty_path)


def test_load_raw_rejects_path_traversal_and_symlink_escape(tmp_path: Path) -> None:
    traversal = _write_raw_manifest(tmp_path / "traversal", artifact_name="../outside.bin")
    with pytest.raises(IntegrityError, match="simple file name"):
        load_raw(traversal)

    outside = tmp_path / "outside.bin"
    payload = b"outside file"
    outside.write_bytes(payload)
    directory = tmp_path / "symlink"
    directory.mkdir()
    (directory / "frame.bin").symlink_to(outside)
    symlink_manifest = _write_raw_manifest(directory, payload=payload)
    with pytest.raises(IntegrityError, match="outside manifest directory"):
        load_raw(symlink_manifest)


def test_load_raw_requires_manifest_filename_and_valid_json(tmp_path: Path) -> None:
    wrong_name = tmp_path / "manifest.json"
    wrong_name.write_text("{}", encoding="utf-8")
    with pytest.raises(IntegrityError, match="requires a raw-manifest.json"):
        load_raw(wrong_name)

    malformed = tmp_path / "raw-manifest.json"
    malformed.write_text("{", encoding="utf-8")
    with pytest.raises(IntegrityError, match="invalid raw manifest"):
        load_raw(malformed)


def test_replay_decode_uses_retained_local_artifacts(tmp_path: Path) -> None:
    config = {"runtime": {"allow_network": False}, "cache": {"enabled": False}}
    with Client(config=config) as client:
        report = client.download(QUERY, output=tmp_path, raw_only=True)

    assert report.counts["written"] == 1
    assert not list(tmp_path.rglob("*.nc"))
    manifest_path = next(tmp_path.rglob("raw-manifest.json"))
    field = replay_decode(manifest_path)

    assert field.provenance["source"] == "my"
    assert field.data.shape == (640, 826)
