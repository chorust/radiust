from __future__ import annotations

import json
from datetime import datetime, timezone

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.nz import NzSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_nz_uses_mobile_headers_keeps_tls_default_and_preserves_raw(tmp_path, fixture_root, monkeypatch):
    monkeypatch.setattr(NzSource, "STATIONS", {"NZAU2": "Kumeu"})
    api = f"{NzSource.BASE_URL}/publicData/mobileRainRadar_rural_Kumeu"
    image = f"{NzSource.BASE_URL}/radarImage"
    payload = json.dumps({"imageList": [{"url": "/radarImage", "dateTimeISO": "2026-09-18T15:50:00+12:00"}]}).encode()
    transport = ReplayTransport({api: payload, image: (fixture_root / "nz/raw/NZAU2.gif").read_bytes()})
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "nz", transport=transport, temp_root=tmp_path)
    source = NzSource(registry.get_info("nz"))
    ref = (await source.discover(Query("nz", latest=True), context))[0]
    assert ref.station == "NZAU2"
    assert ref.valid_time == datetime(2026, 9, 18, 3, 50, tzinfo=timezone.utc)
    assert transport.get_calls[0][1]["User-Agent"].startswith("rural/")
    raw = await source.download(ref, context)
    try:
        assert raw.artifacts[0].sha256 == "0d98e9bc3ac0cd1b25d303a1b40db3641e316fc85db75554b0b8053c9607f85e"
        with pytest.raises(DecodeError, match="verified scientific decoder"):
            source.decode(raw, context)
    finally:
        raw.close()
        context.close()
