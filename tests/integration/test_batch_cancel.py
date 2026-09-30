from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest
from radiust import AsyncClient, BatchError, Client, FrameRef
from radiust.config import load_config

from tests.support.native_science_fixture import (
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


# Exercise the public stream wrappers against the native Rust Engine and its
# offline cache fixtures.
def _offline_config():
    return load_config(
        {"runtime": {"allow_network": False}, "cache": {"enabled": False}},
        environ={},
    )


def _refs(count: int) -> list[FrameRef]:
    base = datetime(2025, 12, 29, 6, 50, tzinfo=timezone.utc)
    return [
        FrameRef(
            "my",
            "composite",
            base + timedelta(minutes=index),
            station="east",
            locator={
                "url": "https://www.met.gov.my/data/radar_east.gif",
                "artifacts": [],
                "station": "east",
                "name": "my_east.png",
                "media_type": "image/png",
            },
            locator_version="my-legacy-v1",
            revision=f"offline-batch-{index}",
        )
        for index in range(count)
    ]


def test_sync_stream_closes_native_prefetch(tmp_path):
    refs = [tw_grid_ref(), tw_grid_ref(tw_grid_ref().valid_time + timedelta(minutes=5))]
    cache_root = tmp_path / "cache"
    for ref in refs:
        seed_tw_grid_cache(cache_root, ref)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")

    with Client(config=config) as client:
        stream = client.iter_fetch(refs, max_prefetch=2)
        first = next(stream)
        stream.close()
        assert not client._streams

    assert first.status == "success"


@pytest.mark.asyncio
async def test_async_stream_close_releases_native_prefetch(tmp_path):
    first_ref = tw_grid_ref()
    refs = [first_ref, tw_grid_ref(first_ref.valid_time + timedelta(minutes=5))]
    cache_root = tmp_path / "cache"
    for ref in refs:
        seed_tw_grid_cache(cache_root, ref)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")

    async with AsyncClient(config=config) as client:
        stream = client.aiter_fetch(refs, max_prefetch=2)
        first = await anext(stream)
        await stream.aclose()
        assert not client._streams

    assert first.status == "success"


@pytest.mark.asyncio
async def test_stream_raise_reports_started_and_not_started_frames():
    refs = _refs(4)

    async with AsyncClient(config=_offline_config()) as client:
        stream = client.aiter_fetch(refs, max_prefetch=2, on_error="raise")
        with pytest.raises(BatchError) as caught:
            await anext(stream)
        assert not client._streams

    partial = caught.value.partial_result
    statuses = [item.status for item in partial.items]
    assert statuses.count("failed") == 1
    assert statuses.count("cancelled") == 1
    assert statuses.count("not_started") == 2
    assert [item.ref.logical_id for item in partial.items] == [ref.logical_id for ref in refs]
