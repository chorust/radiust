from __future__ import annotations

import importlib
from datetime import datetime, timezone

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.ca import CaSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_ca_walks_cappi_directory_and_preserves_rain_gif(tmp_path, fixture_root, monkeypatch):
    class FixedDateTime(datetime):
        @classmethod
        def now(cls, tz=None):
            return cls(2026, 9, 18, 4, 0, tzinfo=tz or timezone.utc)

    ca_module = importlib.import_module("radiust.sources.ca")
    monkeypatch.setattr(ca_module, "datetime", FixedDateTime)
    root = "https://dd.meteo.gc.ca/20260918/WXO-DD/radar/CAPPI/GIF/"
    station = root + "CASFT/"
    level = station + "1.5/"
    image = level + "202609180354_CASFT_CAPPI_1.5_RAIN.gif"
    transport = ReplayTransport(
        {
            root: b'<a href="CASFT/">CASFT/</a>',
            station: b'<a href="1.5/">1.5/</a>',
            level: b'<a href="202609180354_CASFT_CAPPI_1.5_RAIN.gif">rain</a>',
            image: (fixture_root / "ca/raw/CASFT.gif").read_bytes(),
        }
    )
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "ca", transport=transport, temp_root=tmp_path)
    source = CaSource(registry.get_info("ca"))
    ref = (await source.discover(Query("ca", latest=True), context))[0]
    assert ref.station == "CASFT"
    assert ref.valid_time == datetime(2026, 9, 18, 3, 54, tzinfo=timezone.utc)
    raw = await source.download(ref, context)
    try:
        assert raw.artifacts[0].sha256 == "90d1df687644579822459a2527b8d300f8a3b4e0de73e226a7db62dab7fee5b0"
        with pytest.raises(DecodeError, match="verified scientific decoder"):
            source.decode(raw, context)
    finally:
        raw.close()
        context.close()


@pytest.mark.asyncio
async def test_ca_station_query_does_not_walk_other_station_directories(tmp_path, monkeypatch):
    class FixedDateTime(datetime):
        @classmethod
        def now(cls, tz=None):
            return cls(2026, 9, 18, 4, 0, tzinfo=tz or timezone.utc)

    ca_module = importlib.import_module("radiust.sources.ca")
    monkeypatch.setattr(ca_module, "datetime", FixedDateTime)
    root = "https://dd.meteo.gc.ca/20260918/WXO-DD/radar/CAPPI/GIF/"
    station = root + "CASFT/"
    level = station + "1.5/"
    transport = ReplayTransport(
        {
            root: b'<a href="CASFT/">CASFT/</a><a href="CARAA/">CARAA/</a>',
            station: b'<a href="1.5/">1.5/</a>',
            level: b'<a href="202609180354_CASFT_CAPPI_1.5_RAIN.gif">rain</a>',
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "ca",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        refs = await CaSource(registry.get_info("ca")).discover(
            Query("ca", latest=True, stations=("CASFT",)), context
        )
        assert [ref.station for ref in refs] == ["CASFT"]
        assert [url for url, _headers in transport.get_calls] == [root, station, level]
    finally:
        context.close()
