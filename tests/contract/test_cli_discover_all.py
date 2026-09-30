"""The Python command forwards aggregate discovery to the native CLI."""

from __future__ import annotations

import json
import os

import pytest
from click.testing import CliRunner
from radiust.cli.main import main


@pytest.fixture(autouse=True)
def isolate_native_cli_configuration(monkeypatch: pytest.MonkeyPatch, tmp_path) -> None:
    for key in tuple(os.environ):
        if key.startswith("RADIUST_"):
            monkeypatch.delenv(key, raising=False)
    isolated_home = tmp_path / "home"
    isolated_home.mkdir()
    monkeypatch.setenv("HOME", str(isolated_home))
    monkeypatch.setenv("RADIUST_RUNTIME__ALLOW_NETWORK", "false")
    monkeypatch.chdir(tmp_path)


@pytest.mark.parametrize(
    "selector",
    [
        ("--at", "2026-09-22T00:00:00Z"),
        ("--start", "2026-09-22T00:00:00Z"),
        ("--end", "2026-09-23T00:00:00Z"),
        ("--base-time", "2026-09-22T00:00:00Z"),
        ("--product", "rain"),
        ("--station", "station-a"),
    ],
)
def test_native_discover_all_rejects_single_source_selectors(selector: tuple[str, str]) -> None:
    result = CliRunner().invoke(main, ["discover", "all", *selector, "--json"])

    assert result.exit_code == 2, result.output
    report = json.loads(result.output)
    assert report["schema_version"] == 1
    assert report["error"]["stage"] == "validate"
    assert report["items"] == []


def test_native_discover_all_returns_one_offline_aggregate_report() -> None:
    result = CliRunner().invoke(main, ["discover", "all", "--json"])

    assert result.exit_code == 5, result.output
    report = json.loads(result.output)
    assert report["schema_version"] == 1
    assert report["query"] == {"source": "all", "latest": True, "max_age": None}
    assert report["counts"]["total"] == 26
    assert report["counts"]["total"] == len(report["items"])
    assert report["counts"]["network_restricted"] > 0
    ph = next(item for item in report["items"] if item["source"] == "ph")
    assert ph["status"] == "network_restricted"


def test_native_discover_all_keeps_max_age_in_the_report() -> None:
    result = CliRunner().invoke(main, ["discover", "all", "--max-age", "600", "--json"])

    assert result.exit_code in {0, 3, 4, 5}, result.output
    report = json.loads(result.output)
    assert report["query"]["latest"] is True
    assert report["query"]["max_age"] == 600.0


@pytest.mark.parametrize("age", ["0", "-1"])
def test_native_discover_all_rejects_invalid_max_age(age: str) -> None:
    result = CliRunner().invoke(main, ["discover", "all", "--max-age", age, "--json"])

    assert result.exit_code == 2, result.output
    report = json.loads(result.output)
    assert report["error"]["stage"] == "validate"
    assert report["items"] == []
