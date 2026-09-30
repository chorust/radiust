from __future__ import annotations

import asyncio
import json
from datetime import datetime, timedelta, timezone

import pytest
from radiust import AsyncClient, Client, FrameRef, Query, rust_client
from radiust.errors import (
    AmbiguousFrameError,
    AuthenticationError,
    StaleFrameError,
    TransportError,
    UnsupportedQueryError,
)


@pytest.mark.parametrize("method", ["discover", "fetch", "download", "fetch_many"])
def test_offline_discovery_propagates_network_failure(method):
    with (
        Client(config={"runtime": {"allow_network": False}}) as client,
        pytest.raises(TransportError, match="network access is disabled") as caught,
    ):
        getattr(client, method)(Query("sg", latest=True))
    assert caught.value.stage == "discover"
    assert caught.value.source == "sg"
    assert not caught.value.retryable


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "status,error_type",
    [
        ("upstream_failed", TransportError),
        ("timeout", TransportError),
        ("missing_credentials", AuthenticationError),
        ("stale", StaleFrameError),
        ("ambiguous", AmbiguousFrameError),
        ("retired", UnsupportedQueryError),
        ("cancelled", asyncio.CancelledError),
        ("not_started", asyncio.CancelledError),
    ],
)
async def test_discovery_status_is_not_silently_discarded(status, error_type):
    class Report:
        def to_json(self):
            return json.dumps({"items": [{
                "status": status, "target": {"source": "sg"}, "frame": None,
                "error": {"message": "discovery failure", "stage": "discover", "retryable": True},
            }]})

    class Session:
        async def discover_report(self, query):
            return Report()

    with pytest.raises(error_type, match="discovery failure"):
        await rust_client._discover_frames(Session(), Query("sg", latest=True))


@pytest.mark.asyncio
async def test_range_frames_reach_sdk_download_and_fetch_reports(monkeypatch, tmp_path):
    start = datetime(2026, 9, 24, tzinfo=timezone.utc)
    frames = rust_client._bridge._native_frames([
        FrameRef("rainviewer", "composite", start + timedelta(minutes=i * 5))
        for i in range(2)
    ])

    query = Query("rainviewer", start=start, end=start + timedelta(minutes=10))
    core = rust_client._bridge._core
    frames = [core.FrameRef(json.dumps({
        **json.loads(frame.to_json()), "logical_id": core.identity_logical_id(frame.to_json()),
    })) for frame in frames]
    native_report = core.DiscoveryReport(json.dumps({
        "schema_version": 1,
        "query": rust_client._bridge._native_query_payload(query),
        "interrupted": False,
        "counts": {name: (2 if name in {"total", "success"} else 0) for name in (
            "total", "success", "no_data", "stale", "missing_credentials", "retired",
            "network_restricted", "upstream_failed", "ambiguous", "timeout", "cancelled", "not_started",
        )},
        "items": [{
            "status": "success", "frame": json.loads(frame.to_json()),
            "target": {"source": frame.source, "product": frame.product, "station": None},
            "valid_time": json.loads(frame.to_json())["valid_time"], "error": None,
        } for frame in frames],
    }))

    class Session:
        def __init__(self, config):
            self.engine = core.Engine(json.dumps({"runtime": {"allow_network": False}}))

        async def discover_report(self, query):
            return native_report

        async def download_netcdf(self, refs, **kwargs):
            return await self.engine.download_netcdf(refs, "collect", True, False)

        download_png = download_geotiff = download_zarr = download_netcdf

    monkeypatch.setattr(rust_client._bridge, "CoreEngineSession", Session)
    async with AsyncClient() as client:
        assert [rust_client._frame_key(frame) for frame in await client.discover(query)] == [
            rust_client._frame_key(frame) for frame in frames
        ]
        report = await client.download(query, output=tmp_path)
        assert len(report.items) == 2
        assert [rust_client._frame_key(item.ref) for item in report.items] == [
            rust_client._frame_key(frame) for frame in frames
        ]
        with pytest.raises(AmbiguousFrameError, match="returned 2 frames"):
            await client.fetch(query)


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["discover", "fetch", "fetch_many", "download", "decode", "acquire"])
async def test_async_close_cancels_and_drains_ordinary_operations(monkeypatch, method):
    started = asyncio.Event()
    cleaning = asyncio.Event()
    release_cleanup = asyncio.Event()
    cancelled = []

    class Session:
        def __init__(self, config):
            pass

        def cancel(self):
            cancelled.append(True)

        async def block(self, *args, **kwargs):
            started.set()
            try:
                await asyncio.Future()
            finally:
                cleaning.set()
                await release_cleanup.wait()

        discover_report = fetch_raw = fetch_many_decoded = decode_science = block
        download_png = download_netcdf = download_geotiff = download_zarr = block

    monkeypatch.setattr(rust_client._bridge, "CoreEngineSession", Session)
    client = AsyncClient()
    frame = FrameRef("rainviewer", "composite", datetime(2026, 9, 24, tzinfo=timezone.utc))

    async def run():
        if method == "acquire":
            async with client.acquire(frame):
                pass
        else:
            arg = Query("sg", latest=True) if method == "discover" else frame
            await getattr(client, method)(arg)

    task = asyncio.create_task(run())
    await asyncio.wait_for(started.wait(), 2)
    close = asyncio.create_task(client.aclose())
    await asyncio.wait_for(cleaning.wait(), 2)
    assert cancelled == [True]
    assert not close.done()
    with pytest.raises(RuntimeError, match="closed"):
        await client.discover(Query("sg", latest=True))
    second_close = asyncio.create_task(client.aclose())
    release_cleanup.set()
    await asyncio.wait_for(asyncio.gather(close, second_close), 2)
    assert task.cancelled()
    assert not client._operations


@pytest.mark.asyncio
async def test_async_close_also_cancels_operations_waiting_for_event_gate(monkeypatch):
    started = asyncio.Event()
    calls = []

    class Events:
        def drain_json(self):
            return "[]"

    class Session:
        def __init__(self, config):
            pass

        def cancel(self):
            pass

        def subscribe_events(self):
            return Events()

        async def discover_report(self, query):
            calls.append(True)
            started.set()
            await asyncio.Future()

    monkeypatch.setattr(rust_client._bridge, "CoreEngineSession", Session)
    client = AsyncClient()
    query = Query("sg", latest=True)
    active = asyncio.create_task(client.discover(query, progress=lambda *args: None))
    await asyncio.wait_for(started.wait(), 2)
    queued = asyncio.create_task(client.discover(query))
    await asyncio.sleep(0)
    assert len(client._operations) == 2
    await asyncio.wait_for(client.aclose(), 2)
    assert active.cancelled() and queued.cancelled()
    assert calls == [True]
    assert not client._operations


@pytest.mark.asyncio
async def test_async_close_drains_sessions_replaced_by_download_config(monkeypatch):
    sessions = []
    both_started = asyncio.Event()

    class Session:
        def __init__(self, config):
            self.cancelled = False
            sessions.append(self)

        def cancel(self):
            self.cancelled = True

        async def block(self, *args, **kwargs):
            if len(client._operations) == 2:
                both_started.set()
            await asyncio.Future()

        fetch_raw = download_png = download_netcdf = download_geotiff = download_zarr = block

    monkeypatch.setattr(rust_client._bridge, "CoreEngineSession", Session)
    client = AsyncClient()
    frame = FrameRef("rainviewer", "composite", datetime(2026, 9, 24, tzinfo=timezone.utc))
    first = asyncio.create_task(client.fetch(frame))
    await asyncio.sleep(0)
    second = asyncio.create_task(client.download(frame, config={"cache": {"enabled": False}}))
    await asyncio.wait_for(both_started.wait(), 2)
    await asyncio.wait_for(client.aclose(), 2)
    assert len(sessions) == 2
    assert all(session.cancelled for session in sessions)
    assert first.cancelled() and second.cancelled()
