from __future__ import annotations

from datetime import datetime, timezone
from io import BytesIO

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.cam import CamSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_cam_parses_slideshow_filename_and_station(tmp_path):
    image_path = "/radar/20260918040500_cambodia_composite.jpg"
    image_url = f"{CamSource.BASE_URL}{image_path}"
    page = f'<img src="{image_path}">'.encode()
    image = BytesIO()
    Image.new("RGB", (2, 2), (10, 20, 30)).save(image, format="JPEG")
    transport = ReplayTransport({CamSource.PAGE_URL: page, image_url: image.getvalue()})
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "cam",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        ref = (await CamSource(registry.get_info("cam")).discover(Query("cam", latest=True), context))[0]
        assert transport.get_calls == [
            (
                CamSource.PAGE_URL,
                {
                    "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
                    "Accept-Language": "zh,en;q=0.9",
                    "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
                },
            )
        ]
        assert ref.station == "cambodia"
        assert ref.valid_time == datetime(2026, 9, 18, 4, 5, tzinfo=timezone.utc)
        assert ref.uri == image_url
        raw = await CamSource(registry.get_info("cam")).download(ref, context)
        try:
            assert raw.artifacts[0].size_bytes == len(image.getvalue())
            assert raw.artifacts[0].media_type == "image/jpeg"
            assert transport.get_calls[-1] == (image_url, None)
        finally:
            raw.close()
    finally:
        context.close()
