from __future__ import annotations

import numpy as np
import pytest
import xarray as xr
from radiust.errors import GridError
from radiust.field import RadarField
from radiust.grids import CurvilinearGrid, GeographicGrid, PolarGrid


def test_geographic_grid_preserves_center_axis_direction_and_extent():
    grid = GeographicGrid([100, 101, 102], [4, 3])
    assert grid.shape == (2, 3)
    assert grid.extent == (100.0, 4.0, 102.0, 3.0)

    with pytest.raises(GridError):
        GeographicGrid([100, 102, 101], [4, 3])


def test_polar_and_curvilinear_grids_are_explicit_types():
    polar = PolarGrid([0, 90], [1000, 2000], site_longitude=102, site_latitude=4, elevation=0.5)
    curvilinear = CurvilinearGrid(np.array([[100, 101], [100.1, 101.1]]), np.array([[4, 4], [3, 3]]))
    assert polar.kind == "polar"
    assert polar.shape == (2, 2)
    assert curvilinear.kind == "curvilinear"
    assert curvilinear.shape == (2, 2)


def test_curvilinear_coordinates_remain_two_dimensional_and_read_only():
    grid = CurvilinearGrid([[100, 101], [100.1, 101.1]], [[4, 4], [3, 3]])
    field = RadarField(
        xr.DataArray(np.zeros((2, 2), dtype="float32"), dims=("y", "x"), name="reflectivity"),
        grid,
    )

    dataset = field.to_dataset()

    assert dataset.latitude.dims == ("y", "x")
    assert dataset.longitude.dims == ("y", "x")
    with pytest.raises(ValueError):
        grid.longitude[0, 0] = 0
