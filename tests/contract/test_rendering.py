from __future__ import annotations

import numpy as np
import xarray as xr
from radiust.field import RadarField
from radiust.grids import GeographicGrid
from radiust.rendering import render_field


def test_render_is_pure_and_unknown_quality_is_transparent():
    grid = GeographicGrid([100, 101], [4, 3])
    data = xr.DataArray(np.array([[0, 10], [20, np.nan]], dtype="float32"), dims=("latitude", "longitude"), name="reflectivity", attrs={"units": "dBZ"})
    quality = xr.DataArray(np.array([[0, 4], [0, 0]], dtype="uint16"), dims=data.dims, name="quality")
    field = RadarField(data, grid, quality, {"source": "test", "product": "composite", "valid_time": "2025-01-01T00:00:00Z"})
    before = field.data.values.copy()

    image = render_field(field, vmin=0, vmax=20)

    assert image.rgba.shape == (2, 2, 4)
    assert image.rgba[0, 1, 3] == 0
    assert np.array_equal(field.data.values, before, equal_nan=True)
    assert "test / composite" in image.title
