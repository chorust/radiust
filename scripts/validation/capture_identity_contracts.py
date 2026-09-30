#!/usr/bin/env python3
"""Capture stable identity vectors from the current Python implementation."""

from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from radiust.identity import (
    artifact_receipt,
    cache_key,
    frame_identity,
    logical_id,
    output_id,
    processing_hash,
    processing_identity,
    resolved_revision,
    safe_ref,
)
from radiust.models import Artifact, FrameRef, ProcessingSpec

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "tests/fixtures/rust-migration/output/identity.json"


def sample() -> tuple[FrameRef, list[Artifact], ProcessingSpec]:
    frame = FrameRef(
        source="my",
        product="composite",
        station="east",
        valid_time=datetime(2025, 1, 1, 8, 0, tzinfo=timezone(timedelta(hours=8))),
        base_time=datetime(2025, 1, 1, 7, 55, tzinfo=timezone(timedelta(hours=8))),
        locator={
            "path": {"name": "radar/雪.png", "sequence": 7},
            "url": "https://example.invalid/image?token=volatile",
            "auth": {"signature": "volatile", "scope": "image"},
        },
        locator_version="2",
        revision="upstream-42",
        metadata={"ignored": True},
    )
    artifacts = [
        Artifact("image.png", "data", "image/png", b"sample-image-bytes", "upstream-42"),
        Artifact("metadata.json", "metadata", "application/json", b'{"version":1}', "upstream-42"),
    ]
    spec = ProcessingSpec(options={"palette": "rainbow", "levels": [0, 10, 20]})
    return frame, artifacts, spec


def capture() -> dict[str, Any]:
    frame, artifacts, spec = sample()
    return {
        "schema_version": 1,
        "frame_identity": frame_identity(frame),
        "logical_id": logical_id(frame),
        "safe_ref": safe_ref(frame),
        "artifact_receipts": [artifact_receipt(item) for item in artifacts],
        "resolved_revision_from_artifacts": resolved_revision(artifacts),
        "resolved_revision_upstream": resolved_revision(artifacts, "upstream-42"),
        "processing_identity": processing_identity(spec),
        "processing_hash": processing_hash(spec),
        "output_id": output_id(frame, resolved_revision(artifacts), spec),
        "cache_key": cache_key(frame, resolved_revision(artifacts)),
    }


def main() -> None:
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(
        json.dumps(capture(), ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
