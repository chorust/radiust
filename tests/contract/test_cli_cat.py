from __future__ import annotations

import json
from pathlib import Path

import numpy as np
import xarray as xr
from click.testing import CliRunner
from radiust.cli.main import main

FIXTURE = Path("tests/fixtures/sources/my/raw/east.png")

def _set_cf_time(dataset: xr.Dataset, values: np.ndarray) -> None:
    dataset.coords["time"] = values
    dataset["time"].attrs["units"] = "seconds since 1970-01-01 00:00:00 UTC"
    dataset["time"].attrs["calendar"] = "proleptic_gregorian"


def test_cat_requires_exactly_one_source_or_file():
    runner = CliRunner()
    neither = runner.invoke(main, ["cat"])
    both = runner.invoke(main, ["cat", "my", "--file", str(FIXTURE)])

    assert neither.exit_code == 2
    assert "exactly one" in neither.output
    assert both.exit_code == 2
    assert "exactly one" in both.output


def test_cat_file_text_is_redirectable_and_image_renderer_requires_tty():
    runner = CliRunner()
    text = runner.invoke(main, ["cat", "--file", str(FIXTURE), "--renderer", "text"])
    ansi = runner.invoke(main, ["cat", "--file", str(FIXTURE), "--renderer", "ansi"])

    assert text.exit_code == 0, text.output
    assert "image path=east.png" in text.output
    assert "\x1b[" not in text.output
    assert ansi.exit_code == 2
    assert "requires a TTY" in ansi.output


def test_cat_decoded_source_requires_a_native_science_product():
    runner = CliRunner()
    result = runner.invoke(main, ["cat", "my", "--decoded", "--renderer", "text"])

    assert result.exit_code == 2
    assert "native scientific preview supports only" in result.output
    assert "selected my" in result.output


def test_cat_file_allows_variable_selection_for_netcdf(tmp_path):
    data = xr.Dataset(
        {
            "reflectivity": (("latitude", "longitude"), np.ones((2, 2), dtype="float32")),
            "rain_rate": (("latitude", "longitude"), np.full((2, 2), 3.0, dtype="float32")),
        },
        coords={"latitude": [1.0, 0.0], "longitude": [0.0, 1.0]},
    )
    _set_cf_time(data, np.int64(1_767_225_600))
    path = tmp_path / "multi.nc"
    data.to_netcdf(path, engine="h5netcdf", format="NETCDF4")

    result = CliRunner().invoke(main, ["cat", "--file", str(path), "--renderer", "text", "--variable", "rain_rate"])

    assert result.exit_code == 0, result.output
    assert "variable=rain_rate" in result.output


def test_cat_file_requires_at_to_select_a_multitime_netcdf(tmp_path):
    data = xr.Dataset(
        {
            "reflectivity": (
                ("time", "latitude", "longitude"),
                np.stack(
                    [
                        np.ones((2, 2), dtype="float32"),
                        np.full((2, 2), 3.0, dtype="float32"),
                    ]
                ),
            )
        },
        coords={
            "time": np.array([1_735_689_600, 1_735_693_200], dtype="int64"),
            "latitude": [1.0, 0.0],
            "longitude": [0.0, 1.0],
        },
        attrs={"radiust_provenance": json.dumps({"source": "test", "product": "composite"})},
    )
    data["time"].attrs["units"] = "seconds since 1970-01-01 00:00:00 UTC"
    data["time"].attrs["calendar"] = "proleptic_gregorian"
    path = tmp_path / "time.nc"
    data.to_netcdf(path, engine="h5netcdf", format="NETCDF4")

    result = CliRunner().invoke(
        main,
        [
            "cat",
            "--file",
            str(path),
            "--renderer",
            "text",
            "--variable",
            "reflectivity",
            "--at",
            "2025-01-01T01:00:00Z",
        ],
    )

    assert result.exit_code == 0, result.output
    assert "time=2025-01-01T01:00:00Z" in result.output
    assert "size=2x2" in result.output


def test_cat_file_selection_errors_are_usage_errors(tmp_path):
    data = xr.Dataset(
        {
            "reflectivity": (
                ("time", "latitude", "longitude"),
                np.stack(
                    [
                        np.ones((2, 2), dtype="float32"),
                        np.full((2, 2), 3.0, dtype="float32"),
                    ]
                ),
            )
        },
        coords={
            "time": np.array([1_735_689_600, 1_735_693_200], dtype="int64"),
            "latitude": [1.0, 0.0],
            "longitude": [0.0, 1.0],
        },
    )
    data["time"].attrs["units"] = "seconds since 1970-01-01 00:00:00 UTC"
    data["time"].attrs["calendar"] = "proleptic_gregorian"
    path = tmp_path / "time.nc"
    data.to_netcdf(path, engine="h5netcdf", format="NETCDF4")

    invalid_time = CliRunner().invoke(
        main,
        [
            "cat",
            "--file",
            str(path),
            "--renderer",
            "text",
            "--variable",
            "reflectivity",
            "--at",
            "not-a-time",
        ],
    )
    missing_frame = CliRunner().invoke(
        main,
        [
            "cat",
            "--file",
            str(path),
            "--renderer",
            "text",
            "--variable",
            "reflectivity",
            "--at",
            "2025-01-01T02:00:00Z",
        ],
    )

    assert invalid_time.exit_code == 2
    assert "time must be ISO-8601" in invalid_time.output
    assert missing_frame.exit_code == 2
    assert "has no frame" in missing_frame.output


def test_cat_file_missing_variable_is_a_usage_error(tmp_path):
    data = xr.Dataset(
        {"reflectivity": (("latitude", "longitude"), np.ones((2, 2), dtype="float32"))},
        coords={"latitude": [1.0, 0.0], "longitude": [0.0, 1.0]},
    )
    _set_cf_time(data, np.int64(1_735_689_600))
    path = tmp_path / "single.nc"
    data.to_netcdf(path, engine="h5netcdf", format="NETCDF4")

    result = CliRunner().invoke(
        main,
        ["cat", "--file", str(path), "--renderer", "text", "--variable", "rain_rate"],
    )

    assert result.exit_code == 2
    assert "rain_rate" in result.output
