"""Opt-in provider smoke tests through the Rust CLI and source adapters."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any

import pytest
from click.testing import CliRunner
from radiust.cli.main import main

PUBLIC_CASES = (
    ("cam", ()),
    ("ca", ("CASFT",)),
    ("au", ("AU02",)),
    ("rainviewer", ()),
    ("fr", ()),
    ("kr", ()),
    ("tw-http", ("CV1_3600",)),
    ("es", ("ESCOMP",)),
    ("pt", ("PTST2",)),
    ("sg", ("SGCOMP",)),
    ("th", ("cmp1",)),
    ("th", ("kkn240Loop",)),
    ("th_royalrain", ("takhli",)),
    ("nz", ()),
    ("vn", ()),
    ("windy", ()),
)


def _native_config(
    tmp_path: Path, source_options: dict[str, Any] | None = None
) -> tuple[Path, Path]:
    output = tmp_path / "output"
    document: dict[str, Any] = {
        "runtime": {
            "allow_network": True,
            "request_timeout": 20.0,
            "frame_deadline": 90.0,
            "max_artifact_bytes": 64 * 1024 * 1024,
            "max_frame_bytes": 128 * 1024 * 1024,
            "temp_root": str(tmp_path / "temp"),
        },
        "cache": {"enabled": False, "dir": str(tmp_path / "cache")},
        "storage": {"output": str(output)},
    }
    if source_options:
        source_id, options = next(iter(source_options.items()))
        document["sources"] = {source_id: options}
    config = tmp_path / "radiust-test-config.json"
    config.write_text(json.dumps(document), encoding="utf-8")
    return config, output


def _invoke_native(
    tmp_path: Path,
    args: list[str],
    *,
    source_options: dict[str, Any] | None = None,
) -> tuple[dict[str, Any], Path]:
    config, output = _native_config(tmp_path, source_options)
    result = CliRunner().invoke(main, ["--conf", str(config), "--json", *args])
    assert result.exit_code == 0, result.output
    report = json.loads(result.output)
    assert report.get("error") is None, report
    return report, output


def _raw_only_args(source_id: str, stations: tuple[str, ...]) -> list[str]:
    args = ["download", source_id, "--latest", "--raw-only", "--no-cache"]
    for station in stations:
        args.extend(("--station", station))
    return args


def _assert_raw_receipts(report: dict[str, Any]) -> list[dict[str, Any]]:
    items = report.get("items", [])
    assert items, report
    assert all(item["status"] == "written" for item in items), report
    receipts: list[dict[str, Any]] = []
    for item in items:
        manifest_path = Path(item["output_uri"])
        assert manifest_path.is_absolute() and manifest_path.is_file(), item
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        artifacts = manifest.get("artifacts", [])
        assert artifacts, manifest
        assert all(
            isinstance(artifact.get("size_bytes"), int)
            and artifact["size_bytes"] > 0
            and len(artifact.get("sha256", "")) == 64
            for artifact in artifacts
        ), manifest
        receipts.extend(artifacts)
    return receipts


pytestmark = pytest.mark.live


@pytest.mark.parametrize(("source_id", "stations"), PUBLIC_CASES)
def test_public_latest_raw_smoke(
    source_id: str, stations: tuple[str, ...], tmp_path: Path
) -> None:
    report, _output = _invoke_native(tmp_path, _raw_only_args(source_id, stations))
    _assert_raw_receipts(report)


@pytest.mark.provider
def test_sidarma_latest_raw_provider_smoke(tmp_path: Path) -> None:
    api_key = os.environ.get("RADIUST_TEST_ID_SIDARMA_API_KEY")
    if not api_key:
        pytest.skip("RADIUST_TEST_ID_SIDARMA_API_KEY is not configured")
    report, _output = _invoke_native(
        tmp_path,
        _raw_only_args("id_sidarma", ("JAK",)),
        source_options={"id_sidarma": {"api_key": api_key, "radar_ids": ["JAK"]}},
    )
    assert all(artifact["sha256"] for artifact in _assert_raw_receipts(report))
    assert api_key not in json.dumps(report)


@pytest.mark.provider
def test_pagasa_latest_raw_provider_smoke(tmp_path: Path) -> None:
    report, _output = _invoke_native(tmp_path, _raw_only_args("ph", ()))
    _assert_raw_receipts(report)


@pytest.mark.provider
def test_wunderground_latest_raw_provider_smoke(tmp_path: Path) -> None:
    api_key = os.environ.get("RADIUST_TEST_WUNDERGROUND_API_KEY")
    if not api_key:
        pytest.skip("RADIUST_TEST_WUNDERGROUND_API_KEY is not configured")
    report, _output = _invoke_native(
        tmp_path,
        _raw_only_args("wunderground", ()),
        source_options={"wunderground": {"api_key": api_key}},
    )
    assert len(_assert_raw_receipts(report)) == 4
    assert api_key not in json.dumps(report)


def test_tw_native_numeric_grid_latest_scientific_smoke(tmp_path: Path) -> None:
    xr = pytest.importorskip("xarray")
    report, _output = _invoke_native(
        tmp_path,
        [
            "download",
            "tw",
            "--product",
            "grid",
            "--station",
            "CV1_3600",
            "--latest",
            "--format",
            "netcdf",
            "--no-cache",
        ],
    )
    items = report.get("items", [])
    assert len(items) == 1 and items[0]["status"] == "written", report
    with xr.open_dataset(items[0]["output_uri"]) as dataset:
        field = dataset["reflectivity"]
        assert field.shape == (881, 921)
        assert field.attrs["units"] == "dBZ"
        assert dataset["crs"].attrs["spatial_ref"] == "EPSG:3821"
        assert dataset["quality"].dtype.kind == "u"
