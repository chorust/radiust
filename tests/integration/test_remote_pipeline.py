from __future__ import annotations

import json
from datetime import datetime, timezone

import pytest
from radiust import AsyncClient, Client, Query
from radiust.storage.commit import InMemoryRemoteBackend

QUERY = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))


def test_remote_pipeline_publishes_pointer_and_skips_verified_output() -> None:
    backend = InMemoryRemoteBackend()

    with Client(_remote_backend=backend) as client:
        first = client.download(QUERY, output="s3://bucket/radar")
        second = client.download(QUERY, output="s3://bucket/radar")

    assert first.counts["written"] == 1
    assert second.counts["skipped"] == 1
    assert first.items[0].output_uri == second.items[0].output_uri
    pointer_key = "radar/source=my/product=composite/date=2025-12-29/hour=06/peninsular_20251229T065001Z_8f89d380e6dd.nc.manifest.json"
    manifest = json.loads(backend.objects[pointer_key])
    assert manifest["generation"]
    assert manifest["artifacts"][0]["relative_uri"].startswith("_generations/")


def test_remote_pipeline_raw_request_supplements_decoded_output() -> None:
    backend = InMemoryRemoteBackend()

    with Client(_remote_backend=backend) as client:
        decoded = client.download(QUERY, output="s3://bucket/radar")
        raw = client.download(QUERY, output="s3://bucket/radar", raw=True)

    assert decoded.counts["written"] == 1
    assert raw.counts["written"] == 1
    assert raw.items[0].output_uri == decoded.items[0].output_uri
    pointer_key = "radar/source=my/product=composite/date=2025-12-29/hour=06/peninsular_20251229T065001Z_8f89d380e6dd.nc.manifest.json"
    manifest = json.loads(backend.objects[pointer_key])
    assert manifest["raw_complete"] is True
    assert any(item["relative_uri"].startswith("_generations/") for item in manifest["artifacts"])


@pytest.mark.asyncio
async def test_async_write_uses_the_same_remote_commit_fence() -> None:
    backend = InMemoryRemoteBackend()

    async with AsyncClient(_remote_backend=backend) as client:
        ref = (await client.discover(QUERY))[0]
        field = await client.fetch(QUERY)
        report = await client.write(field, ref=ref, output="s3://bucket/async")

    assert report.counts["written"] == 1
    assert report.items[0].output_uri.startswith("s3://bucket/async/")
