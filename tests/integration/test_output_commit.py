from __future__ import annotations

import hashlib
import json
from pathlib import Path

from radiust import Client
from radiust.models import FrameRef

from tests.support.native_science_fixture import (
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


def _offline_fixture_client(tmp_path: Path, *refs: FrameRef) -> Client:
    cache_root = tmp_path / "cache"
    for ref in refs:
        seed_tw_grid_cache(cache_root, ref)
    return Client(
        config=tw_offline_config(
            cache_root, tmp_path / "configured-output", tmp_path / "temp"
        )
    )


def _inspect_native_manifest(output_root: Path, manifest_path: Path) -> str:
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        for artifact in manifest["artifacts"]:
            payload = (output_root / artifact["relative_uri"]).read_bytes()
            if len(payload) != artifact["size_bytes"]:
                return "incomplete"
            if hashlib.sha256(payload).hexdigest() != artifact["sha256"]:
                return "incomplete"
    except (KeyError, OSError, TypeError, ValueError):
        return "incomplete"
    return "complete"


def test_png_commit_tracks_render_sidecar_and_is_idempotent(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    with _offline_fixture_client(tmp_path, ref) as client:
        first = client.download(ref, output=tmp_path / "output", format="png")
        second = client.download(ref, output=tmp_path / "output", format="png")
    assert first.counts["written"] == 1
    assert second.counts["skipped"] == 1
    image = next(tmp_path.rglob("*.png"))
    sidecar = image.with_name(image.stem + ".render.json")
    manifest = image.with_name(image.name + ".manifest.json")
    assert sidecar.exists()
    assert _inspect_native_manifest(tmp_path / "output", manifest) == "complete"
    names = {item["name"] for item in json.loads(manifest.read_text())["artifacts"]}
    assert image.name in names and sidecar.name in names


def test_template_conflict_does_not_replace_complete_output(tmp_path: Path) -> None:
    first_ref = tw_grid_ref()
    next_ref = tw_grid_ref(first_ref.valid_time.replace(minute=35))
    with _offline_fixture_client(tmp_path, first_ref, next_ref) as client:
        client.download(first_ref, output=tmp_path / "output", output_template="fixed.nc")
        report = client.download(
            next_ref, output=tmp_path / "output", output_template="fixed.nc"
        )
    assert report.counts["failed"] == 1
    assert report.items[0].error["code"] == "output_conflict"


def test_incomplete_group_is_repaired(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    output_root = tmp_path / "output"
    with _offline_fixture_client(tmp_path, ref) as client:
        first = client.download(ref, output=output_root)
        assert first.counts["written"] == 1
        nc = next(output_root.rglob("*.nc"))
        nc.unlink()
        repaired = client.download(ref, output=output_root)
    assert repaired.counts["written"] == 1
    assert next(output_root.rglob("*.nc")).exists()


def test_explicit_overwrite_records_a_new_generation(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    output_root = tmp_path / "output"
    with _offline_fixture_client(tmp_path, ref) as client:
        first = client.download(ref, output=output_root)
        manifest_path = next(output_root.rglob("*.manifest.json"))
        before = json.loads(manifest_path.read_text())
        second = client.download(ref, output=output_root, overwrite=True)
        after = json.loads(manifest_path.read_text())

    assert first.counts["written"] == 1
    assert second.counts["written"] == 1
    assert before["generation"]
    assert after["generation"]
    assert after["generation"] != before["generation"]
    assert after["supersedes"]["output_id"] == before["output_id"]
    assert after["supersedes"]["generation"] == before["generation"]


def test_local_manifest_detects_corruption_and_next_download_repairs(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    output_root = tmp_path / "output"
    with _offline_fixture_client(tmp_path, ref) as client:
        first = client.download(ref, output=output_root)
        output = Path(first.items[0].output_uri)
        manifest_path = output.with_name(output.name + ".manifest.json")
        output.write_bytes(output.read_bytes() + b"corruption")
        assert _inspect_native_manifest(output_root, manifest_path) == "incomplete"

        repaired = client.download(ref, output=output_root)

    assert repaired.counts["written"] == 1
    assert _inspect_native_manifest(output_root, manifest_path) == "complete"
