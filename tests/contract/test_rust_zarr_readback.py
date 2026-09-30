"""Independent xarray readback of a Rust-written consolidated Zarr v2 fixture."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest

FIXTURE = Path(__file__).parents[1] / "fixtures/rust-migration/output/zarr/field.zarr"


def test_python_xarray_reads_rust_zarr_values_and_metadata() -> None:
    xr = pytest.importorskip("xarray")
    pytest.importorskip("zarr")

    with xr.open_zarr(FIXTURE, consolidated=True) as dataset:
        dataset.load()
        field = dataset["reflectivity"]
        assert field.dtype == np.dtype("float32")
        np.testing.assert_allclose(field.values[[0, 1], [0, 1]], [12.5, 0.25])
        assert np.isnan(field.values[0, 1])
        assert field.attrs["units"] == "dBZ"
        assert field.attrs["grid_mapping"] == "crs"
        assert dataset["quality"].dtype == np.dtype("uint16")
        np.testing.assert_array_equal(dataset["quality"].values, [[0, 1], [4, 0]])
        np.testing.assert_array_equal(dataset["longitude"].values, [120.0, 120.5])
        np.testing.assert_array_equal(dataset["latitude"].values, [23.0, 23.5])
        assert str(dataset["time"].values) == "2026-09-26T01:02:03.123456789"
        assert dataset["crs"].attrs["spatial_ref"] == "EPSG:4326"
        assert dataset["crs"].attrs["inverse_flattening"] == pytest.approx(298.257223563)
        assert dataset.attrs["Conventions"] == "CF-1.8"
        assert dataset.attrs["radiust_grid_kind"] == "geographic"
        assert dataset.attrs["radiust_provenance"] == ["source=fixture"]

    metadata = json.loads((FIXTURE / ".zmetadata").read_text(encoding="utf-8"))
    assert metadata["zarr_consolidated_format"] == 1
    arrays = metadata["metadata"]
    assert arrays["reflectivity/.zarray"]["dtype"] == "<f4"
    assert arrays["reflectivity/.zarray"]["chunks"] == [2, 2]
    assert arrays["reflectivity/.zarray"]["compressor"] == {
        "blocksize": 0,
        "clevel": 5,
        "cname": "lz4",
        "id": "blosc",
        "shuffle": 1,
    }
    assert arrays["quality/.zarray"]["dtype"] == "<u2"
    assert arrays["quality/.zarray"]["compressor"] == arrays["reflectivity/.zarray"]["compressor"]
