from __future__ import annotations

import hashlib
import json
from datetime import datetime
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[1] / "fixtures" / "sources"


@pytest.mark.parametrize("manifest", sorted(ROOT.glob("*/fixture.json")), ids=lambda p: p.parent.name)
def test_source_fixture_preserves_real_raw_hashes_and_identity(manifest: Path):
    document = json.loads(manifest.read_text(encoding="utf-8"))
    assert document["schema_version"] == 1
    assert document.get("source") == manifest.parent.name
    assert document.get("origin")
    assert document.get("collected_at")
    assert document.get("license_basis")
    assert document.get("source_commit") or document.get("source")
    frames = document.get("frames")
    if document.get("status") == "blocked":
        assert frames == []
        assert document.get("blockers")
        assert document.get("evidence")
        return
    assert isinstance(frames, list) and frames
    for frame in frames:
        assert frame.get("product")
        assert "station" in frame
        assert frame.get("valid_time")
        assert frame.get("locator_version")
        assert frame.get("revision")
        valid_time = datetime.fromisoformat(frame["valid_time"].replace("Z", "+00:00"))
        assert valid_time.tzinfo is not None and valid_time.utcoffset() is not None
        metadata = frame.get("metadata")
        assert isinstance(metadata, dict)
        science_status = str(metadata.get("scientific_reference_status", ""))
        if science_status.startswith("blocked:"):
            assert any(key in metadata for key in ("reference_shape", "raw_mode", "raw_format", "raw_dtype"))
        else:
            assert science_status.startswith("verified:")
            assert metadata.get("reference_shape")
            assert metadata.get("reference_dtype")
            assert metadata.get("reference_units")
            assert isinstance(metadata.get("reference_grid"), dict)
            assert isinstance(metadata.get("reference_quality"), dict)
            assert metadata.get("reference_pixels")
        artifacts = frame.get("artifacts")
        assert isinstance(artifacts, list) and artifacts
        assert len({artifact.get("name") for artifact in artifacts}) == len(artifacts)
        for artifact in artifacts:
            path = manifest.parent / artifact["path"]
            assert not Path(artifact["path"]).is_absolute()
            assert ".." not in Path(artifact["path"]).parts
            assert path.is_file(), path
            payload = path.read_bytes()
            digest = hashlib.sha256(payload).hexdigest()
            assert digest == artifact["sha256"]
            if "size_bytes" in artifact:
                assert artifact["size_bytes"] == len(payload)
