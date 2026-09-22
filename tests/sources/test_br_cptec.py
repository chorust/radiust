from __future__ import annotations

import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import parse_qs, urlencode, urlparse

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, NoDataError
from radiust.models import ProductInfo, Query, SourceInfo
from radiust.sources.br_cptec import BrCptecSource

from tests.support.http_replay import ReplayTransport

INFO = SourceInfo(
    id="br_cptec",
    description="Brazil CPTEC radar",
    adapter_version="1",
    products=(ProductInfo("CAPPI", ("reflectivity",), {"reflectivity": "dBZ"}, default=True),),
)

WMS_CAPABILITIES = b"""<?xml version="1.0"?>
<WMS_Capabilities xmlns="http://www.opengis.net/wms">
  <Capability><Layer>
    <Layer>
      <Name>DISSM_Nowcasting_R121061400</Name>
      <Title>CAPPI-3-km-(refletividade)</Title>
      <CRS>EPSG:4326</CRS><CRS>CRS:84</CRS>
      <BoundingBox CRS="CRS:84" minx="-37.73875648" miny="-10.42425186" maxx="-33.18227648" maxy="-5.93611186" />
      <Dimension name="time" default="2026-09-19T23:55:00Z" units="ISO8601">2026-09-19T21:05:00Z,2026-09-19T23:55:00Z</Dimension>
      <Style><Name>raster</Name></Style>
    </Layer>
  </Layer></Capability>
</WMS_Capabilities>"""

WMS_ARCHIVE_CAPABILITIES = (
    WMS_CAPABILITIES.replace(b"R121061400", b"R12475984")
    .replace(b"-37.73875648", b"-47.802643562")
    .replace(b"-10.42425186", b"-20.43563066925")
    .replace(b"-33.18227648", b"-43.066081562")
    .replace(b"-5.93611186", b"-15.94054991925")
    .replace(b"2026-09-19T21:05:00Z", b"2018-05-10T11:50:00Z")
    .replace(b"2026-09-19T23:55:00Z", b"2018-05-10T11:50:00Z")
)


@pytest.mark.asyncio
async def test_br_cptec_parses_legacy_radar_page_and_cappi_response(tmp_path):
    page = b"window.app.radares = [{id: 1, nome: 'Radar One'}];"
    api_url = f"{BrCptecSource.RADAR_API_URL.format(radar_id='1')}?{urlencode({'quantidade': 5, 'nome': 'Radar One'})}"
    response = {
        "items": [{
            "produto": "CAPPI",
            "boundingBox": [-50.0, -10.0, -45.0, -5.0],
            "imagens": [{
                "fileDate": "2026-09-18",
                "fileTime": "06:00:00",
                "urls": [{"url": "/radar/1/cappi.png"}],
            }],
        }]
    }
    transport = ReplayTransport(
        {
            BrCptecSource.RADAR_PAGE_URL: page,
            api_url: json.dumps(response).encode(),
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "br_cptec",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        ref = (await BrCptecSource(INFO).discover(Query("br_cptec", latest=True), context))[0]
        assert ref.station == "BRCPTEC1"
        assert ref.valid_time == datetime(2026, 9, 18, 6, 0, tzinfo=timezone.utc)
        assert ref.uri == "https://nowcasting.cptec.inpe.br/radar/1/cappi.png"
        assert tuple(ref.locator["bbox"]) == pytest.approx((-50.0, -10.0, -45.0, -5.0))
    finally:
        context.close()


@pytest.mark.asyncio
async def test_br_cptec_current_frontend_without_legacy_route_fails_closed(tmp_path):
    current_page = b"<!doctype html><script src='/assets/nowcasting.bundle.js'></script>"
    transport = ReplayTransport(
        {
            BrCptecSource.RADAR_PAGE_URL: current_page,
            BrCptecSource.WMS_CAPABILITIES_URL: b"<WMS_Capabilities />",
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "br_cptec",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(NoDataError, match="no matching frame"):
            await BrCptecSource(INFO).discover(Query("br_cptec", latest=True), context)
        assert [url for url, _headers in transport.get_calls] == [
            BrCptecSource.RADAR_PAGE_URL,
            BrCptecSource.WMS_CAPABILITIES_URL,
        ]
    finally:
        context.close()


@pytest.mark.asyncio
async def test_br_cptec_discovers_current_cappi_frames_from_wms_capabilities(tmp_path):
    current_page = b"<!doctype html><script src='/assets/nowcasting.bundle.js'></script>"
    transport = ReplayTransport(
        {
            BrCptecSource.RADAR_PAGE_URL: current_page,
            BrCptecSource.WMS_CAPABILITIES_URL: WMS_CAPABILITIES,
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "br_cptec",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        refs = await BrCptecSource(INFO).discover(Query("br_cptec", latest=True), context)
        assert len(refs) == 1
        ref = refs[0]
        assert ref.station == "R121061400"
        assert ref.valid_time == datetime(2026, 9, 19, 23, 55, tzinfo=timezone.utc)
        assert tuple(ref.locator["bbox"]) == pytest.approx(
            (-37.73875648, -10.42425186, -33.18227648, -5.93611186)
        )
        assert ref.metadata["interface"] == "wms"
        params = parse_qs(urlparse(ref.uri).query)
        assert params["layers"] == ["DISSM_Nowcasting_R121061400"]
        assert params["time"] == ["2026-09-19T23:55:00Z"]
        assert params["crs"] == ["CRS:84"]
        assert params["bbox"] == ["-37.73875648,-10.42425186,-33.18227648,-5.93611186"]
    finally:
        context.close()


@pytest.mark.asyncio
async def test_br_cptec_does_not_treat_wms_service_exception_as_an_image(tmp_path):
    current_page = b"<!doctype html><script src='/assets/nowcasting.bundle.js'></script>"
    service_exception = (
        b"<?xml version='1.0'?><ServiceExceptionReport><ServiceException>"
        b"Error rendering coverage on the fast path: coordinate reference system mismatch"
        b"</ServiceException></ServiceExceptionReport>"
    )
    transport = ReplayTransport(
        {
            BrCptecSource.RADAR_PAGE_URL: current_page,
            BrCptecSource.WMS_CAPABILITIES_URL: WMS_CAPABILITIES,
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "br_cptec",
        transport=transport,
        temp_root=tmp_path,
    )
    source = BrCptecSource(INFO)
    try:
        ref = (await source.discover(Query("br_cptec", latest=True), context))[0]
        transport.responses[ref.uri] = service_exception
        with pytest.raises(DecodeError, match="WMS GetMap returned a ServiceException"):
            await source.download(ref, context)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_br_cptec_replays_retained_wms_raw_and_preserves_raw_pixel_reference(tmp_path):
    manifest_path = Path(__file__).parents[1] / "fixtures/sources/br_cptec/fixture.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    frame = manifest["frames"][0]
    artifact = frame["artifacts"][0]
    fixture_path = manifest_path.parent / artifact["path"]
    payload = fixture_path.read_bytes()
    assert hashlib.sha256(payload).hexdigest() == artifact["sha256"]
    assert len(payload) == artifact["size_bytes"]
    with Image.open(fixture_path) as image:
        assert list(image.size[::-1]) == frame["metadata"]["reference_shape"]
        reference_pixel = frame["metadata"]["reference_pixels"][0]
        assert list(image.convert("RGBA").getpixel((reference_pixel["column"], reference_pixel["row"]))) == reference_pixel["rgba"]

    transport = ReplayTransport(
        {
            BrCptecSource.RADAR_PAGE_URL: b"<!doctype html><script src='/assets/nowcasting.bundle.js'></script>",
            BrCptecSource.WMS_CAPABILITIES_URL: WMS_ARCHIVE_CAPABILITIES,
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "br_cptec",
        transport=transport,
        temp_root=tmp_path,
    )
    source = BrCptecSource(INFO)
    try:
        refs = await source.discover(
            Query("br_cptec", latest=True, stations=(frame["station"],)), context
        )
        assert len(refs) == 1
        ref = refs[0]
        assert ref.valid_time == datetime(2018, 5, 10, 11, 50, tzinfo=timezone.utc)
        transport.responses[ref.uri] = payload
        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].sha256 == artifact["sha256"]
            with pytest.raises(DecodeError, match="no verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_br_cptec_rejects_malformed_legacy_api_json(tmp_path):
    page = b"window.app.radares = [{id: 'r1', nome: 'Radar One'}];"
    api_url = f"{BrCptecSource.RADAR_API_URL.format(radar_id='r1')}?{urlencode({'quantidade': 5, 'nome': 'Radar One'})}"
    transport = ReplayTransport(
        {
            BrCptecSource.RADAR_PAGE_URL: page,
            api_url: b"not-json",
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "br_cptec",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(DecodeError, match="not valid JSON"):
            await BrCptecSource(INFO).discover(Query("br_cptec", latest=True), context)
    finally:
        context.close()
