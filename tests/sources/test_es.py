from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.es import EsSource

TIMELINE = "https://www.aemet.es/es/api-eltiempo/radar/timeline/compo/PB"
IMAGE = "https://www.aemet.es/es/api-eltiempo/radar/imagen-radar/compo/radw202609171440_3857.png"


class ReplayTransport:
    def __init__(self, responses): self.responses = responses
    async def get(self, url, *, headers=None): return self.responses[url]


@pytest.mark.asyncio
async def test_es_discovers_timeline_frame_and_preserves_raw(tmp_path):
    payload = json.dumps([{"Elementos": [{"Fecha": "2026-09-17T16:40:00+02:00", "Nombre fichero": "radw202609171440_3857.png"}]}]).encode()
    image = (Path(__file__).parents[1] / "fixtures/sources/es/raw/ESCOMP.png").read_bytes()
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "es", transport=ReplayTransport({TIMELINE: payload, IMAGE: image}), temp_root=tmp_path)
    source = EsSource(registry.get_info("es"))
    try:
        ref = (await source.discover(Query("es", latest=True), context))[0]
        assert ref.station == "ESCOMP"
        assert ref.valid_time == datetime(2026, 9, 17, 14, 40, tzinfo=timezone.utc)
        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].sha256 == "baf34e1bc4ac94c77a78994bbc153e480e67b87681eb9473534237fac9a35c2f"
            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_es_wraps_non_utf8_timeline_as_decode_error(tmp_path):
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "es",
        transport=ReplayTransport({TIMELINE: b"\xff"}),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(DecodeError, match="AEMET returned invalid timeline JSON"):
            await EsSource(registry.get_info("es")).discover_entries(context)
    finally:
        context.close()
