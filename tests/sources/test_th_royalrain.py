from __future__ import annotations

import importlib
from datetime import datetime, timezone

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.th_royalrain import ThRoyalRainSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_th_royalrain_parses_fixed_station_cappi_and_preserves_raw(tmp_path, fixture_root, monkeypatch):
    th_module = importlib.import_module("radiust.sources.th_royalrain")
    monkeypatch.setattr(th_module, "STATIONS", ("takhli",))
    page = f"{ThRoyalRainSource.BASE_URL}/?station=takhli"
    image = f"{ThRoyalRainSource.BASE_URL}/takhli/2026091803420400dBZ.cappi.png"
    transport = ReplayTransport(
        {
            page: b'<img src="/opendata/radar_data/cappi/takhli/2026091803420400dBZ.cappi.png">',
            image: (fixture_root / "th_royalrain/raw/takhli.png").read_bytes(),
        }
    )
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "th_royalrain", transport=transport, temp_root=tmp_path)
    source = ThRoyalRainSource(registry.get_info("th_royalrain"))
    ref = (await source.discover(Query("th_royalrain", latest=True), context))[0]
    assert ref.station == "takhli"
    assert ref.valid_time == datetime(2026, 9, 18, 3, 42, tzinfo=timezone.utc)
    raw = await source.download(ref, context)
    try:
        assert raw.artifacts[0].sha256 == "8c13c0d0077bf5c66904ca0d1522f75890d96180647ca41dce986667d82e36cd"
        with pytest.raises(DecodeError, match="verified scientific decoder"):
            source.decode(raw, context)
    finally:
        raw.close()
        context.close()


@pytest.mark.asyncio
async def test_th_royalrain_station_query_only_fetches_requested_page(tmp_path, monkeypatch):
    th_module = importlib.import_module("radiust.sources.th_royalrain")
    monkeypatch.setattr(th_module, "STATIONS", ("takhli", "omkoi"))
    page = f"{ThRoyalRainSource.BASE_URL}/?station=takhli"
    transport = ReplayTransport(
        {page: b'<img src="/opendata/radar_data/cappi/takhli/2026091803420400dBZ.cappi.png">'}
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "th_royalrain",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        refs = await ThRoyalRainSource(registry.get_info("th_royalrain")).discover(
            Query("th_royalrain", latest=True, stations=("takhli",)), context
        )
        assert [ref.station for ref in refs] == ["takhli"]
        assert transport.get_calls == [(page, {"User-Agent": "Mozilla/5.0 (compatible; radiust/1)"})]
    finally:
        context.close()
