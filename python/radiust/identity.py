"""The single canonical identity implementation for Python and Rust."""

from __future__ import annotations

import dataclasses
import hashlib
import json
import math
from collections.abc import Mapping, Sequence
from datetime import datetime
from pathlib import Path
from typing import Any

from .models import Artifact, FrameRef, ProcessingSpec, format_time

IDENTITY_SCHEMA = 1
VOLATILE_LOCATOR_KEYS = {"url", "uri", "signed_url", "signature", "sig", "token", "expires", "headers", "cookies"}


def _json_value(value: Any) -> Any:
    if isinstance(value, datetime):
        return format_time(value)
    if isinstance(value, Path):
        return str(value)
    if dataclasses.is_dataclass(value):
        return {field.name: _json_value(getattr(value, field.name)) for field in dataclasses.fields(value)}
    if isinstance(value, Mapping):
        return {str(key): _json_value(item) for key, item in value.items()}
    if isinstance(value, (tuple, list)):
        return [_json_value(item) for item in value]
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("NaN and Infinity are not valid identity values")
    if isinstance(value, (str, int, float, bool)) or value is None:
        return value
    raise TypeError(f"unsupported identity value: {type(value).__name__}")


def canonical_json(value: Any) -> str:
    return json.dumps(
        _json_value(value),
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
        allow_nan=False,
    )


def digest(value: Any) -> str:
    return hashlib.sha256(canonical_json(value).encode("utf-8")).hexdigest()


def freeze(value: Any) -> Any:
    """Recursively turn mappings and sequences into immutable values."""
    if isinstance(value, Mapping):
        return tuple(sorted((str(key), freeze(item)) for key, item in value.items()))
    if isinstance(value, (tuple, list)):
        return tuple(freeze(item) for item in value)
    if isinstance(value, float) and not math.isfinite(value):
        raise ValueError("NaN and Infinity are not valid identity values")
    return value


def frame_identity(ref: FrameRef) -> dict[str, Any]:
    return {
        "identity_schema": IDENTITY_SCHEMA,
        "source": ref.source,
        "product": ref.product,
        "station": ref.station,
        "valid_time": format_time(ref.valid_time),
        "base_time": format_time(ref.base_time) if ref.base_time else None,
        "locator_version": ref.locator_version,
        "locator": _safe_locator(ref.locator),
    }


def logical_id(ref: FrameRef) -> str:
    return digest(frame_identity(ref))


def artifact_bytes(artifact: Artifact) -> bytes:
    payload = artifact.payload
    if isinstance(payload, bytes):
        return payload
    return Path(payload).read_bytes()


def artifact_receipt(artifact: Artifact) -> dict[str, Any]:
    payload = artifact_bytes(artifact)
    return {
        "name": artifact.name,
        "size_bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
        "source_revision": artifact.source_revision,
    }


def resolved_revision(artifacts: Sequence[Artifact], upstream_revision: str | None = None) -> str:
    if upstream_revision:
        return digest({"namespace": "upstream", "revision": upstream_revision})
    receipts = [artifact_receipt(artifact) for artifact in artifacts]
    receipts.sort(key=lambda item: item["name"])
    return digest({"identity_schema": IDENTITY_SCHEMA, "artifacts": receipts})


def processing_identity(spec: ProcessingSpec) -> dict[str, Any]:
    return {
        "identity_schema": IDENTITY_SCHEMA,
        "output_kind": spec.output_kind,
        "format": spec.format,
        "variable": spec.variable,
        "grid": spec.grid,
        "bbox": spec.bbox,
        "resolution": spec.resolution,
        "resampling": spec.resampling,
        "decoder_version": spec.decoder_version,
        "resource_version": spec.resource_version,
        "encoder_version": spec.encoder_version,
        "options": _json_value(spec.options),
    }


def processing_hash(spec: ProcessingSpec) -> str:
    return digest(processing_identity(spec))


def output_id(ref: FrameRef, revision: str, spec: ProcessingSpec) -> str:
    return digest(
        {
            "identity_schema": IDENTITY_SCHEMA,
            "logical_id": logical_id(ref),
            "resolved_revision": revision,
            "processing_hash": processing_hash(spec),
        }
    )


def variant_id(output: str) -> str:
    return output[:12]


def cache_key(ref: FrameRef, revision: str | None, *, acquisition_version: str = "1", mosaic_version: str = "1") -> str:
    return digest(
        {
            "logical_id": logical_id(ref),
            "revision": revision,
            "acquisition_version": acquisition_version,
            "mosaic_version": mosaic_version,
        }
    )


def safe_ref(ref: FrameRef) -> dict[str, Any]:
    """Persistent/report projection that deliberately excludes URI secrets."""
    return {
        "source": ref.source,
        "product": ref.product,
        "station": ref.station,
        "valid_time": format_time(ref.valid_time),
        "base_time": format_time(ref.base_time) if ref.base_time else None,
        "locator": _safe_locator(ref.locator),
        "locator_version": ref.locator_version,
        "revision": ref.revision,
        "logical_id": logical_id(ref),
    }


def _safe_locator(value: Any) -> Any:
    if isinstance(value, Mapping):
        return {
            str(key): _safe_locator(item)
            for key, item in value.items()
            if str(key).lower() not in VOLATILE_LOCATOR_KEYS
        }
    if isinstance(value, (tuple, list)):
        return [_safe_locator(item) for item in value]
    return value
