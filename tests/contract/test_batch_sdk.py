from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest
from radiust import AsyncClient, BatchError, Client, FrameRef

from tests.support.native_raw_cache import seed_native_raw_cache
from tests.support.native_science_fixture import (
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


def _refs() -> list[FrameRef]:
    valid_time = datetime(2026, 9, 24, tzinfo=timezone.utc)
    return [
        FrameRef("fr", "composite", valid_time, station="FRCOMP"),
        FrameRef("my", "composite", valid_time, station="east"),
    ]


def test_fetch_many_rejects_duplicate_identity_before_native_io() -> None:
    frame = _refs()[0]
    with (
        Client(config={"runtime": {"allow_network": False}}) as client,
        pytest.raises(ValueError, match="duplicate"),
    ):
        client.fetch_many([frame, frame])

def test_fetch_many_collects_native_acquisition_failures_in_input_order() -> None:
    frames = _refs()
    with Client(config={"runtime": {"allow_network": False}}) as client:
        result = client.fetch_many(frames, on_error="collect", max_concurrency=2)

    assert [item.ref.logical_id for item in result.items] == [
        frame.logical_id for frame in frames
    ]
    assert [item.status for item in result.items] == ["failed", "failed"]
    assert all(item.data is None and item.error is not None for item in result.items)


def test_fetch_many_preserves_structured_rust_decode_errors(tmp_path) -> None:
    frame = FrameRef(
        "my",
        "composite",
        datetime(2026, 9, 24, tzinfo=timezone.utc),
        station="east",
        locator={
            "url": "https://www.met.gov.my/data/radar_east.gif",
            "artifacts": [],
            "station": "east",
            "name": "my_east.png",
            "media_type": "image/png",
        },
        locator_version="my-legacy-v1",
        revision="unsupported-science-contract",
    )
    cache_root = tmp_path / "cache"
    seed_native_raw_cache(cache_root, frame, [("my_east.png", "image/png", b"raw fixture")])
    config = {
        "runtime": {"allow_network": False},
        "cache": {"enabled": True, "dir": str(cache_root)},
    }

    with Client(config=config) as client:
        result = client.fetch_many([frame])

    item = result.items[0]
    assert item.status == "failed"
    assert item.error is not None
    assert item.error["code"] == "unsupported_query"
    assert item.error["stage"] == "decode"
    assert item.error["source"] == "my"


def test_fetch_many_stop_keeps_ordered_partial_result() -> None:
    frames = _refs()
    with (
        Client(config={"runtime": {"allow_network": False}}) as client,
        pytest.raises(BatchError) as caught,
    ):
        client.fetch_many(frames, on_error="stop", max_concurrency=1)

    partial = caught.value.partial_result
    assert [item.ref.logical_id for item in partial.items] == [
        frame.logical_id for frame in frames
    ]
    assert [item.status for item in partial.items] == ["failed", "not_started"]


@pytest.mark.asyncio
async def test_async_fetch_many_decodes_cached_frames_in_rust_and_preserves_order(
    tmp_path, monkeypatch
) -> None:
    first = tw_grid_ref()
    second = tw_grid_ref(first.valid_time + timedelta(minutes=5))
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, first)
    seed_tw_grid_cache(cache_root, second)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")

    async with AsyncClient(config=config) as client:
        async def reject_python_decode(_raw):
            raise AssertionError("batch decode must remain in the Rust Engine")

        monkeypatch.setattr(client._session, "decode_science", reject_python_decode)
        result = await client.fetch_many([second, first], max_concurrency=2)

    assert [item.ref.logical_id for item in result.items] == [
        second.logical_id,
        first.logical_id,
    ]
    assert [item.status for item in result.items] == ["success", "success"]
    assert all(item.data.name == "reflectivity" for item in result.items)
    assert all(item.data.shape == [881, 921] for item in result.items)


@pytest.mark.asyncio
async def test_async_iter_fetch_uses_native_decode_and_honors_single_prefetch(
    tmp_path, monkeypatch
) -> None:
    first = tw_grid_ref()
    second = tw_grid_ref(first.valid_time + timedelta(minutes=5))
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, first)
    seed_tw_grid_cache(cache_root, second)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")

    async with AsyncClient(config=config) as client:
        def reject_python_fetch(_ref):
            raise AssertionError("stream fetch must remain in the Rust Engine")

        async def reject_python_decode(_raw):
            raise AssertionError("stream decode must remain in the Rust Engine")

        monkeypatch.setattr(client, "acquire", reject_python_fetch)
        monkeypatch.setattr(client, "decode", reject_python_decode)
        stream = client.aiter_fetch([second, first], max_prefetch=1)
        async with stream:
            items = [item async for item in stream]
        assert not client._streams

    assert [item.ref.logical_id for item in items] == [
        second.logical_id,
        first.logical_id,
    ]
    assert [item.status for item in items] == ["success", "success"]
    assert all(item.data.name == "reflectivity" for item in items)


def test_sync_iter_fetch_uses_native_decode(tmp_path, monkeypatch) -> None:
    frame = tw_grid_ref()
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, frame)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")

    with Client(config=config) as client:
        def reject_python_decode(_raw):
            raise AssertionError("stream decode must remain in the Rust Engine")

        monkeypatch.setattr(client, "decode", reject_python_decode)
        with client.iter_fetch([frame], max_prefetch=1) as stream:
            item = next(stream)

    assert item.ref.logical_id == frame.logical_id
    assert item.status == "success"
    assert item.data.name == "reflectivity"


@pytest.mark.asyncio
async def test_async_iter_fetch_raise_returns_ordered_native_partial_result() -> None:
    frames = _refs()
    async with AsyncClient(config={"runtime": {"allow_network": False}}) as client:
        stream = client.aiter_fetch(frames, on_error="raise", max_prefetch=1)
        with pytest.raises(BatchError) as caught:
            await anext(stream)

        assert not client._streams

    partial = caught.value.partial_result
    assert [item.ref.logical_id for item in partial.items] == [
        frame.logical_id for frame in frames
    ]
    assert [item.status for item in partial.items] == ["failed", "not_started"]
