"""Bounded image-byte decoding for visual preview only."""

from __future__ import annotations

import hashlib
import io
import json
from dataclasses import replace

import numpy as np
from PIL import Image, UnidentifiedImageError

from ..errors import IntegrityError, ResourceLimitError
from ..raw import RawFrame
from .engine import render_legacy_display
from .models import RawPreview
from .registry import LegacyDisplayRegistry, default_registry
from .tiles import compose_verified_tiles


def preview_bytes(
    payload: bytes,
    *,
    name: str,
    limits: dict[str, object] | None = None,
    source: str | None = None,
    product: str | None = None,
    station: str | None = None,
    valid_time: str | None = None,
) -> RawPreview:
    limits = limits or {}
    if len(payload) > int(limits.get("max_artifact_bytes", 512 * 1024 * 1024)):
        raise ResourceLimitError("raw image exceeds artifact byte limit")
    try:
        with Image.open(io.BytesIO(payload)) as image:
            image_format = image.format or "unknown"
            if image_format not in {"PNG", "GIF"}:
                raise ValueError(f"raw preview supports PNG and GIF, got {image_format}")
            pixels = image.width * image.height
            if pixels > int(limits.get("max_pixels", 100_000_000)):
                raise ResourceLimitError("raw image exceeds pixel limit")
            # Conversion and the detached output may coexist temporarily.
            if pixels * 8 > int(limits.get("max_temp_bytes", 10 * 1024**3)):
                raise ResourceLimitError("raw image exceeds temporary byte limit")
            image.seek(0)
            rgba = np.asarray(image.convert("RGBA"), dtype=np.uint8).copy()
    except (UnidentifiedImageError, OSError) as exc:
        raise ValueError("raw artifact is not a readable PNG/GIF image") from exc
    return RawPreview(
        rgba, name, image_format, hashlib.sha256(payload).hexdigest(),
        source, product, station, valid_time,
    )


def preview_raw(
    raw: RawFrame, *, limits: dict[str, object] | None = None,
    registry: LegacyDisplayRegistry | None = None,
    apply_legacy: bool = False,
) -> RawPreview:
    """Preview an acquired raw frame, preserving pixels unless asked to convert.

    RawFrame validates artifact receipts when opened. Check the selected
    snapshot again in case a path-backed artifact changed after acquisition.
    """
    raw._ensure_open()
    rule = evidence = None
    reason = "original source pixels preserved; legacy display not applied"
    if apply_legacy or len(raw.artifacts) != 1:
        selection = registry or default_registry()
        rule, evidence, reason = selection.select(raw)
    ref = raw.ref
    if len(raw.artifacts) != 1:
        if rule is None or evidence is None or "tile_plan" not in rule.input_constraints:
            raise ValueError("raw preview requires a unique verified multi-artifact display combination")
        rgba = compose_verified_tiles(raw, rule, evidence, limits=limits)
        # An aggregate identity identifies the entire verified set; no tile is
        # silently selected as the representative hash of the complete frame.
        receipts = sorted((item.name, item.sha256) for item in raw.receipts)
        aggregate = hashlib.sha256(json.dumps(receipts, separators=(",", ":")).encode("utf-8")).hexdigest()
        payload_format = raw.artifacts[0].media_type.split("/")[-1].upper()
        preview = RawPreview(
            rgba, f"{ref.source}/{ref.product} verified tiles", payload_format, aggregate,
            ref.source, ref.product, ref.station,
            ref.valid_time.isoformat().replace("+00:00", "Z"),
        )
    else:
        artifact = raw.artifacts[0]
        payload = raw.bytes(artifact.name)
        receipt = next((item for item in raw.receipts if item.name == artifact.name), None)
        if receipt is None or receipt.size_bytes != len(payload) or receipt.sha256 != hashlib.sha256(payload).hexdigest():
            raise IntegrityError("raw preview artifact receipt mismatch")
        preview = preview_bytes(
            payload, name=artifact.name, limits=limits,
            source=ref.source, product=ref.product, station=ref.station,
            valid_time=ref.valid_time.isoformat().replace("+00:00", "Z"),
        )
    if not apply_legacy:
        reason = "original source pixels preserved"
        if len(raw.artifacts) != 1:
            reason = "source colors preserved; tiles composed with a verified layout"
        return replace(preview, reason=reason)
    if rule is None:
        return replace(preview, reason=reason)
    if evidence is None:
        raise ValueError("matched legacy display rule is missing validated evidence")
    rgba = render_legacy_display(preview.rgba, rule, evidence, input_format=preview.format, limits=limits)
    return replace(preview, rgba=rgba, display_mode="legacy", rule_version=rule.rule_version,
                   reason="display-only legacy conversion; not a scientifically validated field")
