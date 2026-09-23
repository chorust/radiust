"""Evidence-gated raw tile composition for *display only*.

The rule's explicit plan identifies every tile and its exact position. No
source-agnostic inference, nearest-neighbour filling or scientific regridding
is allowed. A rule's passed status alone does not establish real provider
provenance; the registry must independently verify its comparison evidence.
"""

from __future__ import annotations

import hashlib
import io
from collections.abc import Mapping
from pathlib import Path
from typing import Any

import numpy as np
from PIL import Image, UnidentifiedImageError

from ..raw import RawFrame
from .engine import DisplayBlockedError
from .rules import DisplayEvidence, LegacyDisplayRule


def _positive(value: object, label: str) -> int:
    if type(value) is not int or value < 1:
        raise DisplayBlockedError(f"tile {label} must be a positive integer")
    return value


def compose_verified_tiles(
    raw: RawFrame,
    rule: LegacyDisplayRule,
    evidence: DisplayEvidence | None,
    *,
    limits: Mapping[str, object] | None = None,
) -> np.ndarray:
    """Assemble exactly one selected, intact tile frame into detached RGBA."""
    if rule.validation_status != "passed" or evidence is None or not evidence.validates(rule):
        raise DisplayBlockedError("verified legacy tile display evidence is absent")
    raw._ensure_open()
    if raw.ref.source != rule.source or raw.ref.product != rule.product:
        raise DisplayBlockedError("tile source/product does not match display rule")
    plan = rule.input_constraints.get("tile_plan")
    if not isinstance(plan, Mapping):
        raise DisplayBlockedError("tile combination rule is missing")
    if plan.get("station") != raw.ref.station or not raw.ref.revision:
        raise DisplayBlockedError("tile frame station or revision is not bound")
    layout = plan.get("layout")
    if not isinstance(layout, str) or not layout or raw.metadata.get("tile_layout") != layout:
        raise DisplayBlockedError("tile layout is not source-verified")
    tile_width = _positive(plan.get("tile_width"), "width")
    tile_height = _positive(plan.get("tile_height"), "height")
    columns = _positive(plan.get("columns"), "columns")
    rows = _positive(plan.get("rows"), "rows")
    count = columns * rows
    if count > 256 or raw.metadata.get("tile_count") != count or raw.metadata.get("tile_size") != tile_width:
        raise DisplayBlockedError("tile count or dimensions lack a verified source plan")
    declared = plan.get("tiles")
    if not isinstance(declared, list) or len(declared) != count or len(raw.artifacts) != count:
        raise DisplayBlockedError("incomplete tile set: no missing or extra tiles permitted")
    limits = limits or {}
    max_pixels = min(
        _positive(rule.input_constraints.get("max_pixels"), "rule max_pixels"),
        _positive(limits.get("max_pixels", 100_000_000), "runtime max_pixels"),
    )
    max_artifact = _positive(limits.get("max_artifact_bytes", 512 * 1024 * 1024), "artifact byte limit")
    max_frame = _positive(limits.get("max_frame_bytes", 2 * 1024**3), "frame byte limit")
    max_temp = _positive(limits.get("max_temp_bytes", 10 * 1024**3), "temporary byte limit")
    width, height = tile_width * columns, tile_height * rows
    if width * height > max_pixels or width * height * 12 > max_temp:
        raise DisplayBlockedError("tile mosaic pixel or temporary memory limit exceeded")
    if raw.metadata.get("tile_size") != tile_height:
        raise DisplayBlockedError("tile height does not match source metadata")

    receipt_by_name = {item.name: item for item in raw.receipts}
    if len(receipt_by_name) != count:
        raise DisplayBlockedError("tile receipt count or identities differ")
    tiles: list[tuple[int, int, Any]] = []
    seen: set[tuple[int, int]] = set()
    total_bytes = 0
    for tile, artifact in zip(declared, raw.artifacts, strict=True):
        if not isinstance(tile, Mapping):
            raise DisplayBlockedError("tile plan entry must be explicit")
        x, y = tile.get("x"), tile.get("y")
        if type(x) is not int or type(y) is not int or not (0 <= x < columns and 0 <= y < rows):
            raise DisplayBlockedError("tile coordinate is not in the declared layout")
        if (x, y) in seen:
            raise DisplayBlockedError("duplicate tile coordinate in display plan")
        seen.add((x, y))
        if (tile.get("name") != artifact.name or tile.get("role") != artifact.role or
                tile.get("media_type") != artifact.media_type):
            raise DisplayBlockedError("tile identity, role, media type or order differs from display plan")
        if artifact.source_revision != raw.ref.revision:
            raise DisplayBlockedError("tile belongs to a different frame revision")
        receipt = receipt_by_name.get(artifact.name)
        if receipt is None or receipt.source_revision != raw.ref.revision:
            raise DisplayBlockedError("tile receipt does not belong to selected frame")
        if receipt.size_bytes > max_artifact or receipt.size_bytes > max_frame - total_bytes:
            raise DisplayBlockedError("tile byte limits exceeded before reading artifact")
        # Path-backed raw can change after the RawFrame was acquired. Never
        # read an unbounded path before rechecking its receipt and size cap.
        if isinstance(artifact.payload, bytes):
            payload = artifact.payload
        else:
            payload_path = Path(artifact.payload)
            if payload_path.stat().st_size > max_artifact:
                raise DisplayBlockedError("tile artifact changed beyond byte limit")
            with payload_path.open("rb") as stream:
                payload = stream.read(max_artifact + 1)
        total_bytes += len(payload)
        if (len(payload) > max_artifact or total_bytes > max_frame or
                receipt.size_bytes != len(payload) or receipt.sha256 != hashlib.sha256(payload).hexdigest()):
            raise DisplayBlockedError("tile byte limits or integrity receipt mismatch")
        required_format = tile.get("format")
        if required_format not in {"PNG", "WEBP"}:
            raise DisplayBlockedError("unsupported or missing tile format in display plan")
        try:
            with Image.open(io.BytesIO(payload)) as image:
                if image.format != required_format or image.size != (tile_width, tile_height):
                    raise DisplayBlockedError("tile format or dimensions differ from display plan")
                pixels = np.asarray(image.convert("RGBA"), dtype=np.uint8).copy()
        except (OSError, ValueError, UnidentifiedImageError) as exc:
            if isinstance(exc, DisplayBlockedError):
                raise
            raise DisplayBlockedError("tile is not a valid source-verified image") from exc
        tiles.append((x, y, pixels))
    if len(seen) != count:
        raise DisplayBlockedError("tile plan has missing coordinates")
    mosaic = np.zeros((height, width, 4), dtype=np.uint8)
    for x, y, pixels in tiles:
        mosaic[y * tile_height:(y + 1) * tile_height,
               x * tile_width:(x + 1) * tile_width] = pixels
    return mosaic


__all__ = ["compose_verified_tiles"]
