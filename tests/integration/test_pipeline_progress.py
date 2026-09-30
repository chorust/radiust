"""Progress callbacks report Rust Engine work through the public SDK."""

from __future__ import annotations

import json
from pathlib import Path

from radiust import Client

from tests.support.native_science_fixture import (
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


def _seeded(tmp_path: Path):
    ref = tw_grid_ref()
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, ref)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")
    return ref, config


def test_download_progress_reflects_resolve_and_native_completion(tmp_path: Path) -> None:
    ref, config = _seeded(tmp_path)
    events = []
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
    assert ("resolve", 1, None) in events
    assert ("download", 0, 1) in events
    assert ("download", 1, 1) in events
    assert all(stage != "decode" for stage, _completed, _total in events)
    assert events.index(("download", 1, 1)) == len(events) - 1


def test_scientific_fetch_emits_native_acquire_and_decode_completion(
    tmp_path: Path,
) -> None:
    ref, config = _seeded(tmp_path)
    events = []
    with Client(config=config) as client:
        field = client.fetch(
            ref,
            progress=lambda stage, completed, total: events.append(
                (stage, completed, total)
            ),
        )
    assert field.shape == [881, 921]
    completed = [(stage, count) for stage, count, _ in events if count == 1]
    assert completed == [("acquire", 1), ("decode", 1)]


def test_sdk_calls_without_callbacks_emit_no_progress(tmp_path: Path, capsys) -> None:
    ref, config = _seeded(tmp_path)
    with Client(config=config) as client:
        report = client.download(ref, output=tmp_path / "output", raw_only=True)
    assert report.counts["written"] == 1
    captured = capsys.readouterr()
    assert captured.out == captured.err == ""


def test_batch_fetch_progress_counts_native_fetch_completions(tmp_path: Path) -> None:
    ref, config = _seeded(tmp_path)
    events = []
    with Client(config=config) as client:
        result = client.fetch_many(
            [ref],
            progress=lambda stage, completed, total: events.append(
                (stage, completed, total)
            ),
        )
    assert len(result.succeeded) == 1
    assert ("resolve", 1, None) in events
    assert ("fetch", 0, 1) in events
    assert ("fetch", 1, 1) in events


def test_batch_fetch_decodes_in_rust_and_preserves_input_order(
    tmp_path: Path, monkeypatch
) -> None:
    first = tw_grid_ref()
    second = tw_grid_ref(first.valid_time.replace(minute=35))
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, first)
    seed_tw_grid_cache(cache_root, second)
    config = tw_offline_config(cache_root, tmp_path / "output", tmp_path / "temp")

    with Client(config=config) as client:
        async def reject_python_decode(_raw):
            raise AssertionError("batch decode must remain in the Rust Engine")

        monkeypatch.setattr(client._session, "decode_science", reject_python_decode)
        report = client.fetch_many([second, first], max_concurrency=2)

    assert [item.ref.logical_id for item in report.items] == [
        second.logical_id,
        first.logical_id,
    ]
    assert [item.status for item in report.items] == ["success", "success"]
    assert [item.data.name for item in report.items] == ["reflectivity", "reflectivity"]
    assert all(item.data.shape == [881, 921] for item in report.items)
    assert [json.loads(item.data.to_json())["valid_time"] for item in report.items] == [
        second.valid_time.isoformat(),
        first.valid_time.isoformat(),
    ]


def test_native_cli_json_has_no_non_tty_progress_noise() -> None:
    from click.testing import CliRunner
    from radiust.cli.main import main

    result = CliRunner().invoke(main, ["list", "sources", "--json"])
    assert result.exit_code == 0, result.output
    assert json.loads(result.output)["schema_version"] == 1
    assert "progress" not in result.output.lower()
