from __future__ import annotations

import json
from datetime import datetime, timezone

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, TransportError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.kr import KrSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_kr_discovers_kst_frame_and_preserves_live_raw(tmp_path, fixture_root, monkeypatch):
    monkeypatch.setattr(KrSource, "STATIONS", ("KWK",))
    discovery = json.dumps([{"result": 1, "recDate": "202609181250"}]).encode()
    transport = ReplayTransport(posts={KrSource.DISCOVERY_URL: discovery})
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "kr", transport=transport, temp_root=tmp_path)
    source = KrSource(registry.get_info("kr"))
    ref = (await source.discover(Query("kr", latest=True), context))[0]
    assert ref.station == "KWK"
    assert ref.valid_time == datetime(2026, 9, 18, 3, 50, tzinfo=timezone.utc)
    transport.responses[ref.uri] = (fixture_root / "kr/raw/KWK.png").read_bytes()
    raw = await source.download(ref, context)
    try:
        assert raw.artifacts[0].sha256 == "7a8c77b24859eefdaf6208c4a288850980c0260f7d3b5caf2c50578bc09b59bc"
        with pytest.raises(DecodeError, match="verified scientific decoder"):
            source.decode(raw, context)
    finally:
        raw.close()
        context.close()


@pytest.mark.asyncio
async def test_legacy_post_transport_error_does_not_echo_sensitive_header(tmp_path):
    secret = "fixture-post-authorization-secret"

    class EchoingPostTransport:
        async def post(self, url, body, *, headers):
            raise RuntimeError(f"request failed with credential {headers['x-api-key']}")

    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "kr",
        transport=EchoingPostTransport(),
        temp_root=tmp_path,
    )
    source = KrSource(registry.get_info("kr"))
    try:
        with pytest.raises(TransportError) as raised:
            await source._request(
                context,
                KrSource.DISCOVERY_URL,
                method="POST",
                body={"query": "latest"},
                headers={"x-api-key": secret},
            )
        assert secret not in str(raised.value)
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("payload", [b"\xff", b"{"])
async def test_kr_rejects_nonempty_invalid_discovery_json(tmp_path, monkeypatch, payload):
    monkeypatch.setattr(KrSource, "STATIONS", ("KWK",))
    transport = ReplayTransport(posts={KrSource.DISCOVERY_URL: payload})
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "kr",
        transport=transport,
        temp_root=tmp_path,
    )
    source = KrSource(registry.get_info("kr"))
    try:
        with pytest.raises(DecodeError, match="KMA returned invalid station discovery JSON"):
            await source.discover(Query("kr", latest=True), context)
    finally:
        context.close()
