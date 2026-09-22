from __future__ import annotations

import hashlib
import io
import json
from datetime import datetime, timezone
from urllib.parse import parse_qs, urlparse

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, IntegrityError, UnsupportedQueryError
from radiust.identity import artifact_bytes, safe_ref
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.bmkg import BmkgSource
from radiust.sources.opensnow import OpenSnowSource
from radiust.sources.windy import WindySource
from radiust.sources.wunderground import WundergroundSource


class _TileReplayTransport:
    def __init__(self, payloads: tuple[bytes, ...]) -> None:
        self.payloads = payloads
        self.calls: list[str] = []

    async def get(self, url: str, *, headers=None) -> bytes:
        self.calls.append(url)
        return self.payloads[len(self.calls) - 1]


def _png(color: tuple[int, int, int, int], *, size: int = 256) -> bytes:
    buffer = io.BytesIO()
    Image.new("RGBA", (size, size), color).save(buffer, format="PNG")
    return buffer.getvalue()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("source_id", "source_type", "api_key"),
    [
        ("opensnow", OpenSnowSource, None),
        ("bmkg", BmkgSource, None),
        ("wunderground", WundergroundSource, "fixture-only-key"),
    ],
)
async def test_legacy_tile_adapters_replay_all_raw_tiles_without_claiming_science(
    tmp_path, monkeypatch, source_id, source_type, api_key
):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(source_type, "frame_time", lambda _self: valid_time)
    payloads = tuple(_png((index * 40, 30, 90, 255)) for index in range(4))
    transport = _TileReplayTransport(payloads)
    values = {"runtime": {"allow_network": True}}
    if api_key is not None:
        values["sources"] = {"wunderground": {"api_key": api_key}}
    context = SourceContext(
        load_config(values, environ={}),
        source_id,
        transport=transport,
        temp_root=tmp_path,
    )
    source = source_type(registry.get_info(source_id))
    try:
        ref = (await source.discover(Query(source_id, latest=True), context))[0]
        specs = ({"url": ref.uri, "name": ref.locator["name"]}, *ref.locator["artifacts"])
        assert len(specs) == 4
        assert ref.valid_time == valid_time
        assert ref.station == "global"
        assert "fixture-only-key" not in repr(ref)
        assert "fixture-only-key" not in json.dumps(safe_ref(ref))

        raw = await source.download(ref, context)
        try:
            assert len(transport.calls) == 4
            assert [artifact.name for artifact in raw.artifacts] == [spec["name"] for spec in specs]
            assert [artifact.role for artifact in raw.artifacts] == ["data", "tile", "tile", "tile"]
            for artifact, payload, request_url in zip(raw.artifacts, payloads, transport.calls, strict=True):
                assert artifact_bytes(artifact) == payload
                assert artifact.sha256 == hashlib.sha256(payload).hexdigest()
                query = parse_qs(urlparse(request_url).query)
                if api_key is None:
                    assert "apiKey" not in query
                else:
                    assert query["apiKey"] == [api_key]
                    assert request_url.count(api_key) == 1

            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("source_id", "source_type", "config"),
    [
        ("windy", WindySource, {}),
        ("opensnow", OpenSnowSource, {}),
        ("bmkg", BmkgSource, {}),
        ("wunderground", WundergroundSource, {"sources": {"wunderground": {"api_key": "fixture-only-key"}}}),
    ],
)
async def test_legacy_tile_adapters_reject_historical_queries_before_discovery(
    tmp_path, source_id, source_type, config
):
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}, **config}, environ={}),
        source_id,
        transport=_TileReplayTransport(()),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(UnsupportedQueryError, match="only supports latest frames"):
            await source_type(registry.get_info(source_id)).discover(
                Query(source_id, at=datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)),
                context,
            )
        assert context.transport.calls == []
    finally:
        context.close()


@pytest.mark.asyncio
async def test_legacy_tile_adapter_rejects_non_256_square_tile(tmp_path, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(OpenSnowSource, "frame_time", lambda _self: valid_time)
    transport = _TileReplayTransport(tuple(_png((index * 40, 30, 90, 255), size=2) for index in range(4)))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "opensnow",
        transport=transport,
        temp_root=tmp_path,
    )
    source = OpenSnowSource(registry.get_info("opensnow"))
    try:
        ref = (await source.discover(Query("opensnow", latest=True), context))[0]
        with pytest.raises(IntegrityError, match="256x256"):
            await source.download(ref, context)
    finally:
        context.close()
