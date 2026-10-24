from __future__ import annotations

import asyncio
import hashlib
import json
import os
import shutil
import subprocess
from collections.abc import Mapping
from functools import lru_cache
from datetime import datetime, timezone
from pathlib import Path

import pytest
from radiust import AsyncClient, Client, _bridge
from radiust.config import load_config

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/sources"
TEST_ENGINE = getattr(_bridge._core.Engine, "for_local_view_test_fixture", None)
# One synthetic observation time across all entry points and cache replays.
FIXTURE_VALID_TIME = datetime.now(timezone.utc).isoformat()
pytestmark = pytest.mark.skipif(
    TEST_ENGINE is None,
    reason=(
        "build radiust-python with extension-module,test-support before running "
        "the cross-entry replay"
    ),
)


def _config(root: Path, source: str, *, options: dict | None = None):
    source_options = {
        "fixture_root": str(FIXTURES / source / "raw"),
        "fixture_discovery_log": str(root / "discovery.log"),
        "fixture_fetch_log": str(root / "fetch.log"),
        "fixture_valid_time": FIXTURE_VALID_TIME,
        **(options or {}),
    }
    values = {
        "runtime": {"allow_network": False},
        "cache": {"enabled": True, "dir": str(root / "cache")},
        "location": {"longitude": -16.9, "latitude": 32.7}
        if source == "pt"
        else {"longitude": 22.67578125, "latitude": 71.5805318},
        "local_view": {"radius_km": 1, "resolution_m": 1000},
        "sources": {source: source_options},
    }
    root.mkdir(parents=True, exist_ok=True)
    path = root / "config.json"
    path.write_text(json.dumps(values), encoding="utf-8")
    return load_config(values, environ={}), path, root / "fetch.log"


def _fixture_engine(client):
    client._session._engine = TEST_ENGINE(client.config._native_config)


@lru_cache(maxsize=1)
def _cli_path() -> str:
    configured = os.environ.get("RADIUST_TEST_LOCAL_VIEW_BIN")
    if configured:
        return configured
    target_dir = ROOT / "target/local-view-test-support"
    build = subprocess.run(
        [
            shutil.which("cargo") or "cargo",
            "build",
            "--manifest-path",
            "crates/radiust-cli/Cargo.toml",
            "--target-dir",
            str(target_dir),
            "--features",
            "test-support",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    assert build.returncode == 0, build.stderr
    executable = "radiust.exe" if os.name == "nt" else "radiust"
    return str(target_dir / "debug" / executable)


def _sdk_call(client, source: str, mode: str, radius_km: float = 1, resampling: str = "nearest"):
    center = (-16.9, 32.7) if source == "pt" else (22.67578125, 71.5805318)
    return client.local_view(
        center=center,
        radius_km=radius_km,
        resolution_m=1000,
        source=source,
        product="composite",
        mode=mode,
        resampling=resampling,
    )


def _async_call(client, source: str, mode: str, radius_km: float = 1, resampling: str = "nearest"):
    center = (-16.9, 32.7) if source == "pt" else (22.67578125, 71.5805318)
    return client.local_view(
        center=center,
        radius_km=radius_km,
        resolution_m=1000,
        source=source,
        product="composite",
        mode=mode,
        resampling=resampling,
    )


def _cli_call(config_path: Path, source: str, mode: str, radius_km: int = 1, resampling: str | None = None):
    args = [
        _cli_path(),
        "--json",
        "--conf",
        str(config_path),
        "cat",
        source,
        "--local",
        "--radius-km",
        str(radius_km),
        "--resolution-m",
        "1000",
        "--product",
        "composite",
        "--renderer",
        "text",
    ]
    if mode != "auto":
        args.append(f"--{mode}")
    if resampling:
        args.extend(["--resampling", resampling])
    environment = os.environ.copy()
    environment["RADIUST_TEST_LOCAL_VIEW_FIXTURE"] = "1"
    completed = subprocess.run(args, capture_output=True, text=True, env=environment, check=False)
    assert completed.returncode in (0, 3, 5), completed.stderr + completed.stdout
    return completed.returncode, json.loads(completed.stdout)


def _identity(result):
    selected = result.selected
    grid = result.grid
    processing = result.processing
    return {
        "source": selected["source"],
        "product": selected["product"],
        "station": selected["station"],
        "valid_time": selected["valid_time"],
        "logical_id": selected["logical_id"],
        "revision": selected["revision"],
        "processing_id": processing["processing_id"],
        "input_logical_id": processing["input_logical_id"],
        "input_receipts": _plain(processing["input_receipts"]),
        "center": dict(grid.center),
        "radius_m": grid.radius_m,
        "requested_resolution_m": grid.requested_resolution_m,
        "actual_resolution_m": grid.actual_resolution_m,
        "shape": list(grid.shape),
        "affine": list(grid.affine) if grid.affine is not None else None,
        "data_kind": processing["data_kind"],
        "resampling": processing["resampling"],
        "requested_mode": processing["requested_mode"],
    }


def _data_fingerprints(result):
    if result.raw_rgba is not None:
        return {
            "data_kind": "raw_image",
            "shape": list(result.grid.shape),
            "rgba_sha256": hashlib.sha256(result.raw_rgba).hexdigest(),
        }
    fields = []
    for field in result.fields:
        metadata = json.loads(field.metadata_json())
        fields.append(
            {
                "name": metadata["name"],
                "units": metadata["units"],
                "shape": metadata["shape"],
                "values_sha256": hashlib.sha256(field.values_le_bytes()).hexdigest(),
                "quality_sha256": hashlib.sha256(field.quality_le_bytes()).hexdigest(),
            }
        )
    return {"data_kind": "numeric", "fields": fields}


def _plain(value):
    if isinstance(value, Mapping):
        return {key: _plain(item) for key, item in value.items()}
    if isinstance(value, (tuple, list)):
        return [_plain(item) for item in value]
    return value


def _cli_identity(document):
    result = document["result"]
    selected = result["selected"]
    processing = result["processing"]
    return {
        "source": selected["source"],
        "product": selected["product"],
        "station": selected["station"],
        "valid_time": selected["valid_time"],
        "logical_id": selected["logical_id"],
        "revision": selected["revision"],
        "processing_id": processing["processing_id"],
        "input_logical_id": processing["input_logical_id"],
        "input_receipts": processing["input_receipts"],
        "center": result["center"],
        "radius_m": result["radius_m"],
        "requested_resolution_m": result["requested_resolution_m"],
        "actual_resolution_m": result["actual_resolution_m"],
        "shape": result["shape"],
        "affine": result["affine"],
        "data_kind": processing["data_kind"],
        "resampling": processing["resampling"],
        "requested_mode": processing["requested_mode"],
    }


def _log_count(path: Path) -> int:
    return len(path.read_text(encoding="utf-8").splitlines()) if path.exists() else 0


def test_categorical_source_is_excluded_before_discovery_across_entries(tmp_path):
    sync_root = tmp_path / "sync"
    sync_config, _, sync_fetch_log = _config(sync_root, "sg")
    with Client(config=sync_config) as client:
        _fixture_engine(client)
        sync_result = _sdk_call(client, "sg", "auto", resampling="bilinear")

    async_root = tmp_path / "async"
    async_config, _, async_fetch_log = _config(async_root, "sg")

    async def run_async():
        async with AsyncClient(config=async_config) as client:
            _fixture_engine(client)
            return await _async_call(client, "sg", "auto", resampling="bilinear")

    async_result = asyncio.run(run_async())
    cli_root = tmp_path / "cli"
    _, config_path, cli_fetch_log = _config(cli_root, "sg")
    cli_code, cli_result = _cli_call(config_path, "sg", "auto", resampling="bilinear")

    sync_metadata = json.loads(sync_result.to_json())
    async_metadata = json.loads(async_result.to_json())
    assert sync_result.status == async_result.status == "no_candidate"
    assert cli_code == 3 and cli_result["status"] == "no_candidate"
    for selection in (
        sync_metadata["selection"],
        async_metadata["selection"],
        cli_result["result"]["selection"],
    ):
        assert selection
        assert all(item["reason"] == "unsupported_mode_or_geometry" for item in selection)
    logs = (
        sync_root / "discovery.log",
        async_root / "discovery.log",
        cli_root / "discovery.log",
        sync_fetch_log,
        async_fetch_log,
        cli_fetch_log,
    )
    assert all(_log_count(path) == 0 for path in logs)


def test_fixed_pt_raw_frame_matches_cli_sync_async_and_survives_view_cache_hit(tmp_path):
    sync_config, _, sync_log = _config(tmp_path / "sync", "pt")
    with Client(config=sync_config) as client:
        _fixture_engine(client)
        sync_first = _sdk_call(client, "pt", "raw", 1)
        sync_second = _sdk_call(client, "pt", "raw", 2)

    async_config, _, async_log = _config(tmp_path / "async", "pt")

    async def run_async():
        async with AsyncClient(config=async_config) as client:
            _fixture_engine(client)
            return (
                await _async_call(client, "pt", "raw", 1),
                await _async_call(client, "pt", "raw", 2),
            )

    async_first, async_second = asyncio.run(run_async())
    cli_config, config_path, cli_log = _config(tmp_path / "cli", "pt")
    del cli_config
    first_code, cli_first = _cli_call(config_path, "pt", "raw", 1)
    second_code, cli_second = _cli_call(config_path, "pt", "raw", 2)

    assert sync_first.status == async_first.status == "selected"
    assert first_code == second_code == 0
    assert _identity(sync_first) == _identity(async_first) == _cli_identity(cli_first)
    assert _data_fingerprints(sync_first) == _data_fingerprints(async_first)
    assert _data_fingerprints(sync_first) == cli_first["result"]["test_support_data_fingerprints"]
    assert sync_first.raw_rgba == async_first.raw_rgba
    assert sync_second.selected["valid_time"] == sync_first.selected["valid_time"]
    assert sync_second.selected["logical_id"] == sync_first.selected["logical_id"]
    assert sync_second.processing["input_receipts"] == sync_first.processing["input_receipts"]
    assert sync_second.processing["processing_id"] != sync_first.processing["processing_id"]
    assert async_second.selected["logical_id"] == async_first.selected["logical_id"]
    assert async_second.processing["input_receipts"] == async_first.processing["input_receipts"]
    assert async_second.processing["processing_id"] != async_first.processing["processing_id"]
    assert cli_second["result"]["selected"]["logical_id"] == cli_first["result"]["selected"]["logical_id"]
    assert cli_second["result"]["processing"]["input_receipts"] == cli_first["result"]["processing"]["input_receipts"]
    assert cli_second["result"]["processing"]["processing_id"] != cli_first["result"]["processing"]["processing_id"]
    assert [_log_count(path) for path in (sync_log, async_log, cli_log)] == [1, 1, 1]


@pytest.mark.parametrize("mode", ["decoded", "dbz"])
def test_fixed_rainviewer_numeric_frame_matches_cli_sync_async(tmp_path, mode):
    sync_config, _, sync_log = _config(tmp_path / "sync", "rainviewer")
    with Client(config=sync_config) as client:
        _fixture_engine(client)
        sync_result = _sdk_call(client, "rainviewer", mode)

    async_config, _, async_log = _config(tmp_path / "async", "rainviewer")

    async def run_async():
        async with AsyncClient(config=async_config) as client:
            _fixture_engine(client)
            return await _async_call(client, "rainviewer", mode)

    async_result = asyncio.run(run_async())
    _, config_path, cli_log = _config(tmp_path / "cli", "rainviewer")
    code, cli_result = _cli_call(config_path, "rainviewer", mode)

    assert sync_result.status == async_result.status == "selected"
    assert code == 0
    assert _identity(sync_result) == _identity(async_result) == _cli_identity(cli_result)
    assert _data_fingerprints(sync_result) == _data_fingerprints(async_result)
    assert _data_fingerprints(sync_result) == cli_result["result"]["test_support_data_fingerprints"]
    assert sync_result.field().values_le_bytes() == async_result.field().values_le_bytes()
    assert sync_result.field().quality_le_bytes() == async_result.field().quality_le_bytes()
    assert sync_result.field().metadata_json() == async_result.field().metadata_json()
    assert [_log_count(path) for path in (sync_log, async_log, cli_log)] == [1, 1, 1]


def test_unsupported_raw_geometry_and_native_failure_never_return_raw(tmp_path):
    pt_config, _, pt_log = _config(tmp_path / "pt", "pt")
    with Client(config=pt_config) as client:
        _fixture_engine(client)
        excluded = _sdk_call(client, "pt", "auto", resampling="bilinear")

    async_pt_config, _, async_pt_log = _config(tmp_path / "async-pt", "pt")

    async def run_async_exclusion():
        async with AsyncClient(config=async_pt_config) as client:
            _fixture_engine(client)
            return await _async_call(client, "pt", "auto", resampling="bilinear")

    async_excluded = asyncio.run(run_async_exclusion())
    _, cli_pt_path, cli_pt_log = _config(tmp_path / "cli-pt", "pt")
    cli_code, cli_excluded = _cli_call(cli_pt_path, "pt", "auto", resampling="bilinear")
    assert excluded.status == async_excluded.status == "no_candidate"
    assert excluded.raw_rgba is async_excluded.raw_rgba is None
    assert cli_code == 3 and cli_excluded["status"] == "no_candidate"
    assert all(_log_count(path) == 0 for path in (pt_log, async_pt_log, cli_pt_log))

    failure_options = {"fixture_native_failure": True}
    sync_config, _, sync_log = _config(tmp_path / "failure-sync", "rainviewer", options=failure_options)
    with Client(config=sync_config) as client:
        _fixture_engine(client)
        sync_failed = _sdk_call(client, "rainviewer", "dbz")

    async_config, _, async_log = _config(tmp_path / "failure-async", "rainviewer", options=failure_options)

    async def run_async_failure():
        async with AsyncClient(config=async_config) as client:
            _fixture_engine(client)
            return await _async_call(client, "rainviewer", "dbz")

    async_failed = asyncio.run(run_async_failure())
    _, cli_config_path, cli_log = _config(
        tmp_path / "failure-cli", "rainviewer", options=failure_options
    )
    cli_failure_code, cli_failed = _cli_call(cli_config_path, "rainviewer", "dbz")

    assert sync_failed.status == async_failed.status == cli_failed["status"]
    assert sync_failed.raw_rgba is async_failed.raw_rgba is None
    assert sync_failed.error["code"] == async_failed.error["code"] == cli_failed["result"]["error"]["code"]
    assert cli_failure_code in (3, 5)
    assert cli_failed["status"] != "selected" and cli_failed["result"]["selected"] is None
    assert [_log_count(path) for path in (sync_log, async_log, cli_log)] == [1, 1, 1]


def test_explicit_stale_fixture_is_rejected_before_acquisition(tmp_path):
    config, _, fetch_log = _config(
        tmp_path, "rainviewer",
        options={"fixture_valid_time": "2000-01-01T00:00:00Z"},
    )
    with Client(config=config) as client:
        _fixture_engine(client)
        result = _sdk_call(client, "rainviewer", "decoded")
    assert result.status == "failed"
    assert result.error["code"] == "stale"
    assert _log_count(fetch_log) == 0
