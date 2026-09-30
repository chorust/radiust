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


def test_transport_artifact_host_and_path_do_not_change_frame_identity() -> None:
    a = ref(locator={"revision": "frame.png", "artifact_host": "radar.bmkg.go.id", "artifact_path": "/sidarma/frame.png"})
    b = ref(locator={"revision": "frame.png", "artifact_host": "cdn.bmkg.go.id", "artifact_path": "/other/frame.png"})
    assert logical_id(a) == logical_id(b)


def test_tw_native_frame_keeps_the_legacy_python_logical_identity() -> None:
    valid_time = datetime(2026, 9, 20, 4, 30, tzinfo=timezone.utc)
    revision = "O-A0059-001-1789878600"
    url = "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json"
    legacy = FrameRef(
        "tw",
        "grid",
        valid_time,
        station="CV1_3600",
        locator={"url": url, "artifacts": (), "station": "CV1_3600", "revision": revision},
        locator_version="tw-legacy-v1",
        revision=revision,
    )
    native = FrameRef(
        "tw",
        "grid",
        valid_time,
        station="CV1_3600",
        locator={
            "url": url,
            "name": "O-A0059-001.json",
            "media_type": "application/json",
            "artifacts": (),
            "station": "CV1_3600",
            "revision": revision,
            "time_semantics": "provider_grid_time",
            "geometry_status": "provider_native_twd67",
            "native_crs": "EPSG:3821",
            "grid_dimension": [881, 921],
            "grid_origin": [115.0, 18.0],
            "grid_resolution": 0.0125,
        },
        locator_version="tw-cwa-v2",
        revision=revision,
    )

    assert logical_id(native) == logical_id(legacy)


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
