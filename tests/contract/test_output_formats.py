from __future__ import annotations

import importlib.util
import json

import numpy as np
import pytest
import xarray as xr
from radiust import RadarField
from radiust.errors import MissingDependencyError
from radiust.outputs.zarr import write_zarr


def _field() -> RadarField:
    return RadarField(json.dumps({
        "name": "reflectivity",
        "values": [1, 2, 3, 4],
        "shape": [2, 2],
        "quality": [0, 0, 0, 1],
        "units": "dBZ",
        "valid_time": "2026-09-22T00:00:00Z",
        "grid": {
            "shape": [2, 2],
            "crs": "EPSG:4326",
            "x": [100.0, 101.0],
            "y": [4.0, 3.0],
            "affine": None,
        },
        "provenance": ["source=test", "product=composite"],
    }))


def test_optional_zarr_adapter_writes_interoperable_v2(tmp_path):
    if importlib.util.find_spec("zarr") is None:
        with pytest.raises(MissingDependencyError):
            write_zarr(_field(), tmp_path / "radar.zarr")
        return

    paths = write_zarr(_field(), tmp_path / "radar.zarr")
    assert paths[0].is_dir()
    reopened = xr.open_zarr(paths[0], consolidated=True).load()
    assert reopened.reflectivity.shape == (2, 2)
    assert reopened.quality.dtype == np.dtype("uint16")


def test_zarr_adapter_gives_an_actionable_optional_dependency_error(monkeypatch, tmp_path):
    original_find_spec = importlib.util.find_spec
    monkeypatch.setattr(
        importlib.util,
        "find_spec",
        lambda name, *args, **kwargs: (
            None if name == "zarr" else original_find_spec(name, *args, **kwargs)
        ),
    )
    output = tmp_path / "missing-dependency.zarr"

    with pytest.raises(MissingDependencyError, match=r"install radiust\[zarr\]"):
        write_zarr(_field(), output)

    assert not output.exists()
