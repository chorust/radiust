from __future__ import annotations

from datetime import datetime, timezone
from urllib.parse import parse_qs, urlparse

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.bmkg import BmkgSource


@pytest.mark.asyncio
async def test_bmkg_builds_tms_tiles_with_cadence_time(tmp_path, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(BmkgSource, "frame_time", lambda self: valid_time)
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "bmkg",
        temp_root=tmp_path,
    )
    try:
        ref = (await BmkgSource(registry.get_info("bmkg")).discover(Query("bmkg", latest=True), context))[0]
        assert ref.valid_time == valid_time
        assert len(ref.locator["artifacts"]) == 3
        urls = [ref.uri, *(item["url"] for item in ref.locator["artifacts"])]
        parsed = [urlparse(url) for url in urls]
        assert all("/202609180410/202609180410/1/" in item.path for item in parsed)
        assert {tuple(parse_qs(item.query)["overlays"]) for item in parsed} == {("contourf",)}
        assert "/1/0/1.png" in urls[0]
        assert any("/1/1/0.png" in url for url in urls)
    finally:
        context.close()
