"""Offline replay of a verified raw-manifest directory."""

from __future__ import annotations

import hashlib
import json
from datetime import datetime
from pathlib import Path
from typing import Any

from .errors import IntegrityError
from .models import Artifact, FrameRef, utc_datetime
from .raw import RawFrame


def load_raw(path: str | Path) -> RawFrame:
    manifest_path = Path(path).expanduser().resolve()
    if manifest_path.name != "raw-manifest.json":
        raise IntegrityError("raw replay requires a raw-manifest.json file")
    try:
        document = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError) as exc:
        raise IntegrityError(f"invalid raw manifest: {manifest_path}") from exc
    if document.get("schema_version") != 1 or document.get("raw_complete") is not True:
        raise IntegrityError("raw manifest is not a complete schema v1 manifest")
    ref_document = document.get("ref")
    if not isinstance(ref_document, dict):
        raise IntegrityError("raw manifest has no safe frame reference")
    ref = FrameRef(
        source=str(ref_document["source"]),
        product=str(ref_document["product"]),
        station=ref_document.get("station"),
        valid_time=utc_datetime(datetime.fromisoformat(str(ref_document["valid_time"]).replace("Z", "+00:00"))),
        base_time=utc_datetime(datetime.fromisoformat(str(ref_document["base_time"]).replace("Z", "+00:00"))) if ref_document.get("base_time") else None,
        locator=ref_document.get("locator", {}),
        locator_version=str(ref_document.get("locator_version", "1")),
        revision=ref_document.get("revision"),
    )
    artifacts: list[Artifact] = []
    receipts = document.get("artifacts", [])
    if not isinstance(receipts, list) or not receipts:
        raise IntegrityError("raw manifest has no artifacts")
    root = manifest_path.parent.resolve()
    for receipt in receipts:
        name = str(receipt.get("name", ""))
        if not name or Path(name).name != name:
            raise IntegrityError("raw manifest artifact name must be a simple file name")
        target = (root / name).resolve()
        if root not in target.parents or not target.is_file():
            raise IntegrityError(f"raw artifact is outside manifest directory or missing: {name}")
        payload = target.read_bytes()
        if len(payload) != int(receipt.get("size_bytes", -1)) or hashlib.sha256(payload).hexdigest() != receipt.get("sha256"):
            raise IntegrityError(f"raw artifact receipt mismatch: {name}")
        artifacts.append(
            Artifact(
                name,
                str(receipt.get("role", "data")),
                str(receipt.get("media_type", "application/octet-stream")),
                target,
                receipt.get("source_revision"),
                len(payload),
                str(receipt.get("sha256")),
            )
        )
    return RawFrame(ref, tuple(artifacts), metadata=dict(document.get("metadata", {})))


def replay_decode(raw_manifest_path: str | Path, *, source_id: str | None = None) -> Any:
    raw = load_raw(raw_manifest_path)
    try:
        source = source_id or raw.ref.source
        from .config import load_config
        from .context import SourceContext
        from .registry import get_source

        context = SourceContext(load_config(), source)
        try:
            return get_source(source).decode(raw, context)
        finally:
            context.close()
    finally:
        raw.close()
