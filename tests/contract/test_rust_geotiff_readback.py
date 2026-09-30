"""Independent Rasterio readback of a Rust-written GeoTIFF artifact group."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from pathlib import Path

import pytest

FIXTURE = Path(__file__).parents[1] / "fixtures/rust-migration/output/geotiff/field.tif"


def test_rasterio_reads_rust_geotiff_values_quality_and_web_mercator_grid() -> None:
    if importlib.util.find_spec("rasterio") is None:
        pytest.skip("Rasterio is not installed")

    reader = r'''import json, math, sys
import numpy as np
import rasterio
from pathlib import Path

path = Path(sys.argv[1])
radius = 6_378_137.0
dx = radius * math.radians(0.5)
west = radius * math.radians(120.0) - dx / 2
with rasterio.open(path) as data:
    assert data.crs.to_epsg() == 3857
    assert data.count == 1 and data.dtypes == ("float32",)
    assert data.compression.name.lower() == "deflate"
    assert math.isnan(data.nodata)
    np.testing.assert_allclose(tuple(data.transform)[:6], (dx, 0.0, west, 0.0, -1_000_000.0, 2_500_000.0), rtol=0, atol=1e-5)
    np.testing.assert_allclose(data.read(1), [[1.5, 2.5], [-3.0, 0.25], [12.5, np.nan]], rtol=0, atol=0, equal_nan=True)
    data_transform = data.transform

with rasterio.open(path.with_name("field_quality.tif")) as quality:
    assert quality.crs.to_epsg() == 3857
    assert quality.transform == data_transform
    assert quality.dtypes == ("uint16",)
    np.testing.assert_array_equal(quality.read(1), [[2, 3], [4, 0], [0, 1]])

twd67_path = path.with_name("field_twd67.tif")
with rasterio.open(twd67_path) as twd67:
    assert twd67.crs.to_epsg() == 3821
    np.testing.assert_allclose(tuple(twd67.transform)[:6], (0.5, 0.0, 119.75, 0.0, -0.5, 24.25), rtol=0, atol=1e-12)
    np.testing.assert_allclose(twd67.read(1), [[1.5, 2.5], [-3.0, 0.25], [12.5, np.nan]], rtol=0, atol=0, equal_nan=True)

provenance = json.loads(path.with_name("field_provenance.json").read_text())
assert provenance["source_crs"] == "EPSG:4326"
assert provenance["crs"] == "EPSG:3857"
assert provenance["coordinate_operation"] == "spherical_web_mercator"
assert provenance["grid"] == "projected"
assert provenance["provenance"] == ["source=fixture"]
'''
    environment = os.environ.copy()
    for name in ("PROJ_DATA", "PROJ_LIB", "GDAL_DATA", "GDAL_DRIVER_PATH"):
        environment.pop(name, None)
    result = subprocess.run(
        [sys.executable, "-c", reader, str(FIXTURE)],
        check=False,
        capture_output=True,
        text=True,
        env=environment,
    )
    assert result.returncode == 0, f"Rasterio readback failed:\n{result.stdout}\n{result.stderr}"
