from __future__ import annotations

import json
from pathlib import Path

from click.testing import CliRunner
from radiust import Client
from radiust.cli.main import main

from tests.support.native_science_fixture import (
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


def test_offline_native_fetch_and_repeat_download(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, ref)
    output_root = tmp_path / "output"
    config = tw_offline_config(cache_root, output_root, tmp_path / "temp")
    with Client(config=config) as client:
        field = client.fetch(ref)
        first = client.download(ref, output=output_root)
        second = client.download(ref, output=output_root)
    assert field.shape == [881, 921]
    assert first.counts["written"] == 1
    assert second.counts["skipped"] == 1


def test_native_cli_json_is_one_object() -> None:
    result = CliRunner().invoke(main, ["list", "sources", "--json"])
    assert result.exit_code == 0, result.output

    payload = json.loads(result.output)
    assert payload["schema_version"] == 1
    assert payload["command"] == "list"
    assert len(payload["items"]) == 25
