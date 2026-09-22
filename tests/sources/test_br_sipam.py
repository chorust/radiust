from __future__ import annotations

import json
from datetime import datetime, timezone

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import ProductInfo, Query, SourceInfo
from radiust.sources.br_sipam import BrSipamSource

from tests.support.http_replay import ReplayTransport

INFO = SourceInfo(
    id="br_sipam",
    description="Brazil SIPAM radar",
    adapter_version="1",
    products=(ProductInfo("dbz", ("reflectivity",), {"reflectivity": "dBZ"}, default=True),),
)


@pytest.mark.asyncio
async def test_br_sipam_discovers_utc_scan_and_declared_bbox(tmp_path, fixture_root):
    metadata_url = "https://apihidro.sipam.gov.br/radares/"
    image_url = "https://siger.sipam.gov.br/radar/sbbe/dbz/2026_09_18_06_20_05.png"
    response = [{
        "nomeRadar": "sbbe",
        "nomeMunicipio": "Belém",
        "latitudeMax": 0.85202,
        "latitudeMin": -3.67153,
        "longitudeMax": -46.21039,
        "longitudeMin": -50.70789,
        "produtos": ["dbz"],
        "varreduras": ["2026-09-18T06:20:05Z"],
    }]
    transport = ReplayTransport({metadata_url: json.dumps(response).encode(), image_url: (fixture_root / "br_sipam/raw/BRBE_20260918T062005Z.png").read_bytes()})
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "br_sipam",
        transport=transport,
        temp_root=tmp_path,
    )
    source = BrSipamSource(INFO)
    try:
        ref = (await source.discover(Query("br_sipam", latest=True), context))[0]
        assert ref.station == "BRBE"
        assert ref.valid_time == datetime(2026, 9, 18, 6, 20, 5, tzinfo=timezone.utc)
        assert tuple(ref.locator["bbox"]) == pytest.approx((-50.70789, -3.67153, -46.21039, 0.85202))
        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].sha256 == "65c989f153c8ae4636beb1b5be78577d32574baef64dead9de853a7ec9e7c9bb"
            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()
