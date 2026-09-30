from __future__ import annotations

import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
from radiust._bridge import CoreEngineSession
from radiust.identity import safe_ref
from radiust.models import FrameRef


@pytest.mark.asyncio
async def test_rust_binding_loads_verified_raw_bytes_and_receipts(tmp_path: Path) -> None:
    payload = b"retained source bytes"
    ref = FrameRef(
        "my",
        "composite",
        datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc),
        station="peninsular",
        locator={"fixture": True},
        revision="fixture-revision",
    )
    artifact_root = tmp_path / "raw"
    artifact_root.mkdir()
    (artifact_root / "frame.bin").write_bytes(payload)
    manifest = {
        "schema_version": 1,
        "ref": safe_ref(ref),
        "artifacts": [
            {
                "name": "frame.bin",
                "role": "data",
                "media_type": "application/octet-stream",
                "size_bytes": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
                "source_revision": "fixture-revision",
            }
        ],
        "metadata": {"fixture": True},
        "raw_complete": True,
    }
    manifest_path = tmp_path / "raw-manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    staging = tmp_path / "staging"
    session = CoreEngineSession(
        {"runtime": {"allow_network": False, "temp_root": str(staging)}}
    )

    raw = await session.load_raw_manifest(manifest_path)
    try:
        receipt = json.loads(raw.receipt_json())
        artifact = json.loads(raw.artifact_json(0))
        assert receipt["frame"]["source"] == "my"
        assert receipt["frame"]["logical_id"] == ref.logical_id
        assert raw.artifact_count == 1
        assert raw.artifact_bytes(0) == payload
        assert artifact["name"] == "frame.bin"
        assert artifact["sha256"] == hashlib.sha256(payload).hexdigest()
        assert len(list(staging.iterdir())) == 1
    finally:
        raw.close()
        session.cancel()

    assert list(staging.iterdir()) == []
