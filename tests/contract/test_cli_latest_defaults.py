from __future__ import annotations

import json
from datetime import datetime, timezone

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
def test_default_latest_query_matches_explicit_with_product_and_station(monkeypatch, command):
    from tests.support.cli_experience import image_bytes

    queries = []

    def capture_raw(query, _config, **_kwargs):
        queries.append(query)
        from radiust.display.raw import preview_bytes
        return preview_bytes(image_bytes(), name="frame.png", source=query.source,
                            product=query.product, station=query.stations[0])

    def capture_client(_client, query):
        queries.append(query)
        return []

    options = ["th", "--product", "composite", "--station", "a"]
    if command == "download":
        options.extend(["--dry-run", "--json"])
    if command == "cat":
        monkeypatch.setattr("radiust.cli.cat.preview_source_raw", capture_raw)
        options.extend(["--raw", "--renderer", "text"])
    else:
        from radiust.client import Client

        monkeypatch.setattr(Client, "discover", capture_client)
    runner = CliRunner()
    implicit = runner.invoke(main, [command, *options])
    explicit = runner.invoke(main, [command, *options, "--latest"])
    assert implicit.exit_code == explicit.exit_code == 0, (implicit.output, explicit.output)
    assert len(queries) == 2 and queries[0] == queries[1]
    assert queries[0].latest and queries[0].product == "composite" and queries[0].stations == ("a",)


@pytest.mark.parametrize("command", ["discover", "download"])
def test_explicit_time_base_time_and_max_age_preserve_selector(monkeypatch, command):
    from radiust.client import Client

    from tests.support.cli_experience import frame

    queries = []

    def discover(_client, query):
        queries.append(query)
        return [frame(station="a")]

    monkeypatch.setattr(Client, "discover", discover)
    options = ["--dry-run"] if command == "download" else []
    runner = CliRunner()
    at = "2026-09-22T00:00:00Z"
    base = "2026-09-21T18:00:00Z"
    explicit = runner.invoke(main, [command, "th", *options, "--at", at, "--base-time", base])
    latest = runner.invoke(main, [command, "th", *options, "--max-age", "600"])
    interval = runner.invoke(main, [command, "th", *options, "--start", at,
                                    "--end", "2026-09-23T00:00:00Z"])
    assert (explicit.exit_code, latest.exit_code, interval.exit_code) == (0, 0, 0)
    assert len(queries) == 3
    assert queries[0].at == datetime(2026, 9, 22, tzinfo=timezone.utc)
    assert queries[0].base_time == datetime(2026, 9, 21, 18, tzinfo=timezone.utc)
    assert not queries[0].latest and queries[0].max_age is None
    assert queries[1].latest and queries[1].max_age.total_seconds() == 600
    assert queries[2].start is not None and queries[2].end is not None and not queries[2].latest


@pytest.mark.parametrize("command", ["discover", "download", "cat"])
def test_unsupported_latest_spelling_is_rejected(command):
    response = CliRunner().invoke(main, [command, "th", "--lates"])
    assert response.exit_code == 2
    assert "no such option" in response.output.lower()


@pytest.mark.parametrize("command", ["discover", "download"])
def test_cli_latest_uses_default_product_and_keeps_each_station(monkeypatch, tmp_path, command):
    from radiust.client import Client
    from radiust.config import load_config
    from radiust.context import SourceContext
    from radiust.models import ProductInfo, SourceInfo, StationInfo
    from radiust.sources.base import FixtureSource

    fixture = tmp_path / "frames.json"
    fixture.write_text(json.dumps({"frames": [
        {"product": "default", "station": "a", "valid_time": "2026-09-20T00:00:00Z"},
        {"product": "default", "station": "a", "valid_time": "2026-09-21T00:00:00Z"},
        {"product": "default", "station": "b", "valid_time": "2026-09-22T00:00:00Z"},
        {"product": "other", "station": "a", "valid_time": "2026-09-22T00:00:00Z"},
    ]}), encoding="utf-8")
    info = SourceInfo("fake", "offline", "1", (
        ProductInfo("default", variables=("reflectivity",), default=True),
        ProductInfo("other", variables=("reflectivity",)),
    ), (StationInfo("a", "A", 0, 0), StationInfo("b", "B", 1, 1)))
    source = FixtureSource(info, fixture)

    def discover(_client, query):
        import asyncio

        context = SourceContext(load_config(environ={}), "fake")
        try:
            return asyncio.run(source.discover(query, context))
        finally:
            context.close()

    monkeypatch.setattr(Client, "discover", discover)
    options = ["--dry-run", "--json"] if command == "download" else ["--json"]
    runner = CliRunner()
    implicit = runner.invoke(main, [command, "fake", *options])
    explicit = runner.invoke(main, [command, "fake", "--latest", *options])
    assert implicit.exit_code == explicit.exit_code == 0, (implicit.output, explicit.output)
    for response in (implicit, explicit):
        items = json.loads(response.output)["items"]
        assert {(item["product"], item["station"]) for item in items} == {
            ("default", "a"), ("default", "b"),
        }


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


def test_cat_explicit_at_and_base_time_never_inject_latest(monkeypatch):
    from tests.support.cli_experience import image_bytes

    captured = []

    def capture(query, _config, **_kwargs):
        captured.append(query)
        from radiust.display.raw import preview_bytes
        return preview_bytes(image_bytes(), name="frame.png", source=query.source,
                            station=query.stations[0] if query.stations else None)

    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", capture)
    base = ["cat", "th", "--raw", "--renderer", "text"]
    result = CliRunner().invoke(main, [*base, "--at", "2026-09-22T00:00:00Z",
                                       "--base-time", "2026-09-21T18:00:00Z"])
    assert result.exit_code == 0, result.output
    assert len(captured) == 1 and not captured[0].latest
    assert captured[0].at == datetime(2026, 9, 22, tzinfo=timezone.utc)
    assert captured[0].base_time == datetime(2026, 9, 21, 18, tzinfo=timezone.utc)
    assert CliRunner().invoke(main, [*base, "--latest", "--at", "2026-09-22T00:00:00Z"]).exit_code == 2
