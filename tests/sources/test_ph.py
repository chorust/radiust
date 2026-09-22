from __future__ import annotations

import json
from datetime import datetime, timezone
from urllib.parse import parse_qs, urlencode, urlparse

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.identity import safe_ref
from radiust.logging import redact
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.ph import PhSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_ph_applies_csrf_and_converts_manila_timeline(tmp_path):
    credential = "fixture token+&value=1"
    timeline_url = f"{PhSource.TIMELINE_URL}?{urlencode({'token': credential})}"
    transport = ReplayTransport(
        {
            PhSource.BASE_URL: b'<meta name="csrf-token" content="csrf-value">',
            timeline_url: b'{"data":{"timeline":[{"observed_at":"2026-09-18 12:00:00","image_url":"https://example.test/ph.png"}]}}',
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}, "sources": {"ph": {"timeline_token": credential}}}, environ={}),
        "ph",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        ref = (await PhSource(registry.get_info("ph")).discover(Query("ph", latest=True), context))[0]
        assert ref.station == "PHCOMP4"
        assert ref.valid_time == datetime(2026, 9, 18, 4, 0, tzinfo=timezone.utc)
        assert transport.get_calls[1][1]["X-CSRF-TOKEN"] == "csrf-value"
        assert parse_qs(urlparse(transport.get_calls[1][0]).query) == {"token": [credential]}
        assert credential not in ref.uri
        assert credential not in ref.logical_id
        assert credential not in repr(ref)
        assert credential not in json.dumps(safe_ref(ref))
        assert credential not in redact(transport.get_calls[1][0])
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "payload",
    [b"\xff", b"{not-json", b'{"data":[]}'],
    ids=["non-utf8", "invalid-json", "invalid-shape"],
)
async def test_ph_rejects_malformed_timeline_instead_of_reporting_no_data(tmp_path, payload):
    credential = "fixture-token"
    timeline_url = f"{PhSource.TIMELINE_URL}?{urlencode({'token': credential})}"
    transport = ReplayTransport({PhSource.BASE_URL: b"", timeline_url: payload})
    context = SourceContext(
        load_config(
            {"runtime": {"allow_network": True}, "sources": {"ph": {"timeline_token": credential}}},
            environ={},
        ),
        "ph",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(DecodeError, match="PAGASA.*timeline"):
            await PhSource(registry.get_info("ph")).discover(Query("ph", latest=True), context)
    finally:
        context.close()
