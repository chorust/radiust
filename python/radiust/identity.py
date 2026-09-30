"""Thin Python facade for canonical identity operations implemented in Rust."""

from __future__ import annotations

import json
from collections.abc import Sequence
from pathlib import Path
from typing import Any

from . import _bridge
from .models import Artifact, FrameRef, ProcessingSpec

IDENTITY_SCHEMA = 1


def _native() -> Any:
    if _bridge._core is None:
        raise ImportError("canonical identities require the compiled radiust Rust extension")
    return _bridge._core


def _json(value: Any) -> str:
    return json.dumps(
        _bridge._json_safe(value), ensure_ascii=False, allow_nan=False, separators=(",", ":")
    )


def _frame_json(ref: FrameRef) -> str:
    if isinstance(ref, FrameRef):
        value = {
            "source": ref.source,
            "product": ref.product,
            "station": ref.station,
            "valid_time": ref.valid_time,
            "base_time": ref.base_time,
            "logical_id": "",
            "revision": ref.revision,
            "locator_version": ref.locator_version,
            "locator": ref.locator,
        }
        return _json(value)
    to_json = getattr(ref, "to_json", None)
    if callable(to_json):
        return to_json()
    raise TypeError("frame identity requires a FrameRef")


def _spec_json(spec: ProcessingSpec) -> str:
    if not isinstance(spec, ProcessingSpec):
        raise TypeError("processing identity requires a ProcessingSpec")
    return _json({
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
        "options": spec.options,
    })


def canonical_json(value: Any) -> str:
    """Serialize a JSON value with Rust's stable key ordering and formatting."""
    return _native().identity_canonical_json(_json(value))


def digest(value: Any) -> str:
    return _native().identity_digest(_json(value))


def frame_identity(ref: FrameRef) -> dict[str, Any]:
    return json.loads(_native().identity_frame_json(_frame_json(ref)))


def logical_id(ref: FrameRef) -> str:
    if not isinstance(ref, FrameRef) and isinstance(ref, getattr(_native(), "FrameRef", ())):
        return ref.logical_id
    return _native().identity_logical_id(_frame_json(ref))


def artifact_bytes(artifact: Artifact) -> bytes:
    payload = artifact.payload
    return payload if isinstance(payload, bytes) else Path(payload).read_bytes()


def artifact_receipt(artifact: Artifact) -> dict[str, Any]:
    receipt = _native().identity_artifact_receipt_json(
        artifact.name, artifact_bytes(artifact), artifact.source_revision
    )
    return json.loads(receipt)


def resolved_revision(artifacts: Sequence[Artifact], upstream_revision: str | None = None) -> str:
    material = [
        (artifact.name, artifact_bytes(artifact), artifact.source_revision)
        for artifact in artifacts
    ]
    return _native().identity_resolved_revision(material, upstream_revision)


def processing_identity(spec: ProcessingSpec) -> dict[str, Any]:
    return json.loads(_native().identity_processing_json(_spec_json(spec)))


def processing_hash(spec: ProcessingSpec) -> str:
    return _native().identity_processing_hash(_spec_json(spec))


def output_id(ref: FrameRef, revision: str, spec: ProcessingSpec) -> str:
    return _native().identity_output_id(_frame_json(ref), revision, _spec_json(spec))


def variant_id(output: str) -> str:
    return _native().identity_variant_id(output)


def cache_key(
    ref: FrameRef,
    revision: str | None,
    *,
    acquisition_version: str = "1",
    mosaic_version: str = "1",
) -> str:
    return _native().identity_cache_key(
        _frame_json(ref), revision, acquisition_version, mosaic_version
    )


def safe_ref(ref: FrameRef) -> dict[str, Any]:
    return json.loads(_native().identity_safe_ref(_frame_json(ref)))
