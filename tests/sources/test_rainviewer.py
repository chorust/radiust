from __future__ import annotations

import io
import json
from datetime import datetime, timezone

import numpy as np
import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import Query, SourceInfo
from radiust.sources.rainviewer import RainViewerSource

API_URL = "https://api.rainviewer.com/public/weather-maps.json"
HOST = "https://tilecache.rainviewer.com"
FRAME_PATH = "/v2/radar/test-frame"


class ReplayTransport:
    def __init__(self, responses: dict[str, bytes]):
        self.responses = responses
        self.urls: list[str] = []

    async def get(self, url: str) -> bytes:
        self.urls.append(url)
        return self.responses[url]


def _info() -> SourceInfo:
    from radiust.registry import get_source

    return get_source("rainviewer").info


def _context(transport: ReplayTransport, tmp_path) -> SourceContext:
    return SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "rainviewer",
        transport=transport,
        temp_root=tmp_path,
    )


def _png(color: tuple[int, int, int, int]) -> bytes:
    image = Image.new("RGBA", (512, 512), color)
    output = io.BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


def _tile_url(x: int, y: int) -> str:
    return f"{HOST}{FRAME_PATH}/512/1/{x}/{y}/2/0_0.png"


def _manifest() -> bytes:
    return json.dumps(
        {
            "version": "2.0",
            "generated": 1_800_000_000,
            "host": HOST,
            "radar": {
                "past": [
                    {"time": 1_799_999_400, "path": "/v2/radar/old-frame"},
                    {"time": 1_800_000_000, "path": FRAME_PATH},
                ],
                "nowcast": [],
            },
        }
    ).encode()


@pytest.mark.asyncio
async def test_discover_uses_upstream_frames_and_query_time_semantics(tmp_path):
    transport = ReplayTransport({API_URL: _manifest()})
    source = RainViewerSource(_info())
    context = _context(transport, tmp_path)
    try:
        refs = await source.discover(Query("rainviewer", latest=True), context)
        assert len(refs) == 1
        assert refs[0].valid_time == datetime.fromtimestamp(1_800_000_000, timezone.utc)
        assert refs[0].revision == "test-frame"
        assert refs[0].metadata["time_semantics"] == "frame_generation_time"
        assert refs[0].valid_time.timestamp() - 1_799_999_400 == 600

        exact = await source.discover(
            Query("rainviewer", at=datetime.fromtimestamp(1_799_999_400, timezone.utc)),
            context,
        )
        assert [ref.revision for ref in exact] == ["old-frame"]
    finally:
        context.close()


@pytest.mark.asyncio
async def test_discovery_wraps_non_utf8_manifest_as_decode_error(tmp_path):
    transport = ReplayTransport({API_URL: b"\xff"})
    source = RainViewerSource(_info())
    context = _context(transport, tmp_path)
    try:
        with pytest.raises(DecodeError, match="invalid RainViewer API response"):
            await source.discover(Query("rainviewer", latest=True), context)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_download_preserves_all_global_tiles_as_raw_artifacts(tmp_path):
    responses = {_tile_url(x, y): _png((206, 192, 135, 150)) for y in range(2) for x in range(2)}
    transport = ReplayTransport(responses)
    source = RainViewerSource(_info())
    context = _context(transport, tmp_path)
    try:
        discovery_context = _context(ReplayTransport({API_URL: _manifest()}), tmp_path / "discover")
        try:
            ref = (await source.discover(Query("rainviewer", latest=True), discovery_context))[0]
        finally:
            discovery_context.close()
        raw = await source.download(ref, context)
        assert len(raw.artifacts) == 4
        assert {artifact.role for artifact in raw.artifacts} == {"tile"}
        assert raw.metadata["tile_size"] == 512
        assert raw.metadata["zoom"] == 1
        assert len(transport.urls) == 4
        assert all(url.endswith(".png") for url in transport.urls)
        assert all(".webp" not in url for url in transport.urls)
        raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_decode_maps_universal_blue_rgba_to_dbz_and_quality(tmp_path):
    responses = {_tile_url(x, y): _png((206, 192, 135, 150)) for y in range(2) for x in range(2)}
    transport = ReplayTransport(responses)
    source = RainViewerSource(_info())
    context = _context(transport, tmp_path)
    try:
        discovery_transport = ReplayTransport({API_URL: _manifest()})
        discovery_context = _context(discovery_transport, tmp_path / "discover")
        ref = (await source.discover(Query("rainviewer", latest=True), discovery_context))[0]
        discovery_context.close()
        raw = await source.download(ref, context)
        field = source.decode(raw, context)
        assert field.data.shape == (1024, 1024)
        assert field.data.attrs["units"] == "dBZ"
        assert np.allclose(field.data.values, 10.0)
        assert np.all(field.quality.values == 0)
        assert field.grid.crs == "EPSG:4326"
        raw.close()
    finally:
        context.close()
