from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest
from radiust import Client

from tests.support.native_science_fixture import (
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


def test_native_tw_fixture_round_trips_netcdf(tmp_path: Path) -> None:
    xr = pytest.importorskip("xarray")
    pytest.importorskip("netCDF4")
    ref = tw_grid_ref()
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, ref)
    output_root = tmp_path / "output"
    config = tw_offline_config(cache_root, output_root, tmp_path / "temp")

    with Client(config=config) as client:
        field = client.fetch(ref)
        report = client.download(ref, output=output_root, raw=True)

    assert report.counts["written"] == 1
    nc = next(output_root.rglob("*.nc"))
    with xr.open_dataset(nc, engine="netcdf4") as dataset:
        assert dataset.reflectivity.shape == tuple(field.shape)
        assert dataset.reflectivity.dtype == np.dtype("float32")
        assert dataset.quality.dtype == np.dtype("uint16")
        assert dataset.attrs["Conventions"] == "CF-1.8"
        assert dataset.reflectivity.attrs["ancillary_variables"] == "quality"
        assert dataset.reflectivity.attrs["long_name"]
        assert dataset.quality.attrs["long_name"] == "quality flags"
        assert dataset.x.size == field.shape[1]
        assert dataset.y.size == field.shape[0]
        assert dataset.reflectivity.encoding["coordinates"] == "time y x"
        assert dataset.time.attrs["standard_name"] == "time"
        assert dataset.crs.attrs["spatial_ref"] == "EPSG:3821"
        assert (nc.with_name(nc.name + ".manifest.json")).exists()
