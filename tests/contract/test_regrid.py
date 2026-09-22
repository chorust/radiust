from __future__ import annotations

import numpy as np
import pytest
import xarray as xr
from radiust.errors import GridError
from radiust.field import RadarDataset, RadarField
from radiust.grids import GeographicGrid


def _field(values: np.ndarray, *, variable: str = "reflectivity", units: str = "dBZ", quality: np.ndarray | None = None) -> RadarField:
    grid = GeographicGrid([0, 1], [1, 0])
    data = xr.DataArray(values.astype("float32"), dims=("latitude", "longitude"), name=variable, attrs={"units": units})
    flags = xr.DataArray(np.zeros(values.shape, dtype="uint16") if quality is None else quality, dims=data.dims, name="quality")
    return RadarField(data, grid, flags)


def test_bilinear_reflectivity_uses_linear_physical_space():
    field = _field(np.array([[0, 10], [10, 20]], dtype="float32"))
    target = GeographicGrid([0, 0.5, 1], [1, 0.5, 0])

    result = field.regrid(target, method="bilinear")

    expected = 10 * np.log10((10 ** (0 / 10) + 10 ** (10 / 10) + 10 ** (10 / 10) + 10 ** (20 / 10)) / 4)
    assert np.isclose(result.data.values[1, 1], expected, atol=1e-5)
    assert result.data.values is not field.data.values


def test_bilinear_rejects_categorical_and_does_not_fill_missing_holes():
    categorical = _field(np.array([[1, 2], [3, 4]]), variable="category", units="1")
    with pytest.raises(GridError):
        categorical.regrid(GeographicGrid([0.5], [0.5]), method="bilinear")

    values = np.array([[1, np.nan], [3, 4]], dtype="float32")
    result = _field(values).regrid(GeographicGrid([0.5], [0.5]), method="bilinear")
    assert np.isnan(result.data.values[0, 0])


def test_bilinear_marks_interpolated_quality_and_preserves_missing_reason():
    field = _field(np.ones((2, 2), dtype="float32"))
    result = field.regrid(GeographicGrid([0.5], [0.5]), method="bilinear")
    assert result.quality.values[0, 0] & 16

    values = np.array([[1, 2], [np.nan, 4]], dtype="float32")
    quality = np.array([[0, 0], [4, 0]], dtype="uint16")
    result = _field(values, quality=quality).regrid(GeographicGrid([0.25], [0.75]), method="bilinear")
    assert np.isnan(result.data.values[0, 0])
    assert result.quality.values[0, 0] & 4


def test_bilinear_ignores_missing_samples_with_zero_weight():
    grid = GeographicGrid([0, 1, 2], [2, 1, 0])
    values = np.array([[1, 2, np.nan], [3, 7, np.nan], [5, 17, np.nan]], dtype="float32")
    quality = np.array([[0, 0, 1], [0, 0, 1], [0, 0, 1]], dtype="uint16")
    data = xr.DataArray(values, dims=("latitude", "longitude"), name="reflectivity", attrs={"units": "dBZ"})
    field = RadarField(data, grid, xr.DataArray(quality, dims=data.dims))
    result = field.regrid(GeographicGrid([1], [1, 0.5]), method="bilinear")
    np.testing.assert_allclose(result.data.values[:, 0], [7, 10 * np.log10((10**0.7 + 10**1.7) / 2)])
    np.testing.assert_array_equal(result.quality.values[:, 0], [0, 16])


def test_bilinear_exact_source_grid_retains_valid_and_missing_pixels():
    grid = GeographicGrid([0, 1, 2], [2, 1, 0])
    values = np.array([[1, 2, np.nan], [3, 7, np.nan], [5, 17, np.nan]], dtype="float32")
    data = xr.DataArray(values, dims=("latitude", "longitude"), name="reflectivity", attrs={"units": "dBZ"})
    flags = xr.DataArray(np.where(np.isnan(values), 1, 0).astype("uint16"), dims=data.dims)
    field = RadarField(data, grid, flags)
    result = field.regrid(field.grid, method="bilinear")
    np.testing.assert_allclose(result.data.values, field.data.values, equal_nan=True)
    assert result.quality.values[0, 0] == 0
    assert result.quality.values[0, 2] & 1
    assert result.quality.values[1, 1] == 0


def test_multivariable_bilinear_regrid_preserves_interpolated_quality():
    grid = GeographicGrid([0, 1], [1, 0])
    dataset = xr.Dataset({
        "reflectivity": (("latitude", "longitude"), np.full((2, 2), 10, dtype="float32")),
        "rain_rate": (("latitude", "longitude"), np.full((2, 2), 2, dtype="float32")),
    })
    quality = xr.DataArray(np.zeros((2, 2), dtype="uint16"), dims=("latitude", "longitude"))
    field = RadarDataset(dataset, grid, quality)

    result = field.regrid(GeographicGrid([0.5], [0.5]), method="bilinear")

    assert set(result.data.data_vars) == {"reflectivity", "rain_rate"}
    assert int(result.quality.values[0, 0]) == 16
