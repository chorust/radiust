"""Fixture validation used by offline replay tests."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any


def load_fixture(path: str | Path) -> dict[str, Any]:
    fixture_path = Path(path)
    value = json.loads(fixture_path.read_text(encoding="utf-8"))
    required = {"schema_version", "source", "origin", "collected_at", "license_basis", "frames"}
    missing = required - set(value)
    if missing:
        raise ValueError(f"fixture is missing {sorted(missing)}")
    if value["schema_version"] != 1 or not value["frames"]:
        raise ValueError("fixture schema_version must be 1 and contain source and frames")
    for frame in value["frames"]:
        for artifact in frame.get("artifacts", []):
            artifact_path = fixture_path.parent / artifact["path"]
            data = artifact_path.read_bytes()
            if "sha256" in artifact and hashlib.sha256(data).hexdigest() != artifact["sha256"]:
                raise ValueError(f"fixture hash mismatch: {artifact_path}")
            artifact["size_bytes"] = len(data)
    return value
