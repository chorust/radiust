"""Rust-backed cache contracts that remain observable without provider I/O.

The Python Engine binding constructs its built-in Rust registry and exposes no
adapter-injection hook. A verified shared-cache fixture exercises real offline
hits and corruption handling without contacting a provider.
"""

from datetime import datetime, timezone
from pathlib import Path

import pytest
from radiust import Client, FrameRef
from radiust.config import load_config

from tests.support.native_raw_cache import seed_native_raw_cache


def _config(cache_dir, *, enabled):
    return load_config(
        {
            "runtime": {"allow_network": False},
            "cache": {"dir": str(cache_dir), "enabled": enabled},
            "storage": {"output": str(cache_dir.parent / "output")},
        },
        environ={},
    )


def _ref() -> FrameRef:
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
        revision="offline-cache-contract",
    )


def test_valid_raw_cache_hit_is_reused_offline_and_closed_with_acquire_context(tmp_path):
    cache_dir = tmp_path / "cache"
    config = _config(cache_dir, enabled=True)
    ref = _ref()
    payload = b"offline Rust cache artifact"
    seed_native_raw_cache(cache_dir, ref, [("my_east.png", "image/png", payload)])

    with Client(config=config) as client:
        with client.acquire(ref) as raw:
            stage_path = Path(raw.artifact_path(0))
            assert stage_path.is_file()
            assert raw.artifact_count == 1
            assert raw.artifact_bytes(0) == payload
        assert not stage_path.exists()
        with pytest.raises(RuntimeError, match="RawFrame is closed"):
            raw.artifact_bytes(0)


def test_corrupt_raw_cache_entry_fails_closed_to_offline_miss(tmp_path):
    cache_dir = tmp_path / "cache"
    config = _config(cache_dir, enabled=True)
    ref = _ref()
    paths = seed_native_raw_cache(
        cache_dir, ref, [("my_east.png", "image/png", b"valid cached payload")]
    )
    artifact_key = next(key for key in paths if ":artifact:" in key)
    paths[artifact_key].write_bytes(b"tampered cached payload")

    with (
        Client(config=config) as client,
        pytest.raises(PermissionError, match="network access"),
        client.acquire(ref),
    ):
        pytest.fail("a digest-invalid cache entry must not produce a RawFrame")


def test_disabled_rust_cache_leaves_offline_acquisition_without_cache_files(tmp_path):
    cache_dir = tmp_path / "cache"
    config = _config(cache_dir, enabled=False)
    ref = _ref()

    with Client(config=config) as client:
        for _ in range(2):
            with (
                pytest.raises(PermissionError, match="network access"),
                client.acquire(ref),
            ):
                pytest.fail("network-disabled acquisition must not yield a raw frame")

    assert not cache_dir.exists()
