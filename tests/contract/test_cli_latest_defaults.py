from __future__ import annotations

import json

import pytest
from click.testing import CliRunner
from radiust.cli.main import main


def test_discover_and_download_dry_run_default_equal_explicit_latest():
    runner = CliRunner()
    for command in ("discover", "download"):
        options = ["my", "--json"]
        if command == "download":
            options.append("--dry-run")
        implicit = runner.invoke(main, [command, *options])
        explicit = runner.invoke(main, [command, *options, "--latest"])
        assert implicit.exit_code == explicit.exit_code, (implicit.output, explicit.output)
        i, e = json.loads(implicit.output), json.loads(explicit.output)
        if command == "discover":
            assert i["items"] == e["items"]
        else:
            assert i["counts"] == e["counts"]
            assert [x["status"] for x in i["items"]] == [x["status"] for x in e["items"]]


def test_explicit_bad_selector_is_not_replaced_with_latest():
    runner = CliRunner()
    for command in ("discover", "download"):
        assert runner.invoke(main, [command, "my", "--start", "2025-01-01T00:00:00Z"]).exit_code == 2
        assert runner.invoke(main, [command, "my", "--latest", "--at", "2025-01-01T00:00:00Z"]).exit_code == 2
        assert runner.invoke(main, [command, "my", "--at", "not-a-time"]).exit_code == 2


def test_scientific_cat_can_omit_time_on_offline_source():
    implicit = CliRunner().invoke(main, ["cat", "my", "--decoded", "--renderer", "text"])
    explicit = CliRunner().invoke(main, ["cat", "my", "--decoded", "--latest", "--renderer", "text"])
    assert implicit.exit_code == explicit.exit_code
    assert implicit.output == explicit.output


@pytest.mark.parametrize("command", ["discover", "download", "cat"])
def test_default_latest_with_product_and_station_uses_native_cli(command):
    options = ["my", "--product", "composite", "--station", "east", "--json"]
    if command == "download":
        options.append("--dry-run")
    if command == "cat":
        options.extend(["--raw", "--renderer", "text"])
    runner = CliRunner()
    implicit = runner.invoke(main, [command, *options])
    explicit = runner.invoke(main, [command, *options, "--latest"])
    assert implicit.exit_code == explicit.exit_code, (implicit.output, explicit.output)
    assert implicit.output == explicit.output


@pytest.mark.parametrize("command", ["discover", "download"])
def test_native_cli_accepts_selector_forms_and_rejects_conflicts(command):
    options = ["--dry-run"] if command == "download" else []
    runner = CliRunner()
    at = "2026-09-22T00:00:00Z"
    base = "2026-09-21T18:00:00Z"
    for selectors in (
        ["--at", at, "--base-time", base],
        ["--max-age", "600"],
        ["--start", at, "--end", "2026-09-23T00:00:00Z"],
    ):
        result = runner.invoke(main, [command, "my", *options, "--json", *selectors])
        # The request reaches the Rust engine and is rejected only at its
        # network gate because this test intentionally runs offline.
        assert result.exit_code == 2, result.output

    conflict = runner.invoke(
        main, [command, "my", *options, "--latest", "--at", at, "--json"]
    )
    assert conflict.exit_code == 2
    assert "--latest cannot be combined" in conflict.output

    malformed = runner.invoke(main, [command, "my", *options, "--at", "not-a-time", "--json"])
    assert malformed.exit_code == 2
    assert "timezone" in malformed.output.lower() or "iso-8601" in malformed.output.lower()


@pytest.mark.parametrize("command", ["discover", "download", "cat"])
def test_unsupported_latest_spelling_is_rejected(command):
    response = CliRunner().invoke(main, [command, "th", "--lates"])
    assert response.exit_code == 2
    assert "unexpected argument" in response.output.lower()
    assert "--latest" in response.output


def test_cli_file_multitime_netcdf_still_requires_explicit_time(tmp_path):
    import numpy as np
    import xarray as xr

    path = tmp_path / "two-times.nc"
    dataset = xr.Dataset(
        {"reflectivity": (("time", "latitude", "longitude"), np.ones((2, 1, 1), dtype="float32"))},
        coords={"time": np.array(["2026-09-20T00:00:00", "2026-09-21T00:00:00"], dtype="datetime64[ns]"),
                "latitude": [0.0], "longitude": [0.0]},
    )
    dataset.to_netcdf(path, engine="h5netcdf")
    response = CliRunner().invoke(main, ["cat", "--file", str(path), "--renderer", "text"])
    assert response.exit_code == 2
    assert "multi-time netcdf requires --at" in response.output.lower()
