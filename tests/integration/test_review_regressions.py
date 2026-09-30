from __future__ import annotations

import asyncio
import json
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import numpy as np
import pytest
import radiust._core as core
import xarray as xr
from pyproj import Transformer
from radiust import AsyncClient, Client, FrameRef

from tests.support.native_science_fixture import (
    read_tw_grid_raw,
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


def _ref(source: str = "review-test") -> FrameRef:
    return FrameRef(source, "reflectivity", datetime(2026, 1, 1, tzinfo=timezone.utc), station="s0")


def _field_document(value: float = 1.0, *, name: str = "reflectivity") -> dict[str, object]:
    return {
        "name": name,
        "values": [value] * 4,
        "shape": [2, 2],
        "quality": [0, 0, 0, 0],
        "units": "dBZ",
        "valid_time": "2026-01-01T00:00:00Z",
        "grid": {
            "shape": [2, 2],
            "crs": "EPSG:4326",
            "x": [100.0, 101.0],
            "y": [20.0, 21.0],
            "affine": None,
        },
        "provenance": ["review-regression"],
    }


def _field(value: float = 1.0) -> core.RadarField:
    return core.RadarField(json.dumps(_field_document(value)))


def _tw_cached(tmp_path: Path) -> tuple[FrameRef, dict[str, object]]:
    ref = tw_grid_ref()
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, ref)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")
    return ref, config


async def _wait_for_engine_operation(receiver: Any, operation: str) -> None:
    loop = asyncio.get_running_loop()
    deadline = loop.time() + 10.0
    while loop.time() < deadline:
        for event in json.loads(receiver.drain_json()):
            if event["operation"] == operation and event["stage"] == "started":
                return
        await asyncio.sleep(0.001)
    pytest.fail(f"Rust Engine did not start {operation!r} within 10 seconds")


@pytest.mark.asyncio
async def test_projected_target_coordinates_are_transformed_before_rust_sampling() -> None:
    to_mercator = Transformer.from_crs("EPSG:4326", "EPSG:3857", always_xy=True)
    x0, y0 = to_mercator.transform(10.0, 50.0)
    x1, y1 = to_mercator.transform(11.0, 51.0)
    field_document = {
        **_field_document(),
        "values": [1.0, 2.0, 3.0, 4.0],
        "grid": {
            "shape": [2, 2],
            "crs": "EPSG:3857",
            "x": [x0, x1],
            "y": [y0, y1],
            "affine": None,
        },
    }
    field = core.RadarField(json.dumps(field_document))
    # Rust regridding accepts matching CRS values; project the requested
    # geographic target coordinates before handing the grid to the binding.
    target_x0, target_y0 = to_mercator.transform(10.0, 50.0)
    target_x1, target_y1 = to_mercator.transform(11.0, 51.0)
    target = {
        "shape": [2, 2],
        "crs": "EPSG:3857",
        "x": [target_x0, target_x1],
        "y": [target_y0, target_y1],
        "affine": None,
    }
    engine = core.Engine(json.dumps({"runtime": {"allow_network": False}}))

    result = await engine.regrid(field, json.dumps(target), "nearest")

    values = np.frombuffer(result.values_le_bytes(), dtype="<f4").reshape((2, 2))
    np.testing.assert_allclose(values, [[1.0, 2.0], [3.0, 4.0]])


def test_acquire_keeps_native_temporary_files_until_rawframe_closes(tmp_path: Path) -> None:
    ref, config = _tw_cached(tmp_path)
    expected = read_tw_grid_raw(ref)

    with Client(config=config) as client:
        with client.acquire(ref) as raw:
            path = Path(raw.artifact_path(0))
            assert path.is_file()
            assert raw.artifact_count == 1
            assert raw.artifact_bytes(0) == expected
        assert not path.exists()


@pytest.mark.asyncio
async def test_async_acquire_keeps_native_temporary_files_until_rawframe_closes(
    tmp_path: Path,
) -> None:
    ref, config = _tw_cached(tmp_path)
    expected = read_tw_grid_raw(ref)

    async with AsyncClient(config=config) as client:
        async with client.acquire(ref) as raw:
            path = Path(raw.artifact_path(0))
            assert path.is_file()
            assert raw.artifact_count == 1
            assert raw.artifact_bytes(0) == expected
        assert not path.exists()


def test_write_applies_variable_selection_before_encoding(tmp_path: Path) -> None:
    first = _field_document(1.0, name="a")
    second = _field_document(2.0, name="b")
    dataset = core.RadarDataset(
        json.dumps(
            {
                "fields": [first, second],
                "valid_time": first["valid_time"],
                "source": "review-regression",
            }
        )
    )

    with Client() as client:
        report = client.write(dataset, output=tmp_path, ref=_ref(), variable="a")

    output = Path(report.items[0].output_uri)
    with xr.open_dataset(output) as stored:
        assert "a" in stored.data_vars
        assert "b" not in stored.data_vars


def test_write_overwrite_replaces_same_identity_output(tmp_path: Path) -> None:
    ref = _ref()
    with Client() as client:
        first = client.write(_field(1.0), output=tmp_path, ref=ref)
        second = client.write(_field(9.0), output=tmp_path, ref=ref, overwrite=True)

    assert first.items[0].status == "written"
    assert second.items[0].status == "written"
    with xr.open_dataset(Path(second.items[0].output_uri)) as stored:
        np.testing.assert_allclose(stored["reflectivity"].values, 9.0)


@pytest.mark.asyncio
async def test_stream_facade_does_not_retain_returned_native_payload(tmp_path: Path) -> None:
    ref, config = _tw_cached(tmp_path)
    async with AsyncClient(config=config) as client:
        stream = client.aiter_fetch([ref], max_prefetch=1)
        try:
            result = await anext(stream)
            assert isinstance(result.data, core.RadarField)
            assert result.data.shape == [881, 921]
            assert not any(value is result.data for value in vars(stream).values())
        finally:
            await stream.aclose()
        assert stream._native is None
        assert result.data.shape == [881, 921]


@pytest.mark.asyncio
async def test_cancelled_async_download_never_commits_late_output(tmp_path: Path) -> None:
    ref, config = _tw_cached(tmp_path)
    output = tmp_path / "cancelled-download"
    progress_events: list[tuple[str, int, int | None]] = []
    cancellation_requested = False

    async with AsyncClient(config=config) as client:
        def cancel_after_rust_starts(stage: str, completed: int, total: int | None) -> None:
            nonlocal cancellation_requested
            progress_events.append((stage, completed, total))
            if stage == "download" and completed == 0 and not cancellation_requested:
                cancellation_requested = True
                client.cancel()

        report = await client.download(
            [ref], output=output, format="netcdf", progress=cancel_after_rust_starts
        )

    assert cancellation_requested
    assert ("download", 0, 1) in progress_events
    assert report.counts["cancelled"] + report.counts["not_started"] == 1
    assert not list(output.rglob("*.manifest.json"))


@pytest.mark.asyncio
async def test_cancelled_async_write_never_commits_late_output(tmp_path: Path) -> None:
    side = 1024
    pixel_count = side * side
    field = core.RadarField(
        json.dumps(
            {
                **_field_document(),
                "values": [float(index % 251) for index in range(pixel_count)],
                "shape": [side, side],
                "quality": [0] * pixel_count,
                "grid": {
                    "shape": [side, side],
                    "crs": "EPSG:4326",
                    "x": [100.0 + index / (side - 1) for index in range(side)],
                    "y": [20.0 + index / (side - 1) for index in range(side)],
                    "affine": None,
                },
            }
        )
    )
    output = tmp_path / "cancelled-write"

    async with AsyncClient(config={"runtime": {"allow_network": False}}) as client:
        events = client._session.subscribe_events()
        task = asyncio.create_task(client.write(field, output=output, ref=_ref()))
        await _wait_for_engine_operation(events, "download_netcdf")
        client.cancel()
        report = await task

    assert report.counts["cancelled"] == 1
    assert not list(output.rglob("*.manifest.json"))
