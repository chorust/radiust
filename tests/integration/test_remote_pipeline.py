from __future__ import annotations

from pathlib import Path

from radiust import Client

from tests.support.native_science_fixture import (
    seed_tw_grid_cache,
    tw_grid_ref,
    tw_offline_config,
)


def test_remote_sdk_download_respects_disabled_network_policy(tmp_path: Path) -> None:
    ref = tw_grid_ref()
    cache_root = tmp_path / "cache"
    seed_tw_grid_cache(cache_root, ref)
    config = tw_offline_config(cache_root, tmp_path / "unused-local-output", tmp_path / "temp")

    with Client(config=config) as client:
        report = client.download(
            ref,
            output="s3://bucket/radar",
            raw_only=True,
        )

    assert report.counts["failed"] == 1
    assert report.items[0].error["code"] == "network_restricted"
    assert report.items[0].error["stage"] == "commit"
