import numpy as np
import pytest
import xarray as xr
from radiust.field import RadarDataset, RadarField
from radiust.grids import GeographicGrid


def make_field() -> RadarField:
    grid = GeographicGrid([100, 101, 102], [4, 3])
    data = xr.DataArray(np.array([[0, 1, np.nan], [2, 3, 4]], dtype="float32"), dims=("latitude", "longitude"), name="reflectivity", attrs={"units": "dBZ"})
    quality = xr.DataArray(np.zeros((2, 3), dtype="uint16"), dims=data.dims, name="quality")
    return RadarField(data, grid, quality)


def test_quality_is_uint16_and_zero_is_not_missing() -> None:
    field = make_field()
    assert field.quality.dtype == np.dtype("uint16")
    assert field.data.values[0, 0] == 0
    assert np.isnan(field.data.values[0, 2])
    assert set(field.to_dataset().data_vars) == {"reflectivity", "quality"}


def test_dataset_requires_matching_grid() -> None:
    grid = GeographicGrid([100, 101, 102], [4, 3])
    ds = xr.Dataset({"a": (("latitude", "longitude"), np.zeros((2, 2), dtype="float32"))})
    with pytest.raises(ValueError):
        RadarDataset(ds, grid)
