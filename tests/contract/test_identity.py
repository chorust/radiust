from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest
from radiust import FrameRef, ProcessingSpec
from radiust.identity import (
    canonical_json,
    logical_id,
    output_id,
    processing_hash,
    resolved_revision,
    safe_ref,
)
from radiust.models import Artifact


def ref(**kwargs):
    values = {"source": "my", "product": "composite", "station": "east", "valid_time": datetime(2025, 1, 1, tzinfo=timezone.utc), "locator": {"path": "a"}}
    values.update(kwargs)
    return FrameRef(**values)


def test_canonical_identity_ignores_url_and_normalizes_time() -> None:
    a = ref(uri="https://example.invalid/a?sig=secret", valid_time=datetime(2025, 1, 1, tzinfo=timezone.utc))
    b = ref(uri="https://example.invalid/b?sig=other", valid_time=datetime(2025, 1, 1, 8, tzinfo=timezone(timedelta(hours=8))))
    assert logical_id(a) == logical_id(b)
    assert "uri" not in safe_ref(a)


def test_revision_is_order_independent() -> None:
    a = Artifact("a.bin", "data", "application/octet-stream", b"a")
    b = Artifact("b.bin", "data", "application/octet-stream", b"b")
    assert resolved_revision((a, b)) == resolved_revision((b, a))


def test_processing_and_output_kind_are_distinct() -> None:
    decoded = ProcessingSpec(format="netcdf")
    raw = ProcessingSpec(output_kind="raw-only")
    assert processing_hash(decoded) != processing_hash(raw)
    assert output_id(ref(), "revision", decoded) != output_id(ref(), "revision", raw)


def test_canonical_json_rejects_non_finite() -> None:
    with pytest.raises(ValueError):
        canonical_json({"value": float("nan")})


def test_frame_reference_freezes_nested_locator_and_metadata():
    value = ref(locator={"path": {"name": "a"}}, metadata={"source": "fixture"})
    with pytest.raises(TypeError):
        value.locator["path"] = {"name": "b"}
    assert logical_id(value) == logical_id(ref(locator={"path": {"name": "a"}}))
