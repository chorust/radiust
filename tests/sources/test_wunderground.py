from __future__ import annotations

import io
import json
from datetime import datetime, timezone
from urllib.parse import parse_qs, urlparse

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import ConfigError, TransportError
from radiust.identity import safe_ref
from radiust.logging import redact
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.wunderground import WundergroundSource


class RecordingTransport:
    def __init__(self, payload: bytes):
        self.payload = payload
        self.urls: list[str] = []

    async def get(self, url: str, *, headers=None) -> bytes:
        self.urls.append(url)
        return self.payload


def _png() -> bytes:
    output = io.BytesIO()
    Image.new("RGBA", (256, 256), (0, 0, 0, 0)).save(output, format="PNG")
    return output.getvalue()


@pytest.mark.asyncio
async def test_wunderground_keeps_api_key_out_of_frame_and_adds_it_only_to_requests(tmp_path, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(WundergroundSource, "frame_time", lambda self: valid_time)
    transport = RecordingTransport(_png())
    assert "test-key" not in redact("https://weather.example/tile?apiKey=test-key")
    context = SourceContext(
        load_config(
            {"runtime": {"allow_network": True}, "sources": {"wunderground": {"api_key": "test-key"}}},
            environ={},
        ),
        "wunderground",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        source = WundergroundSource(registry.get_info("wunderground"))
        ref = (await source.discover(Query("wunderground", latest=True), context))[0]
        urls = [ref.uri, *(item["url"] for item in ref.locator["artifacts"])]
        assert len(urls) == 4
        assert all(parse_qs(urlparse(url).query).get("product") == ["wuRadarMosaic"] for url in urls)
        assert all("apiKey" not in parse_qs(urlparse(url).query) for url in urls)
        assert "test-key" not in repr(ref)
        assert "test-key" not in json.dumps(safe_ref(ref))

        other_context = SourceContext(
            load_config(
                {"runtime": {"allow_network": True}, "sources": {"wunderground": {"api_key": "another-test-key"}}},
                environ={},
            ),
            "wunderground",
            temp_root=tmp_path / "other",
        )
        try:
            other_ref = (await source.discover(Query("wunderground", latest=True), other_context))[0]
            assert other_ref.logical_id == ref.logical_id
        finally:
            other_context.close()

        raw = await source.download(ref, context)
        try:
            assert len(transport.urls) == 4
            for url in transport.urls:
                query = parse_qs(urlparse(url).query)
                assert query["product"] == ["wuRadarMosaic"]
                assert query["apiKey"] == ["test-key"]
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_wunderground_requires_key_before_sending_tile_request(tmp_path, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(WundergroundSource, "frame_time", lambda self: valid_time)
    transport = RecordingTransport(_png())
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "wunderground",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        source = WundergroundSource(registry.get_info("wunderground"))
        with pytest.raises(ConfigError, match="sources.wunderground.api_key"):
            await source.discover(Query("wunderground", latest=True), context)
        assert transport.urls == []
    finally:
        context.close()


@pytest.mark.asyncio
async def test_wunderground_download_error_does_not_echo_api_key(tmp_path, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(WundergroundSource, "frame_time", lambda self: valid_time)
    api_key = "fixture-only-secret-key"

    class EchoErrorTransport:
        async def get(self, url: str, *, headers=None) -> bytes:
            raise RuntimeError(f"request failed for {url}")

    context = SourceContext(
        load_config(
            {"runtime": {"allow_network": True}, "sources": {"wunderground": {"api_key": api_key}}},
            environ={},
        ),
        "wunderground",
        transport=EchoErrorTransport(),
        temp_root=tmp_path,
    )
    source = WundergroundSource(registry.get_info("wunderground"))
    try:
        ref = (await source.discover(Query("wunderground", latest=True), context))[0]
        with pytest.raises(Exception) as raised:
            await source.download(ref, context)
        assert api_key not in str(raised.value)
        assert isinstance(raised.value, TransportError)
    finally:
        context.close()
