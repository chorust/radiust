from __future__ import annotations

from datetime import datetime, timedelta, timezone
from hashlib import sha256
from importlib import import_module
from io import BytesIO
from pathlib import Path

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, IntegrityError, NoDataError, UnsupportedQueryError
from radiust.identity import artifact_bytes
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.th import ThSource

CMP = "https://weather.tmd.go.th/cmp/cmp1.gif"
KKN = "https://weather.tmd.go.th/kkn/kkn240Loop.gif"


class ReplayTransport:
    def __init__(self, responses):
        self.responses = responses
        self.calls = []

    async def get(self, url, *, headers=None):
        self.calls.append((url, headers))
        return self.responses[url]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("station", "expected_url", "fixture", "expected_time"),
    [
        ("cmp1", CMP, "cmp1.gif", datetime(2023, 4, 22, 14, 45, tzinfo=timezone.utc)),
        ("kkn240Loop", KKN, "kkn240Loop.gif", datetime(2023, 4, 22, 16, 0, 5, tzinfo=timezone.utc)),
    ],
)
async def test_th_latest_discovery_keeps_each_live_endpoint_explicit(
    tmp_path, station, expected_url, fixture, expected_time
):
    raw_root = Path(__file__).parents[1] / "fixtures/sources/th/raw"
    payload = (raw_root / fixture).read_bytes()
    transport = ReplayTransport({expected_url: payload})
    source = ThSource(registry.get_info("th"))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "th",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        refs = await source.discover(Query("th", latest=True, stations=(station,)), context)
        assert len(refs) == 1
        assert refs[0].station == station
        assert refs[0].locator["url"] == expected_url
        assert refs[0].valid_time == expected_time
        assert refs[0].metadata["time_semantics"] == "rendered_footer_ocr_utc"
        assert refs[0].metadata["geometry_status"] == "unverified"
        assert refs[0].locator["payload_sha256"] == sha256(payload).hexdigest()
        assert len(transport.calls) == 1

        start = refs[0].valid_time
        with pytest.raises(UnsupportedQueryError):
            await source.discover(
                Query("th", start=start, end=start + timedelta(seconds=1), stations=(station,)),
                context,
            )
    finally:
        context.close()


@pytest.mark.asyncio
async def test_th_live_sources_are_latest_only_and_preserve_raw(tmp_path):
    image = (Path(__file__).parents[1] / "fixtures/sources/th/raw/cmp1.gif").read_bytes()
    source = ThSource(registry.get_info("th"))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "th",
        transport=ReplayTransport({CMP: image}),
        temp_root=tmp_path,
    )
    try:
        refs = await source.discover(Query("th", latest=True, stations=("cmp1",)), context)
        assert len(refs) == 1 and refs[0].station == "cmp1"
        assert refs[0].valid_time == datetime(2023, 4, 22, 14, 45, tzinfo=timezone.utc)
        assert refs[0].metadata["time_semantics"] == "rendered_footer_ocr_utc"
        assert len(context.transport.calls) == 1
        raw = await source.download(refs[0], context)
        try:
            assert len(context.transport.calls) == 1, (
                "download must consume the exact bytes whose footer established valid_time"
            )
            assert (
                raw.artifacts[0].sha256
                == "95f482870e0d6cf2ed2d360dbf12101f39d456b810891bc0fe03c81a1c2a3e13"
            )
            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
        with pytest.raises(UnsupportedQueryError):
            await source.discover(
                Query("th", start=refs[0].valid_time, end=refs[0].valid_time.replace(year=2027)),
                context,
            )
    finally:
        context.close()


def test_th_kkn_fixture_is_the_full_animated_gif():
    payload = (Path(__file__).parents[1] / "fixtures/sources/th/raw/kkn240Loop.gif").read_bytes()
    assert (
        sha256(payload).hexdigest()
        == "98b6e087ddf5ff01e139f862409eae65c288d47e44603d67e3e0e224b49cb5cd"
    )
    with Image.open(BytesIO(payload)) as image:
        assert image.format == "GIF"
        assert image.size == (680, 680)
        assert image.n_frames == 6


def test_th_catalog_resource_declares_footer_bound_utc_time():
    import json

    resource = json.loads(
        (
            Path(__file__).parents[2]
            / "python/radiust/resources/sources/th.json"
        ).read_text()
    )
    assert resource["time_semantics"] == "rendered_footer_ocr_utc"
    assert resource["timestamp_extractor"] == "system_tesseract"


@pytest.mark.asyncio
async def test_th_kkn_live_loop_replay_preserves_all_six_frames_and_fails_closed(tmp_path):
    payload = (Path(__file__).parents[1] / "fixtures/sources/th/raw/kkn240Loop.gif").read_bytes()
    source = ThSource(registry.get_info("th"))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "th",
        transport=ReplayTransport({KKN: payload}),
        temp_root=tmp_path,
    )
    try:
        refs = await source.discover(Query("th", latest=True, stations=("kkn240Loop",)), context)
        assert len(refs) == 1
        assert refs[0].station == "kkn240Loop"
        assert refs[0].valid_time == datetime(2023, 4, 22, 16, 0, 5, tzinfo=timezone.utc)
        assert refs[0].metadata["time_semantics"] == "rendered_footer_ocr_utc"
        assert refs[0].metadata["footer_observation_times"] == (
            "2023-04-22T14:45:05Z",
            "2023-04-22T15:00:05Z",
            "2023-04-22T15:15:05Z",
            "2023-04-22T15:30:05Z",
            "2023-04-22T15:45:05Z",
            "2023-04-22T16:00:05Z",
        )
        raw = await source.download(refs[0], context)
        try:
            artifact = raw.artifacts[0]
            assert artifact.media_type == "image/gif"
            assert len(context.transport.calls) == 1
            assert artifact.sha256 == sha256(payload).hexdigest()
            assert artifact_bytes(artifact) == payload
            with Image.open(BytesIO(artifact_bytes(artifact))) as image:
                assert image.n_frames == 6
                durations = []
                for frame_index in range(image.n_frames):
                    image.seek(frame_index)
                    durations.append(image.info.get("duration"))
                assert durations == [750, 750, 750, 750, 750, 1500]
            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_th_max_age_uses_rendered_observation_time_not_retrieval_time(tmp_path):
    payload = (Path(__file__).parents[1] / "fixtures/sources/th/raw/cmp1.gif").read_bytes()
    source = ThSource(registry.get_info("th"))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "th",
        transport=ReplayTransport({CMP: payload}),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(NoDataError, match="no matching frame"):
            await source.discover(
                Query("th", latest=True, stations=("cmp1",), max_age=timedelta(days=1)),
                context,
            )
    finally:
        context.close()


@pytest.mark.asyncio
async def test_th_timestamp_binding_fails_closed_without_tesseract(tmp_path, monkeypatch):
    payload = (Path(__file__).parents[1] / "fixtures/sources/th/raw/cmp1.gif").read_bytes()
    source = ThSource(registry.get_info("th"))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "th",
        transport=ReplayTransport({CMP: payload}),
        temp_root=tmp_path,
    )
    timestamp_module = import_module("radiust.sources._th_time")
    monkeypatch.setattr(timestamp_module.shutil, "which", lambda _: None)
    try:
        with pytest.raises(DecodeError, match="Tesseract is required"):
            await source.discover(Query("th", latest=True, stations=("cmp1",)), context)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_th_acquisition_rejects_gif_changed_after_timestamp_discovery(tmp_path):
    raw_root = Path(__file__).parents[1] / "fixtures/sources/th/raw"
    discovered_payload = (raw_root / "cmp1.gif").read_bytes()
    changed_payload = (raw_root / "kkn240Loop.gif").read_bytes()
    source = ThSource(registry.get_info("th"))
    discovery_context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "th",
        transport=ReplayTransport({CMP: discovered_payload}),
        temp_root=tmp_path / "discovery",
    )
    try:
        ref = (
            await source.discover(Query("th", latest=True, stations=("cmp1",)), discovery_context)
        )[0]
    finally:
        discovery_context.close()

    acquisition_context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "th",
        transport=ReplayTransport({CMP: changed_payload}),
        temp_root=tmp_path / "acquisition",
    )
    try:
        with pytest.raises(IntegrityError, match="changed since timestamp discovery"):
            await source.download(ref, acquisition_context)
    finally:
        acquisition_context.close()
