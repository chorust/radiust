from datetime import datetime, timezone

from radiust import Client, Query
from radiust.outputs.netcdf import read_netcdf


def test_mvp_fixture_round_trips_netcdf(tmp_path):
    query = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))
    with Client() as client:
        field = client.fetch(query)
        report = client.download(query, output=tmp_path, raw=True)
    assert report.counts["written"] == 1
    nc = next(tmp_path.rglob("*.nc"))
    dataset = read_netcdf(nc)
    assert dataset.reflectivity.shape == field.data.shape
    assert dataset.quality.dtype.name == "uint16"
    assert dataset.attrs["Conventions"] == "CF-1.8"
    assert dataset.reflectivity.attrs["ancillary_variables"] == "quality"
    assert dataset.reflectivity.attrs["long_name"]
    assert dataset.quality.attrs["long_name"] == "quality flags"
    assert dataset.latitude.attrs["standard_name"] == "latitude"
    assert dataset.latitude.attrs["units"] == "degrees_north"
    assert dataset.longitude.attrs["standard_name"] == "longitude"
    assert dataset.longitude.attrs["units"] == "degrees_east"
    assert dataset.time.attrs["standard_name"] == "time"
    assert (nc.with_name(nc.name + ".manifest.json")).exists()
