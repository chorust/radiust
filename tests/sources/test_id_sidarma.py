from __future__ import annotations

import asyncio
import io
import json
from datetime import datetime, timezone

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, TransportError, UnsupportedQueryError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.id_sidarma import IdSidarmaSource

API_URL = "https://api.bmkg.go.id/sidarma/sidarma-nowcast/android/ssxCGK.json"
RAW_OLDER = "https://example.test/CGK_20260918_0200.png"
RAW_LATEST = "https://example.test/CGK_20260918_0210.png"


class ReplayTransport:
    def __init__(self, responses: dict[str, bytes]):
        self.responses = responses
        self.requests: list[tuple[str, dict[str, str]]] = []

    async def get(self, url: str, *, headers=None) -> bytes:
        request_headers = dict(headers or {})
        self.requests.append((url, request_headers))
        return self.responses[url]


def _metadata() -> bytes:
    return json.dumps(
        {
            "CMAX": {
                "LastOneHour": {
                    "file": [RAW_OLDER, RAW_LATEST],
                    "timeUTC": ["2026-09-18 02:00 UTC", "2026-09-18 02:10 UTC"],
                },
                "Latest": {
                    "file": RAW_LATEST,
                    "timeUTC": "2026-09-18 02:10 UTC",
                },
            }
        }
    ).encode()


def _png() -> bytes:
    image = Image.new("RGBA", (4, 3), (10, 20, 30, 255))
    output = io.BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


def _context(tmp_path, transport: ReplayTransport, *, api_key: str | None = "fixture-key") -> SourceContext:
    source_config: dict[str, object] = {"radar_ids": ["CGK"]}
    if api_key is not None:
        source_config["api_key"] = api_key
    return SourceContext(
        load_config(
            {
                "runtime": {"allow_network": True},
                "sources": {"id_sidarma": source_config},
            },
            environ={},
        ),
        "id_sidarma",
        transport=transport,
        temp_root=tmp_path,
    )


@pytest.mark.asyncio
async def test_sidarma_discovers_both_buckets_dedupes_latest_and_uses_station_geometry(tmp_path):
    transport = ReplayTransport({API_URL: _metadata()})
    source = IdSidarmaSource(registry.get_info("id_sidarma"))
    context = _context(tmp_path, transport)
    try:
        entries = await source.discover_entries(context)
        assert len(entries) == 2
        assert {entry["metadata"]["bucket"] for entry in entries} == {"LastOneHour", "Latest"}
        latest_entry = max(entries, key=lambda entry: entry["valid_time"])
        assert latest_entry["valid_time"] == datetime(2026, 9, 18, 2, 10, tzinfo=timezone.utc)
        assert latest_entry["bbox"] == pytest.approx((104.43221471981228, -8.37158528018772, 108.92744528018771, -3.876354719812279))

        refs = await source.discover(Query("id_sidarma", latest=True, stations=("CGK",)), context)
        assert len(refs) == 1
        assert refs[0].station == "CGK"
        assert refs[0].uri == RAW_LATEST
        assert tuple(refs[0].locator["bbox"]) == pytest.approx(latest_entry["bbox"])
        assert all(headers.get("x-api-key") == "fixture-key" for _, headers in transport.requests)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_sidarma_uses_configured_api_key_without_embedding_one(tmp_path):
    transport = ReplayTransport({API_URL: _metadata()})
    source = IdSidarmaSource(registry.get_info("id_sidarma"))
    context = _context(tmp_path, transport, api_key="fixture-key")
    try:
        await source.discover(Query("id_sidarma", latest=True), context)
        metadata_request = next(headers for url, headers in transport.requests if url == API_URL)
        assert metadata_request["x-api-key"] == "fixture-key"
    finally:
        context.close()


@pytest.mark.asyncio
async def test_sidarma_transport_error_does_not_echo_configured_api_key(tmp_path):
    api_key = "fixture-sidarma-discovery-secret"

    class EchoErrorTransport:
        async def get(self, url: str, *, headers=None) -> bytes:
            raise RuntimeError(f"request failed for {url} with headers {headers!r}")

    context = _context(tmp_path, EchoErrorTransport(), api_key=api_key)
    source = IdSidarmaSource(registry.get_info("id_sidarma"))
    try:
        with pytest.raises(Exception) as raised:
            await source.discover(Query("id_sidarma", latest=True, stations=("CGK",)), context)
        assert api_key not in str(raised.value)
        assert isinstance(raised.value, TransportError)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_sidarma_preserves_raw_but_rejects_unverified_scientific_decode(tmp_path):
    transport = ReplayTransport({API_URL: _metadata(), RAW_LATEST: _png()})
    source = IdSidarmaSource(registry.get_info("id_sidarma"))
    context = _context(tmp_path, transport)
    try:
        ref = (await source.discover(Query("id_sidarma", latest=True), context))[0]
        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].sha256
            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_sidarma_rejects_historical_queries(tmp_path):
    transport = ReplayTransport({API_URL: _metadata()})
    source = IdSidarmaSource(registry.get_info("id_sidarma"))
    context = _context(tmp_path, transport)
    try:
        with pytest.raises(UnsupportedQueryError):
            await source.discover(
                Query("id_sidarma", at=datetime(2026, 9, 18, 2, 10, tzinfo=timezone.utc)),
                context,
            )
    finally:
        context.close()


@pytest.mark.asyncio
async def test_sidarma_surfaces_total_provider_failure_and_preserves_partial_success(tmp_path):
    source = IdSidarmaSource(registry.get_info("id_sidarma"))
    transport = ReplayTransport({})
    context = _context(tmp_path, transport)

    async def failing_get(url, *, headers=None):
        raise TransportError("SIDARMA upstream unavailable")

    transport.get = failing_get
    try:
        with pytest.raises(TransportError, match="upstream unavailable"):
            await source.discover(Query("id_sidarma", latest=True), context)

        context.config.values["sources"]["id_sidarma"]["radar_ids"] = ["CGK", "BTH"]

        async def partial_get(url, *, headers=None):
            if "ssxBTH" in url:
                raise TransportError("one radar unavailable")
            return _metadata()

        transport.get = partial_get
        refs = await source.discover(Query("id_sidarma", latest=True), context)
        assert [ref.station for ref in refs] == ["CGK"]
        assert context.metadata["discovery_failed_stations"] == 1
    finally:
        context.close()


@pytest.mark.asyncio
async def test_sidarma_propagates_cancelled_radar_request(tmp_path):
    source = IdSidarmaSource(registry.get_info("id_sidarma"))
    transport = ReplayTransport({})
    context = _context(tmp_path, transport)

    async def cancelled_get(url, *, headers=None):
        raise asyncio.CancelledError

    transport.get = cancelled_get
    try:
        with pytest.raises(asyncio.CancelledError):
            await source.discover(Query("id_sidarma", latest=True), context)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_sidarma_wraps_non_utf8_metadata_as_decode_error(tmp_path):
    source = IdSidarmaSource(registry.get_info("id_sidarma"))
    context = _context(tmp_path, ReplayTransport({API_URL: b"\xff"}))
    try:
        with pytest.raises(DecodeError, match="SIDARMA returned invalid JSON for CGK"):
            await source.discover_entries(context)
    finally:
        context.close()
