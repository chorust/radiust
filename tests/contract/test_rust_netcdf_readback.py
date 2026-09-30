"""Independent xarray/netCDF4 readback of a Rust-written NetCDF4 fixture."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import pytest

FIXTURE = Path(__file__).parents[1] / "fixtures/rust-migration/output/netcdf/field.nc"


def test_python_independently_reads_rust_netcdf_values_and_cf_metadata() -> None:
    xr = pytest.importorskip("xarray")
    pytest.importorskip("netCDF4")

    with xr.open_dataset(FIXTURE, engine="netcdf4") as dataset:
        field = dataset["reflectivity"]
        assert field.dtype == np.dtype("float32")
        np.testing.assert_allclose(field.values[[0, 1], [0, 1]], [12.5, 0.25])
        assert np.isnan(field.values[0, 1])
        assert field.attrs["units"] == "dBZ"
        assert field.attrs["grid_mapping"] == "crs"
        assert field.encoding["coordinates"] == "time latitude longitude"
        assert dataset["quality"].dtype == np.dtype("uint16")
        np.testing.assert_array_equal(dataset["quality"].values, [[0, 1], [4, 0]])
        np.testing.assert_array_equal(dataset["longitude"].values, [120.0, 120.5])
        np.testing.assert_array_equal(dataset["latitude"].values, [23.0, 23.5])
        assert str(dataset["time"].values) == "2026-09-26T01:02:03.000000000"
        assert dataset["crs"].attrs["spatial_ref"] == "EPSG:4326"
        assert dataset.attrs["Conventions"] == "CF-1.8"
        assert json.loads(dataset.attrs["radiust_provenance"]) == ["source=fixture"]
        assert json.loads(dataset.attrs["radiust_affine"]) == [119.75, 0.5, 0.0, 23.75, 0.0, -0.5]
