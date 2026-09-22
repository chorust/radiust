from __future__ import annotations

from datetime import datetime, timezone

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.vn import VnSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_vn_ignores_stale_retired_frame_and_preserves_live_cmax(tmp_path, fixture_root, monkeypatch):
    monkeypatch.setattr(VnSource, "STATIONS", ("PLI", "HUE"))
    pages = {
        f"{VnSource.BASE_URL}/radar/PLI": b'<script>tentimesett[0] = "202609180350";</script>',
        f"{VnSource.BASE_URL}/radar/HUE": b'<script>tentimesett[0] = "202312300740";</script>',
    }
    image_url = f"{VnSource.BASE_URL}/dataout_web/PLI/20260918/PLI_202609180350_CMAX00.png"
    pages[image_url] = (fixture_root / "vn/raw/PLI.png").read_bytes()
    transport = ReplayTransport(pages)
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "vn", transport=transport, temp_root=tmp_path)
    source = VnSource(registry.get_info("vn"))
    refs = await source.discover(Query("vn", latest=True), context)
    assert [(ref.station, ref.valid_time) for ref in refs] == [("PLI", datetime(2026, 9, 18, 3, 50, tzinfo=timezone.utc))]
    raw = await source.download(refs[0], context)
    try:
        assert raw.artifacts[0].sha256 == "c6e6f1cffec92a519a1e9f6db8f84ae81e5825538aa8f2ac2286814802324832"
        with pytest.raises(DecodeError, match="verified scientific decoder"):
            source.decode(raw, context)
    finally:
        raw.close()
        context.close()
