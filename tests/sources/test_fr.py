from __future__ import annotations

import hashlib
import io
import json
from dataclasses import dataclass
from datetime import datetime, timezone
from urllib.parse import parse_qs, urlparse

import pytest
from PIL import Image
from pyproj import Transformer
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, IntegrityError
from radiust.identity import safe_ref
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.fr import FrSource, _rot13


@dataclass(frozen=True)
class ReplayResponse:
    body: bytes
    headers: dict[str, str]
    status: int = 200
    url: str = ""


class ReplayTransport:
    def __init__(self, page_cookie: str, png: bytes):
        self.page_cookie = page_cookie
        self.png = png
        self.requests: list[tuple[str, dict[str, str]]] = []

    async def get_response(self, url: str, *, headers=None) -> ReplayResponse:
        self.requests.append((url, dict(headers or {})))
        assert url == FrSource.PAGE_URL
        return ReplayResponse(
            b"<html></html>",
            {"set-cookie": f"mfsession={self.page_cookie}; Path=/; Max-Age=3600; Secure"},
            url=url,
        )

    async def get(self, url: str, *, headers=None) -> bytes:
        self.requests.append((url, dict(headers or {})))
        return self.png


def _png() -> bytes:
    image = Image.new("RGBA", (700, 600), (50, 80, 120, 255))
    output = io.BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


def _context(tmp_path, transport: ReplayTransport) -> SourceContext:
    return SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "fr",
        transport=transport,
        temp_root=tmp_path,
    )


@pytest.mark.asyncio
async def test_fr_derives_session_token_and_builds_current_wms_time_request(tmp_path):
    cookie = "AbcXYZ123"
    transport = ReplayTransport(cookie, _png())
    source = FrSource(registry.get_info("fr"))
    context = _context(tmp_path, transport)
    try:
        ref = (await source.discover(Query("fr", latest=True), context))[0]
        assert transport.requests == []
        parsed = urlparse(ref.uri)
        params = parse_qs(parsed.query)
        assert params["layers"] == ["BASE_REFLECTIVITY"]
        assert params["styles"] == ["synopsis_reflectivity_oppidum_transparence"]
        assert params["crs"] == ["EPSG:3857"]
        assert params["width"] == ["700"]
        assert params["height"] == ["600"]
        assert "token" not in params
        assert _rot13(cookie) not in repr(ref)
        assert _rot13(cookie) not in str(safe_ref(ref))
        assert "time" in params
        frame_time = ref.valid_time.astimezone(timezone.utc)
        assert frame_time.minute % 15 == 0
        assert frame_time.second == 0
        assert params["time"] == [frame_time.strftime("%Y-%m-%dT%H:%M:%SZ")]
        assert ref.station == "FRCOMP"
        assert tuple(ref.locator["bbox"]) == pytest.approx(FrSource.default_bbox)
        assert ref.metadata["time_semantics"] == "wms_time_dimension"
        requested_bounds = tuple(float(value) for value in params["bbox"][0].split(","))
        assert ref.metadata["wms_request_bbox_m"] == pytest.approx(requested_bounds, abs=1e-6)
        assert tuple(ref.metadata["wms_request_size"]) == (700, 600)
        west, south, east, north = FrSource.default_bbox
        project = Transformer.from_crs("EPSG:4326", "EPSG:3857", always_xy=True)
        assert requested_bounds[:2] == pytest.approx(project.transform(west, south), abs=1e-4)
        assert requested_bounds[2:] == pytest.approx(project.transform(east, north), abs=1e-4)
        x0, y0, x1, y1 = requested_bounds
        col, row = 350, 300
        pixel_center = (x0 + (col + 0.5) * (x1 - x0) / 700, y1 - (row + 0.5) * (y1 - y0) / 600)
        assert ref.metadata["wms_request_center_m"] == pytest.approx(pixel_center, abs=1e-6)

        raw = await source.download(ref, context)
        try:
            wms_urls = [url for url, _ in transport.requests if url.startswith(FrSource.WMS_BASE_URL)]
            assert len(wms_urls) == 1
            assert parse_qs(urlparse(wms_urls[0]).query)["token"] == [_rot13(cookie)]
            assert sum(url == FrSource.PAGE_URL for url, _ in transport.requests) == 1
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_fr_preserves_raw_and_rejects_legacy_luminance_as_dbz(tmp_path):
    transport = ReplayTransport("AbcXYZ123", _png())
    source = FrSource(registry.get_info("fr"))
    context = _context(tmp_path, transport)
    try:
        ref = (await source.discover(Query("fr", latest=True), context))[0]
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
async def test_fr_wms_raw_is_validated_as_png_and_has_a_stable_file_name(tmp_path):
    payload = _png()
    transport = ReplayTransport("AbcXYZ123", payload)
    source = FrSource(registry.get_info("fr"))
    context = _context(tmp_path, transport)
    try:
        ref = (await source.discover(Query("fr", latest=True), context))[0]
        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].name.startswith("FRCOMP_")
            assert raw.artifacts[0].name.endswith(".png")
            assert raw.artifacts[0].media_type == "image/png"
            assert raw.bytes() == payload
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_fr_rejects_http_200_wms_xml_service_exception_as_raw(tmp_path):
    transport = ReplayTransport(
        "AbcXYZ123",
        b'<?xml version="1.0"?><ServiceExceptionReport><ServiceException>layer missing</ServiceException></ServiceExceptionReport>',
    )
    source = FrSource(registry.get_info("fr"))
    context = _context(tmp_path, transport)
    try:
        ref = (await source.discover(Query("fr", latest=True), context))[0]
        with pytest.raises(IntegrityError, match="invalid image artifact"):
            await source.download(ref, context)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_fr_retained_wms_frame_matches_references_and_stays_display_only(
    tmp_path, fixture_root, monkeypatch
):
    fixture_root = fixture_root / "fr"
    fixture = json.loads((fixture_root / "fixture.json").read_text(encoding="utf-8"))
    frame = fixture["frames"][0]
    artifact = frame["artifacts"][0]
    payload = (fixture_root / artifact["path"]).read_bytes()
    assert hashlib.sha256(payload).hexdigest() == artifact["sha256"]

    expected_time = datetime.fromisoformat(frame["valid_time"].replace("Z", "+00:00"))
    monkeypatch.setattr(
        FrSource,
        "_current_frame_time",
        staticmethod(lambda now=None: expected_time),
    )
    transport = ReplayTransport("AbcXYZ123", payload)
    source = FrSource(registry.get_info("fr"))
    context = _context(tmp_path, transport)
    try:
        ref = (await source.discover(Query("fr", latest=True), context))[0]
        assert ref.valid_time == expected_time
        assert parse_qs(urlparse(ref.uri).query)["time"] == [
            expected_time.strftime("%Y-%m-%dT%H:%M:%SZ")
        ]

        with Image.open(io.BytesIO(payload)) as image:
            image.load()
            assert image.format == "PNG"
            assert image.mode == "RGBA"
            assert image.size == tuple(reversed(frame["metadata"]["reference_shape"]))
            for coordinate, rgba in frame["metadata"]["reference_rgba"].items():
                row, column = map(int, coordinate.split(","))
                assert image.getpixel((column, row)) == tuple(rgba)

        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].sha256 == artifact["sha256"]
            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()
