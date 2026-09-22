from __future__ import annotations

import asyncio
from datetime import datetime, timedelta, timezone

import pytest
from radiust import AsyncClient, BatchError, Client, FrameRef, ProductInfo, SourceInfo
from radiust.models import Artifact
from radiust.raw import RawFrame


class _SlowSource:
    info = SourceInfo(
        id="slow-batch-test",
        description="slow test source",
        adapter_version="1",
        products=(ProductInfo(id="reflectivity", variables=("reflectivity",), default=True),),
    )

    def __init__(self) -> None:
        self.active = 0
        self.max_active = 0

    async def discover(self, query, context):
        return []

    async def download(self, ref, context):
        self.active += 1
        self.max_active = max(self.max_active, self.active)
        try:
            await asyncio.sleep(float(ref.metadata.get("delay", 0.1)))
            if ref.metadata.get("error"):
                raise RuntimeError(str(ref.metadata["error"]))
            return RawFrame(ref, (Artifact("frame.bin", "data", "application/octet-stream", b"x"),))
        finally:
            self.active -= 1

    def decode(self, raw, context):
        return raw.ref.station


def _refs(count: int) -> list[FrameRef]:
    base = datetime(2025, 12, 29, 6, 50, tzinfo=timezone.utc)
    return [
        FrameRef(
            "slow-batch-test",
            "reflectivity",
            base + timedelta(minutes=index),
            station=f"s{index}",
            metadata={"delay": 0.2 if index == 0 else 0.01},
        )
        for index in range(count)
    ]


def test_sync_stream_has_bounded_prefetch_and_close(monkeypatch):
    source = _SlowSource()
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    refs = _refs(6)

    with Client() as client:
        stream = client.iter_fetch(refs, max_prefetch=2)
        first = next(stream)
        stream.close()

    assert first.status == "success"
    assert source.max_active <= 2
    assert source.active == 0


@pytest.mark.asyncio
async def test_async_stream_close_cancels_pending_work(monkeypatch):
    source = _SlowSource()
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    refs = _refs(6)

    async with AsyncClient() as client:
        stream = client.aiter_fetch(refs, max_prefetch=2)
        first = await anext(stream)
        await stream.aclose()

    assert first.status == "success"
    assert source.max_active <= 2
    assert source.active == 0


@pytest.mark.asyncio
async def test_stream_raise_reports_started_and_not_started_frames(monkeypatch):
    source = _SlowSource()
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    refs = _refs(4)
    refs[1] = FrameRef(
        refs[1].source,
        refs[1].product,
        refs[1].valid_time,
        station=refs[1].station,
        metadata={"delay": 0, "error": "stream failure"},
    )

    async with AsyncClient() as client:
        stream = client.aiter_fetch(refs, max_prefetch=2, on_error="raise")
        with pytest.raises(BatchError) as caught:
            await anext(stream)

    partial = caught.value.partial_result
    assert [item.status for item in partial.items] == ["cancelled", "failed", "not_started", "not_started"]
    assert source.active == 0
