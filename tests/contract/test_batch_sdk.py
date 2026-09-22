from __future__ import annotations

import asyncio
from datetime import datetime, timedelta, timezone

import pytest
from radiust import AsyncClient, BatchError, Client, FrameRef, ProductInfo, SourceInfo
from radiust.models import Artifact
from radiust.raw import RawFrame


class _BatchSource:
    info = SourceInfo(
        id="batch-test",
        description="test source",
        adapter_version="1",
        products=(ProductInfo(id="reflectivity", variables=("reflectivity",), default=True),),
    )

    def __init__(self) -> None:
        self.active = 0
        self.max_active = 0
        self.downloaded = 0
        self.gates: dict[str, tuple[asyncio.Event, asyncio.Event]] = {}

    async def discover(self, query, context):
        return []

    async def download(self, ref, context):
        self.downloaded += 1
        self.active += 1
        self.max_active = max(self.max_active, self.active)
        try:
            gate = self.gates.get(ref.station or "")
            if gate is None:
                await asyncio.sleep(float(ref.metadata.get("delay", 0)))
            else:
                started, release = gate
                started.set()
                await release.wait()
            if ref.metadata.get("error"):
                raise RuntimeError(str(ref.metadata["error"]))
            return RawFrame(ref, (Artifact("frame.bin", "data", "application/octet-stream", b"x"),))
        finally:
            self.active -= 1

    def decode(self, raw, context):
        raw._ensure_open()
        return {"station": raw.ref.station}


def _refs(*specs: tuple[str, float, str | None]) -> list[FrameRef]:
    base = datetime(2025, 12, 29, 6, 50, tzinfo=timezone.utc)
    return [
        FrameRef(
            "batch-test",
            "reflectivity",
            base + timedelta(minutes=index),
            station=station,
            metadata={"delay": delay, **({"error": error} if error else {})},
        )
        for index, (station, delay, error) in enumerate(specs)
    ]


def test_fetch_many_preserves_input_order_and_data(monkeypatch):
    source = _BatchSource()
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    refs = _refs(("a", 0.03, None), ("b", 0.0, None), ("c", 0.01, None))

    with Client() as client:
        result = client.fetch_many(refs)

    assert [item.ref.station for item in result.items] == ["a", "b", "c"]
    assert [item.status for item in result.items] == ["success", "success", "success"]
    assert [item.data["station"] for item in result.items] == ["a", "b", "c"]


def test_fetch_many_rejects_duplicate_identity_before_acquisition(monkeypatch):
    source = _BatchSource()
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    ref = _refs(("a", 0.0, None))[0]

    with Client() as client, pytest.raises(ValueError, match="duplicate"):
        client.fetch_many([ref, ref])

    assert source.downloaded == 0


def test_fetch_many_collects_failures_without_reordering(monkeypatch):
    source = _BatchSource()
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    refs = _refs(("a", 0.01, None), ("b", 0.0, "bad frame"), ("c", 0.0, None))

    with Client() as client:
        result = client.fetch_many(refs, on_error="collect")

    assert [item.status for item in result.items] == ["success", "failed", "success"]
    assert result.items[1].error is not None
    assert "bad frame" in result.items[1].error["message"]


def test_fetch_many_raise_contains_successful_partial_results(monkeypatch):
    source = _BatchSource()
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    refs = _refs(("a", 0.0, None), ("b", 0.02, "bad frame"), ("c", 1.0, None))

    with Client() as client, pytest.raises(BatchError) as caught:
        client.fetch_many(refs, on_error="raise")

    partial = caught.value.partial_result
    assert [item.ref.station for item in partial.items[:2]] == ["a", "b"]
    assert any(item.status == "success" for item in partial.items)
    assert any(item.status == "failed" for item in partial.items)
    assert source.active == 0


@pytest.mark.asyncio
async def test_async_fetch_many_and_stream_are_available(monkeypatch):
    source = _BatchSource()
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    refs = _refs(("a", 0.03, None), ("b", 0.0, None), ("c", 0.01, None))

    async with AsyncClient() as client:
        result = await client.fetch_many(refs)
        started = {station: asyncio.Event() for station in ("a", "b", "c")}
        releases = {station: asyncio.Event() for station in ("a", "b", "c")}
        source.gates = {
            station: (started[station], releases[station]) for station in started
        }
        stream = client.aiter_fetch(refs, max_prefetch=2)
        first_pending = asyncio.create_task(stream.__anext__())
        try:
            await asyncio.gather(started["a"].wait(), started["b"].wait())
            releases["b"].set()
            first = await asyncio.wait_for(first_pending, timeout=1)
            await started["c"].wait()
            releases["c"].set()
            second = await asyncio.wait_for(stream.__anext__(), timeout=1)
            releases["a"].set()
            third = await asyncio.wait_for(stream.__anext__(), timeout=1)
        finally:
            for release in releases.values():
                release.set()
            await stream.aclose()

    assert [item.status for item in result.items] == ["success"] * 3
    assert [first.ref.station, second.ref.station, third.ref.station] == ["b", "c", "a"]
