from __future__ import annotations

import asyncio
import hashlib
import json
from dataclasses import replace
from datetime import datetime, timezone
from pathlib import Path

from click.testing import CliRunner
from radiust import Client, FrameRef
from radiust._bridge import CoreEngineSession
from radiust.cli.main import main
from radiust.identity import cache_key

from tests.support.native_raw_cache import (
    seed_native_cache_entry,
    seed_native_raw_cache,
)
from tests.support.native_science_fixture import (
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


def _seeded_client_config(tmp_path: Path, ref):
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, ref)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")
    return cache_root, config


def _replay_raw_manifest(manifest_path: Path, temp_root: Path):
    session = CoreEngineSession(
        {
            "runtime": {"allow_network": False, "temp_root": str(temp_root)},
            "cache": {"enabled": False},
        }
    )
    return asyncio.run(session.replay_raw_manifest(manifest_path))


def _metadata_time(metadata_json: str) -> datetime:
    valid_time = json.loads(metadata_json)["valid_time"]
    return datetime.fromisoformat(valid_time.replace("Z", "+00:00"))


def _revision_ref(revision: str) -> FrameRef:
    return FrameRef(
        "my",
        "composite",
        datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc),
        station="east",
        locator={
            "url": "https://www.met.gov.my/data/radar_east.gif",
            "artifacts": [],
            "station": "east",
            "name": "my_east.png",
            "media_type": "image/png",
        },
        locator_version="my-legacy-v1",
        revision=revision,
    )


def test_raw_only_skips_decode_and_can_be_replayed(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    _cache_root, config = _seeded_client_config(tmp_path, ref)
    events: list[tuple[str, int, int | None]] = []
    with Client(config=config) as client:
        report = client.download(
            ref,
            output=tmp_path / "output",
            raw_only=True,
            progress=lambda stage, completed, total: events.append(
                (stage, completed, total)
            ),
        )
    assert report.counts["written"] == 1
    assert not list((tmp_path / "output").rglob("*.nc"))
    raw_manifests = list((tmp_path / "output").rglob("raw-manifest.json"))
    assert len(raw_manifests) == 1
    assert "decode" not in {stage for stage, _completed, _total in events}
    field = _replay_raw_manifest(raw_manifests[0], tmp_path / "replay-temp")
    assert field.shape == [881, 921]
    assert _metadata_time(field.metadata_json()) == ref.valid_time


def test_decoded_output_can_be_completed_with_raw(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    _cache_root, config = _seeded_client_config(tmp_path, ref)
    with Client(config=config) as client:
        first = client.download(ref, output=tmp_path / "output")
        manifest = next((tmp_path / "output").rglob("*.manifest.json"))
        first_manifest = json.loads(manifest.read_text())
        second = client.download(ref, output=tmp_path / "output", raw=True)
    assert first.counts["written"] == 1
    assert second.counts["written"] == 1
    second_manifest = json.loads(manifest.read_text())
    assert second_manifest["raw_complete"] is True
    assert second_manifest["revision"] == first_manifest["revision"]
    assert second_manifest["output_id"] == first_manifest["output_id"]
    assert first.items[0].output_uri == second.items[0].output_uri


def test_raw_cache_is_revision_pinned_and_never_uses_mosaic_entries(
    tmp_path: Path,
) -> None:
    first_ref = _revision_ref("revision-one")
    next_ref = replace(first_ref, revision="revision-two")
    assert cache_key(first_ref, first_ref.revision) != cache_key(
        next_ref, next_ref.revision
    )
    cache_root = tmp_path / "cache"
    seed_native_raw_cache(
        cache_root, first_ref, [("my_east.png", "image/png", b"revision one")]
    )
    seed_native_cache_entry(
        cache_root,
        cache_key(next_ref, next_ref.revision),
        b"cached mosaic only",
        kind="mosaic",
    )
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")

    with Client(config=config) as client:
        miss = client.download(
            next_ref, output=tmp_path / "miss", raw_only=True
        )
    assert miss.counts["failed"] == 1

    next_payload = b"revision two"
    seed_native_raw_cache(
        cache_root, next_ref, [("my_east.png", "image/png", next_payload)]
    )
    with Client(config=config) as client:
        hit = client.download(next_ref, output=tmp_path / "hit", raw_only=True)
    assert hit.counts["written"] == 1
    manifest = json.loads(
        next((tmp_path / "hit").rglob("raw-manifest.json")).read_text()
    )
    assert manifest["artifacts"][0]["sha256"] == hashlib.sha256(
        next_payload
    ).hexdigest()


def test_clearing_cache_does_not_remove_formal_raw_artifacts(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    cache_dir, config = _seeded_client_config(tmp_path, ref)
    output_dir = tmp_path / "output"
    with Client(config=config) as client:
        report = client.download(ref, output=output_dir, raw=True)

    assert report.counts["written"] == 1
    raw_path = next(output_dir.rglob("raw-manifest.json"))
    before = _replay_raw_manifest(raw_path, tmp_path / "replay-temp")

    clear = CliRunner().invoke(
        main,
        ["cache", "clear", "--cache-dir", str(cache_dir), "--yes", "--json"],
    )
    assert clear.exit_code == 0, clear.output
    clear_report = json.loads(clear.output)
    assert clear_report["operation"] == "clear"
    assert clear_report["removed"]

    after = _replay_raw_manifest(raw_path, tmp_path / "replay-temp")
    assert after.shape == before.shape
    assert after.values_le_bytes() == before.values_le_bytes()
