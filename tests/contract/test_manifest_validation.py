from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import pytest
from radiust import Client, FrameRef

core = pytest.importorskip("radiust._core")


def _frame() -> FrameRef:
    return FrameRef(
        "my",
        "composite",
        datetime(2026, 9, 24, tzinfo=timezone.utc),
        station="east",
    )


def _field() -> Any:
    return core.RadarField(
        json.dumps(
            {
                "name": "reflectivity",
                "values": [1.5, 2.5, 3.5, 4.5],
                "shape": [2, 2],
                "quality": [0, 1, 2, 3],
                "units": "dBZ",
                "valid_time": "2026-09-24T00:00:00Z",
                "grid": {
                    "shape": [2, 2],
                    "crs": "EPSG:4326",
                    "x": [100.0, 101.0],
                    "y": [20.0, 21.0],
                    "affine": None,
                },
                "provenance": ["manifest-contract"],
            }
        )
    )


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("schema_version", 2),
        ("logical_id", "short"),
        ("revision", "short"),
        ("output_id", "short"),
        ("processing_hash", "short"),
        ("processing_spec", []),
        ("created_at", ""),
        ("artifacts", []),
    ],
)
def test_rust_client_does_not_skip_invalid_manifest(
    tmp_path: Path, field: str, value: Any
) -> None:
    frame = _frame()
    output_root = tmp_path / "outputs"
    with Client() as client:
        first = client.write(
            _field(), output=output_root, format="netcdf", ref=frame
        )
        output = Path(first.items[0].output_uri)
        manifest_path = output.with_name(output.name + ".manifest.json")
        malformed = json.loads(manifest_path.read_text(encoding="utf-8"))
        malformed[field] = value
        manifest_path.write_text(json.dumps(malformed), encoding="utf-8")

        repaired = client.write(
            _field(), output=output_root, format="netcdf", ref=frame
        )

    assert first.counts["written"] == 1
    assert repaired.counts["written"] == 1
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["schema_version"] == 1
    assert manifest["logical_id"] == frame.logical_id
    assert len(manifest["revision"]) == 64
    assert len(manifest["output_id"]) == 64
    assert manifest["processing_spec"]
    assert len(manifest["processing_hash"]) == 64
    assert manifest["artifacts"]
    for artifact in manifest["artifacts"]:
        payload = (output_root / artifact["relative_uri"]).read_bytes()
        assert len(payload) == artifact["size_bytes"]
        assert core.sha256(payload) == artifact["sha256"]
