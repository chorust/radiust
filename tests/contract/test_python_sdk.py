from __future__ import annotations

import asyncio
from datetime import datetime, timezone

import pytest
from radiust import AsyncClient, AsyncContextError, Client, Query

QUERY = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))


def test_acquire_is_a_context_manager_and_fetch_does_not_write_output(tmp_path):
    with Client() as client:
        refs = client.discover(QUERY)
        with client.acquire(refs[0]) as raw:
            assert raw.closed is False
            value = client.decode(raw)
        assert raw.closed is True
        assert value.data.shape[0] > 0
        assert not list(tmp_path.iterdir())


def test_sync_client_rejects_use_inside_running_event_loop():
    async def use_sync_client() -> None:
        with Client() as client, pytest.raises(AsyncContextError):
            client.discover(QUERY)

    asyncio.run(use_sync_client())


def test_async_fetch_matches_sync_shape_and_closes_raw():
    with Client() as sync:
        expected = sync.fetch(QUERY).data.shape
    async def fetch_shape():
        async with AsyncClient() as client:
            return (await client.fetch(QUERY)).data.shape

    actual = asyncio.run(fetch_shape())
    assert actual == expected


def test_download_rejects_unknown_processing_options_before_acquisition(tmp_path):
    with Client() as client, pytest.raises(ValueError, match="unsupported processing"):
        client.download(QUERY, output=tmp_path, unsupported=True)


def test_sync_and_async_write_return_download_reports(tmp_path):
    with Client() as client:
        refs = client.discover(QUERY)
        field = client.fetch(QUERY)
        report = client.write(field, output=tmp_path, ref=refs[0])
    assert report.command == "write"
    assert report.counts["written"] == 1

    async def write_async():
        async with AsyncClient() as client:
            refs = await client.discover(QUERY)
            field = await client.fetch(QUERY)
            return await client.write(field, output=tmp_path / "async", ref=refs[0])

    async_report = asyncio.run(write_async())
    assert async_report.counts["written"] == 1
