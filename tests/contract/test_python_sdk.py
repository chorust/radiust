from __future__ import annotations

import asyncio
import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
from radiust import (
    AsyncClient,
    AsyncContextError,
    Client,
    FrameRef,
    Query,
    RadarField,
    TransportError,
)
from radiust.config import load_config

from tests.support.native_raw_cache import seed_native_raw_cache

QUERY = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))


def _offline_config():
    return load_config(
        {"runtime": {"allow_network": False}, "cache": {"enabled": False}},
        environ={},
    )


def _frame() -> FrameRef:
    return FrameRef(
        "my",
        "composite",
        QUERY.at,
        station="east",
        locator={
            "url": "https://www.met.gov.my/data/radar_east.gif",
            "artifacts": [],
            "station": "east",
            "name": "my_east.png",
            "media_type": "image/png",
        },
        locator_version="my-legacy-v1",
        revision="offline-sdk-contract",
    )


def _field() -> RadarField:
    return RadarField(
        json.dumps(
            {
                "name": "reflectivity",
                "values": [1.5, 2.5, 3.5, 4.5],
                "shape": [2, 2],
                "quality": [0, 1, 2, 3],
                "units": "dBZ",
                "valid_time": "2025-12-29T06:50:01Z",
                "grid": {
                    "shape": [2, 2],
                    "crs": "EPSG:4326",
                    "x": [100.0, 101.0],
                    "y": [20.0, 21.0],
                    "affine": None,
                },
                "provenance": ["offline-sdk-contract"],
            }
        )
    )


def test_acquire_context_owns_verified_cached_raw_lifetime(tmp_path):
    cache_dir = tmp_path / "cache"
    output_root = tmp_path / "output"
    temp_root = tmp_path / "temp"
    temp_root.mkdir()
    sentinel = temp_root / "keep.txt"
    sentinel.write_text("caller-owned", encoding="utf-8")
    ref = _frame()
    payload = b"verified cached raw frame"
    seed_native_raw_cache(cache_dir, ref, [("my_east.png", "image/png", payload)])
    config = load_config(
        {
            "runtime": {"allow_network": False, "temp_root": str(temp_root)},
            "cache": {"enabled": True, "dir": str(cache_dir)},
            "storage": {"output": str(output_root)},
        },
        environ={},
    )
    with Client(config=config) as client:
        with client.acquire(ref) as raw:
            stage_path = Path(raw.artifact_path(0))
            assert stage_path.is_file()
            assert raw.artifact_bytes(0) == payload
        assert not stage_path.exists()
        with pytest.raises(RuntimeError, match="RawFrame is closed"):
            raw.artifact_bytes(0)

    assert sentinel.read_text(encoding="utf-8") == "caller-owned"


def test_sync_client_rejects_use_inside_running_event_loop():
    async def use_sync_client() -> None:
        with Client(config=_offline_config()) as client, pytest.raises(AsyncContextError):
            client.discover(QUERY)

    asyncio.run(use_sync_client())


def test_sync_and_async_fetch_report_the_same_network_failure():
    # Both public clients must preserve the network policy failure from discovery.
    with Client(config=_offline_config()) as sync, pytest.raises(TransportError) as sync_error:
        sync.fetch(QUERY)

    async def fetch_offline():
        async with AsyncClient(config=_offline_config()) as client:
            with pytest.raises(TransportError) as async_error:
                await client.fetch(QUERY)
            return async_error.value

    async_error = asyncio.run(fetch_offline())
    assert (sync_error.value.code, sync_error.value.message) == (
        async_error.code,
        async_error.message,
    )


def test_download_rejects_unknown_processing_options_before_acquisition(tmp_path):
    with Client(config=_offline_config()) as client, pytest.raises(
        ValueError, match="unsupported processing"
    ):
        client.download(QUERY, output=tmp_path, unsupported=True)


def test_sync_and_async_write_return_download_reports(tmp_path):
    ref = _frame()
    with Client(config=_offline_config()) as client:
        report = client.write(_field(), output=tmp_path, ref=ref)
    assert report.command == "write"
    assert report.counts["written"] == 1

    async def write_async():
        async with AsyncClient(config=_offline_config()) as client:
            return await client.write(_field(), output=tmp_path / "async", ref=ref)

    async_report = asyncio.run(write_async())
    assert async_report.counts["written"] == 1
