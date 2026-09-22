from __future__ import annotations

from datetime import datetime, timezone

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.opensnow import OpenSnowSource


@pytest.mark.asyncio
async def test_opensnow_builds_four_rainviewer_like_tiles(tmp_path, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(OpenSnowSource, "frame_time", lambda self: valid_time)
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "opensnow",
        temp_root=tmp_path,
    )
    try:
        ref = (await OpenSnowSource(registry.get_info("opensnow")).discover(Query("opensnow", latest=True), context))[0]
        assert ref.station == "global"
        assert ref.metadata["tile_layout"] == "xyz-2x2-global"
        urls = [ref.uri, *(item["url"] for item in ref.locator["artifacts"])]
        assert len(urls) == 4
        assert all("/256/1/" in url and url.endswith("/6/1_0.png") for url in urls)
    finally:
        context.close()
