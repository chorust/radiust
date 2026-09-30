from __future__ import annotations

import asyncio
import hashlib
import json
from datetime import datetime, timezone

from radiust._bridge import CoreEngineSession
from radiust.config import load_config
from radiust.identity import safe_ref
from radiust.models import FrameRef

from tests.support.cli_experience import image_bytes


def test_rust_manifest_load_does_not_mutate_manifest_or_artifact(tmp_path):
    payload = image_bytes()
    artifact_root = tmp_path / "raw"
    artifact_root.mkdir()
    path = artifact_root / "source.png"
    path.write_bytes(payload)
    ref = FrameRef("th", "composite", datetime(2026, 9, 22, tzinfo=timezone.utc))
    manifest = tmp_path / "raw-manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "ref": safe_ref(ref),
                "artifacts": [
                    {
                        "name": path.name,
                        "media_type": "image/png",
                        "size_bytes": len(payload),
                        "sha256": hashlib.sha256(payload).hexdigest(),
                    }
                ],
                "raw_complete": True,
            }
        ),
        encoding="utf-8",
    )

    before_manifest = manifest.read_bytes()
    temp_root = tmp_path / "replay-temp"
    session = CoreEngineSession(
        load_config(
            {"runtime": {"allow_network": False, "temp_root": str(temp_root)}},
            environ={},
        )
    )
    replayed = asyncio.run(session.load_raw_manifest(manifest))
    try:
        copied = bytearray(replayed.artifact_bytes(0))
        copied[:] = b"\0" * len(copied)
        assert replayed.artifact_bytes(0) == payload
        assert json.loads(replayed.artifact_json(0))["sha256"] == hashlib.sha256(
            payload
        ).hexdigest()
    finally:
        replayed.close()
        session.cancel()

    assert path.read_bytes() == payload
    assert manifest.read_bytes() == before_manifest
    assert list(temp_root.iterdir()) == []
