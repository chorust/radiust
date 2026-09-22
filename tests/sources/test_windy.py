from __future__ import annotations

import hashlib
import io
from datetime import datetime, timezone

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, IntegrityError, ResourceLimitError, TransportError
from radiust.models import Artifact, Query
from radiust.raw import RawFrame
from radiust.registry import registry
from radiust.sources.windy import WindySource

from tests.support.http_replay import ReplayTransport


class _BrowserReplay:
    def __init__(self, responses: dict[str, bytes]):
        self.responses = responses
        self.calls: list[tuple] = []

    async def acquire(self, ref, context, **options):
        self.calls.append((ref, options))
        url = options["page_url"]
        name = options["artifact_name"]
        payload = self.responses[url]
        path = context.temp_root / name
        path.write_bytes(payload)
        return RawFrame(
            ref,
            (
                Artifact(
                    name=name,
                    role="data",
                    media_type="image/png",
                    payload=path,
                    size_bytes=len(payload),
                    sha256=hashlib.sha256(payload).hexdigest(),
                ),
            ),
            metadata={"acquisition": "playwright"},
        )


class _NoHttpTransport:
    async def get(self, url, *, headers=None):
        raise AssertionError("HTTP acquisition used while Playwright mode is selected")


def _png(size: int) -> bytes:
    output = io.BytesIO()
    Image.new("RGBA", (size, size), (10, 20, 30, 255)).save(output, format="PNG")
    return output.getvalue()


@pytest.mark.asyncio
async def test_windy_preserves_live_four_tile_raw_and_keeps_science_blocked(
    tmp_path, fixture_root, monkeypatch
):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(WindySource, "frame_time", lambda self: valid_time)
    source = WindySource(registry.get_info("windy"))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "windy",
        transport=ReplayTransport(),
        temp_root=tmp_path,
    )
    ref = (await source.discover(Query("windy", latest=True), context))[0]
    assert ref.valid_time == valid_time
    assert ref.metadata["time_semantics"] == "cadence_floor_assumption"
    fixture = fixture_root / "windy/raw"
    for y in range(2):
        for x in range(2):
            url = source.tile_url(valid_time, x, y, context)
            context.transport.responses[url] = (fixture / f"tile-z1-x{x}-y{y}.png").read_bytes()
    raw = await source.download(ref, context)
    try:
        assert [artifact.sha256 for artifact in raw.artifacts] == [
            "a3367ba4fa0aa5130690232fe91ec74dd42bda20e2818dee402bf1a80173a915",
            "b7b7a6193388a6d00835cff7d274406c34b2f638d1f19a11cb0c1f5d45f4506c",
            "717363470ca028030e1fbde003527145097367bd26ce75f959ea67f09d7edb7c",
            "9052bae7ccf8af41e8046c0c2f1abe69f30803861bded455424ce9c9d3acef09",
        ]
        with pytest.raises(DecodeError, match="verified scientific decoder"):
            source.decode(raw, context)
    finally:
        raw.close()
        context.close()


@pytest.mark.asyncio
async def test_windy_can_acquire_all_tiles_through_explicit_playwright_mode(
    tmp_path, fixture_root, monkeypatch
):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(WindySource, "frame_time", lambda self: valid_time)
    source = WindySource(registry.get_info("windy"))
    context = SourceContext(
        load_config(
            {
                "runtime": {"allow_network": True},
                "sources": {"windy": {"use_playwright": True}},
            },
            environ={},
        ),
        "windy",
        transport=_NoHttpTransport(),
        temp_root=tmp_path,
    )
    ref = (await source.discover(Query("windy", latest=True), context))[0]
    fixture = fixture_root / "windy/raw"
    responses = {
        source.tile_url(valid_time, x, y, context): (fixture / f"tile-z1-x{x}-y{y}.png").read_bytes()
        for y in range(2)
        for x in range(2)
    }
    browser = _BrowserReplay(responses)
    source._browser_acquirer = browser

    try:
        raw = await source.download(ref, context)
        try:
            assert raw.metadata["acquisition"] == "playwright"
            assert len(browser.calls) == 4
            assert [artifact.role for artifact in raw.artifacts] == ["data", "tile", "tile", "tile"]
            assert [artifact.sha256 for artifact in raw.artifacts] == [
                "a3367ba4fa0aa5130690232fe91ec74dd42bda20e2818dee402bf1a80173a915",
                "b7b7a6193388a6d00835cff7d274406c34b2f638d1f19a11cb0c1f5d45f4506c",
                "717363470ca028030e1fbde003527145097367bd26ce75f959ea67f09d7edb7c",
                "9052bae7ccf8af41e8046c0c2f1abe69f30803861bded455424ce9c9d3acef09",
            ]
            assert {call[1]["page_url"] for call in browser.calls} == set(responses)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_windy_playwright_mode_does_not_bypass_network_opt_in(tmp_path, fixture_root, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(WindySource, "frame_time", lambda self: valid_time)
    source = WindySource(registry.get_info("windy"))
    context = SourceContext(
        load_config(
            {
                "runtime": {"allow_network": False},
                "sources": {"windy": {"use_playwright": True}},
            },
            environ={},
        ),
        "windy",
        transport=_NoHttpTransport(),
        temp_root=tmp_path,
    )
    ref = (await source.discover(Query("windy", latest=True), context))[0]
    fixture = fixture_root / "windy/raw"
    responses = {
        source.tile_url(valid_time, x, y, context): (fixture / f"tile-z1-x{x}-y{y}.png").read_bytes()
        for y in range(2)
        for x in range(2)
    }
    browser = _BrowserReplay(responses)
    source._browser_acquirer = browser

    try:
        with pytest.raises(TransportError, match="public network access is disabled"):
            await source.download(ref, context)
        assert browser.calls == []
    finally:
        context.close()


@pytest.mark.asyncio
async def test_windy_playwright_mode_enforces_total_temp_budget(tmp_path, fixture_root, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(WindySource, "frame_time", lambda self: valid_time)
    source = WindySource(registry.get_info("windy"))
    fixture = fixture_root / "windy/raw"
    payloads = {
        (fixture / f"tile-z1-x{x}-y{y}.png").read_bytes()
        for y in range(2)
        for x in range(2)
    }
    total_size = sum(map(len, payloads))
    context = SourceContext(
        load_config(
            {
                "runtime": {"allow_network": True, "max_temp_bytes": total_size - 1},
                "sources": {"windy": {"use_playwright": True}},
            },
            environ={},
        ),
        "windy",
        transport=_NoHttpTransport(),
        temp_root=tmp_path,
    )
    ref = (await source.discover(Query("windy", latest=True), context))[0]
    responses = {
        source.tile_url(valid_time, x, y, context): (fixture / f"tile-z1-x{x}-y{y}.png").read_bytes()
        for y in range(2)
        for x in range(2)
    }
    browser = _BrowserReplay(responses)
    source._browser_acquirer = browser

    try:
        with pytest.raises(ResourceLimitError, match="temporary data"):
            await source.download(ref, context)
        assert len(browser.calls) == 4
    finally:
        context.close()


@pytest.mark.asyncio
async def test_windy_playwright_rejects_non_256_square_tile(tmp_path, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(WindySource, "frame_time", lambda self: valid_time)
    source = WindySource(registry.get_info("windy"))
    context = SourceContext(
        load_config(
            {
                "runtime": {"allow_network": True},
                "sources": {"windy": {"use_playwright": True}},
            },
            environ={},
        ),
        "windy",
        transport=_NoHttpTransport(),
        temp_root=tmp_path,
    )
    ref = (await source.discover(Query("windy", latest=True), context))[0]
    responses = {
        source.tile_url(valid_time, x, y, context): _png(2)
        for y in range(2)
        for x in range(2)
    }
    source._browser_acquirer = _BrowserReplay(responses)
    try:
        with pytest.raises(IntegrityError, match="256x256"):
            await source.download(ref, context)
    finally:
        context.close()
