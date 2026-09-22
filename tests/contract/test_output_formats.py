from __future__ import annotations

import importlib.util
import json

import numpy as np
import pytest
import xarray as xr
from radiust.errors import MissingDependencyError
from radiust.field import RadarField
from radiust.grids import GeographicGrid
from radiust.outputs.geotiff import write_geotiff
from radiust.outputs.png import write_png
from radiust.outputs.registry import check_encoder_dependencies
from radiust.outputs.zarr import write_zarr


def _field() -> RadarField:
    grid = GeographicGrid([100, 101], [4, 3])
    data = xr.DataArray(np.array([[1, 2], [3, np.nan]], dtype="float32"), dims=("latitude", "longitude"), name="reflectivity", attrs={"units": "dBZ"})
    quality = xr.DataArray(np.zeros((2, 2), dtype="uint16"), dims=data.dims, name="quality")
    return RadarField(data, grid, quality, {"source": "test", "product": "composite"})


def test_png_writes_render_sidecar(tmp_path):
    paths = write_png(_field(), tmp_path / "radar.png")
    sidecar = json.loads(paths[1].read_text(encoding="utf-8"))
    assert paths[0].exists()
    assert sidecar["shape"] == [2, 2]
    assert sidecar["grid"] == "geographic"


def test_wgs84_grid_mapping_encodes_ellipsoid_not_sphere():
    attrs = _field().to_dataset().crs.attrs
    assert attrs["grid_mapping_name"] == "latitude_longitude"
    assert attrs["semi_major_axis"] == pytest.approx(6378137.0)
    assert attrs["inverse_flattening"] == pytest.approx(298.257223563)
    assert "earth_radius" not in attrs


def test_optional_format_dependencies_fail_before_writer_when_missing(tmp_path):
    if importlib.util.find_spec("rasterio") is None:
        with pytest.raises(MissingDependencyError):
            check_encoder_dependencies("geotiff")
    else:
        paths = write_geotiff(_field(), tmp_path / "radar.tif")
        assert {path.name for path in paths} == {"radar.tif", "radar_quality.tif", "radar_provenance.json"}
        import rasterio

        with rasterio.open(paths[0]) as dataset:
            assert dataset.count == 1
            assert dataset.dtypes == ("float32",)
            assert dataset.crs.to_string() == "EPSG:4326"
            assert dataset.read(1).shape == (2, 2)
        with rasterio.open(paths[1]) as dataset:
            assert dataset.dtypes == ("uint16",)
            assert dataset.read(1).max() == 0

    if importlib.util.find_spec("zarr") is None:
        with pytest.raises(MissingDependencyError):
            check_encoder_dependencies("zarr")
    else:
        paths = write_zarr(_field(), tmp_path / "radar.zarr")
        assert paths[0].is_dir()
        reopened = xr.open_zarr(paths[0], consolidated=True).load()
        assert reopened.reflectivity.shape == (2, 2)
        assert reopened.quality.dtype == np.dtype("uint16")
