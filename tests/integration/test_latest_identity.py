import asyncio
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest
from click.testing import CliRunner
from radiust.cli.main import main
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import AmbiguousFrameError
from radiust.models import FrameRef, ProductInfo, Query, SourceInfo, StationInfo
from radiust.query import select_one
from radiust.sources.base import FixtureSource


def test_select_one_never_guesses_for_same_time_different_stations():
    at = datetime(2025, 1, 1, tzinfo=timezone.utc)
    frames = [FrameRef("th", "composite", at, station="a"), FrameRef("th", "composite", at, station="b")]
    try:
        select_one(frames, context="raw")
    except AmbiguousFrameError:
        pass
    else:
        raise AssertionError("ambiguous frame was selected")


def _offline_fixture(tmp_path, frames):
    path = tmp_path / "frames.json"
    path.write_text(json.dumps({"frames": frames}), encoding="utf-8")
    info = SourceInfo("fake", "offline", "1", (ProductInfo("rain", variables=("reflectivity",), default=True),),
                      (StationInfo("a", "A", 0, 0), StationInfo("b", "B", 1, 1)))
    return FixtureSource(info, path)


def _discover(source, query):
    context = SourceContext(load_config(environ={}), source.info.id)
    try:
        return asyncio.run(source.discover(query, context))
    finally:
        context.close()


def test_same_station_time_with_two_upstream_revisions_stays_ambiguous(tmp_path):
    time = "2026-09-22T00:00:00Z"
    source = _offline_fixture(tmp_path, [
        {"product": "rain", "station": "a", "valid_time": time, "revision": "r1"},
        {"product": "rain", "station": "a", "valid_time": time, "revision": "r2"},
    ])
    refs = _discover(source, Query("fake", latest=True))
    assert len(refs) == 2
    assert {ref.revision for ref in refs} == {"r1", "r2"}
    with pytest.raises(AmbiguousFrameError):
        select_one(refs)


def test_fixture_download_uses_exact_selected_revision(tmp_path):
    stamp = "2026-09-22T00:00:00Z"
    frames = []
    for revision in ("r1", "r2"):
        payload = tmp_path / f"{revision}.png"
        payload.write_bytes(revision.encode())
        frames.append({
            "product": "rain", "station": "a", "valid_time": stamp,
            "revision": revision,
            "artifacts": [{"name": f"{revision}.png", "path": str(payload)}],
        })
    source = _offline_fixture(tmp_path, frames)
    refs = _discover(source, Query("fake", latest=True))
    context = SourceContext(load_config(environ={}), source.info.id)
    try:
        for ref in refs:
            raw = asyncio.run(source.download(ref, context))
            try:
                assert raw.ref == ref
                assert raw.bytes() == ref.revision.encode()
            finally:
                raw.close()
    finally:
        context.close()


def test_latest_keeps_cross_station_same_named_frames_and_drops_expired(tmp_path):
    now = datetime.now(timezone.utc).replace(microsecond=0)
    old, fresh = (now - timedelta(minutes=10)).isoformat(), (now - timedelta(seconds=5)).isoformat()
    source = _offline_fixture(tmp_path, [
        {"product": "rain", "station": "a", "valid_time": old, "locator": {"file": "frame.png"}},
        {"product": "rain", "station": "a", "valid_time": fresh, "locator": {"file": "frame.png"}},
        {"product": "rain", "station": "b", "valid_time": fresh, "locator": {"file": "frame.png"}},
    ])
    refs = _discover(source, Query("fake", latest=True, max_age=timedelta(minutes=1)))
    assert len(refs) == 2
    assert {ref.station for ref in refs} == {"a", "b"}
    assert all(ref.valid_time == datetime.fromisoformat(fresh) for ref in refs)
    assert _discover(source, Query("fake", latest=True, max_age=timedelta(seconds=1))) == []


def test_explicit_single_source_timestamp_does_not_switch_to_latest(tmp_path):
    source = _offline_fixture(tmp_path, [
        {"product": "rain", "station": "a", "valid_time": stamp}
        for stamp in ("2026-09-20T00:00:00Z", "2026-09-22T00:00:00Z")
    ])
    at = datetime(2026, 9, 20, tzinfo=timezone.utc)
    refs = _discover(source, Query("fake", at=at, stations=("a",)))
    assert len(refs) == 1 and refs[0].valid_time == at


def test_legacy_adapter_keeps_equal_time_distinct_source_identities():
    from radiust.sources.legacy import LegacyImageSource

    stamp = datetime(2026, 9, 22, tzinfo=timezone.utc)
    previous = stamp - timedelta(hours=1)

    class OfflineLegacy(LegacyImageSource):
        async def discover_entries(self, _context):
            return [
                {"url": "https://example.invalid/a-old.png", "valid_time": previous, "station": "a"},
                {"url": "https://example.invalid/a-v1.png", "valid_time": stamp, "station": "a"},
                {"url": "https://example.invalid/a-v2.png", "valid_time": stamp, "station": "a"},
                {"url": "https://example.invalid/b.png", "valid_time": stamp, "station": "b"},
            ]

    source = OfflineLegacy(SourceInfo("fake", "offline", "1", (
        ProductInfo("rain", variables=("reflectivity",), default=True),
    )))
    latest = _discover(source, Query("fake", latest=True))
    assert len(latest) == 3
    assert [ref.station for ref in latest].count("a") == 2
    assert [ref.station for ref in latest].count("b") == 1
    assert len({(ref.logical_id, ref.revision) for ref in latest}) == 3
    with pytest.raises(AmbiguousFrameError):
        select_one(latest)
    older = _discover(source, Query("fake", at=previous))
    assert len(older) == 1 and older[0].valid_time == previous


def test_cat_rejects_same_station_latest_revision_collision(tmp_path, monkeypatch):
    from radiust.display.isolated import RawPreviewAmbiguousError
    from radiust.models import Query

    source = _offline_fixture(tmp_path, [
        {"product": "rain", "station": "a", "valid_time": "2026-09-22T00:00:00Z", "revision": revision}
        for revision in ("r1", "r2")
    ])
    refs = _discover(source, Query("fake", latest=True))

    def ambiguous(*_args, **_kwargs):
        raise RawPreviewAmbiguousError([
            {"product": ref.product, "station": ref.station,
             "valid_time": ref.valid_time.isoformat().replace("+00:00", "Z")}
            for ref in refs
        ])

    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", ambiguous)
    response = CliRunner().invoke(main, ["cat", "fake", "--raw", "--renderer", "text"])
    assert response.exit_code == 2
    assert "2 candidates" in response.output


def test_download_dry_run_reports_each_station_latest_frame(tmp_path, monkeypatch):
    from radiust.client import Client

    source = _offline_fixture(tmp_path, [
        {"product": "rain", "station": station, "valid_time": "2026-09-22T00:00:00Z"}
        for station in ("a", "b")
    ])
    monkeypatch.setattr(Client, "discover", lambda _client, query: _discover(source, query))
    response = CliRunner().invoke(main, ["download", "fake", "--dry-run", "--json"])
    assert response.exit_code == 0, response.output
    items = json.loads(response.output)["items"]
    assert len(items) == 2
    assert {item["station"] for item in items} == {"a", "b"}
    assert {item["status"] for item in items} == {"planned"}


def test_raw_only_download_reports_and_preserves_two_station_outputs(tmp_path, monkeypatch):
    from tests.support.cli_experience import image_bytes

    image = tmp_path / "frame.png"
    image.write_bytes(image_bytes())
    source = _offline_fixture(tmp_path, [
        {"product": "rain", "station": station, "valid_time": "2026-09-22T00:00:00Z",
         "revision": f"revision-{station}", "artifacts": [
             {"name": "frame.png", "path": str(image), "role": "data", "media_type": "image/png"}]}
        for station in ("a", "b")
    ])
    monkeypatch.setattr("radiust.pipeline.get_source", lambda _source_id: source)
    output = tmp_path / "out"
    response = CliRunner().invoke(main, ["download", "fake", "--raw-only", "--no-cache",
                                         "--output", str(output), "--json"])
    assert response.exit_code == 0, response.output
    items = json.loads(response.output)["items"]
    assert len(items) == 2
    assert {item["station"] for item in items} == {"a", "b"}
    assert {item["status"] for item in items} == {"written"}
    assert len({item["output_uri"] for item in items}) == 2
    # In raw-only mode output_uri names a logical output; the manifest is the
    # published commit marker and no scientific data file is produced.
    assert all(item["output_uri"] and Path(item["output_uri"] + ".manifest.json").exists() for item in items)
