from datetime import datetime, timezone

from radiust import Client, Query
from radiust.config import load_config
from radiust.registry import get_source

QUERY = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))


def test_acquisition_reuses_verified_raw_cache(tmp_path, monkeypatch):
    config = load_config({"cache": {"dir": str(tmp_path / "cache")}, "storage": {"output": str(tmp_path / "output")}})
    source = get_source("my")
    original = source.download
    calls = 0

    async def counted_download(ref, context):
        nonlocal calls
        calls += 1
        return await original(ref, context)

    monkeypatch.setattr(source, "download", counted_download)
    with Client(config=config) as client:
        client.fetch(QUERY)
        client.fetch(QUERY)

    assert calls == 1
    assert config.cache_dir.joinpath("index.sqlite").exists()


def test_no_cache_keeps_acquisition_semantics(tmp_path, monkeypatch):
    config = load_config({"cache": {"dir": str(tmp_path / "cache"), "enabled": False}, "storage": {"output": str(tmp_path / "output")}})
    source = get_source("my")
    original = source.download
    calls = 0

    async def counted_download(ref, context):
        nonlocal calls
        calls += 1
        return await original(ref, context)

    monkeypatch.setattr(source, "download", counted_download)
    with Client(config=config) as client:
        client.fetch(QUERY)
        client.fetch(QUERY)

    assert calls == 2
