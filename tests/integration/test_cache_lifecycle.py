from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
from contextlib import suppress
from datetime import datetime, timedelta, timezone
from threading import Event

import pytest
import radiust.cache as cache_module
from radiust.cache import CacheStore


def test_cache_put_get_reports_receipt_and_evicts_corruption(tmp_path):
    cache = CacheStore(tmp_path / "cache")
    entry = cache.put("frame-key", b"radar", kind="object", validator="v1")

    assert entry is not None
    assert entry.size_bytes == 5
    assert entry.read_bytes() == b"radar"
    assert cache.get("frame-key", validator="v1").sha256 == entry.sha256

    entry.path.write_bytes(b"changed")
    assert cache.get("frame-key") is None
    assert cache.status()["entries"] == 0


def test_cache_lease_protects_entry_during_gc(tmp_path):
    cache = CacheStore(tmp_path / "cache", max_bytes=1)
    cache.put("leased", b"123", kind="object")
    cache.put("other", b"456", kind="object")

    with cache.lease("leased"):
        report = cache.gc()
        assert "leased" not in report["removed"]
        assert cache.get("leased") is not None

    report = cache.gc()
    assert report["removed"]
    assert cache.status()["bytes"] <= 1


def test_cache_supports_multiple_read_leases(tmp_path):
    cache = CacheStore(tmp_path / "cache", max_bytes=0)
    cache.put("shared", b"123", kind="object")

    with cache.lease("shared"), cache.lease("shared"):
        report = cache.gc()
        assert "shared" not in report["removed"]
        assert cache.get("shared") is not None

    assert "shared" in cache.gc()["removed"]


def test_cache_gc_removes_expired_and_stale_tmp_without_touching_output(tmp_path):
    output = tmp_path / "output"
    output.mkdir()
    (output / "formal.raw").write_bytes(b"keep")
    cache = CacheStore(tmp_path / "cache", output_root=output)
    expired = datetime.now(timezone.utc) - timedelta(seconds=1)
    cache.put("expired", b"old", expires_at=expired)
    stale = cache.tmp_dir / "orphan.tmp"
    stale.write_bytes(b"tmp")

    report = cache.gc()

    assert "expired" in report["removed"]
    assert not stale.exists()
    assert (output / "formal.raw").read_bytes() == b"keep"


def test_cache_clear_is_dry_run_safe_and_no_cache_is_equivalent(tmp_path):
    cache = CacheStore(tmp_path / "cache")
    cache.put("one", b"1")
    preview = cache.clear(dry_run=True)
    assert preview["dry_run"] is True
    assert cache.get("one") is not None

    cleared = cache.clear()
    assert cleared["removed"] == ["one"]
    assert cache.get("one") is None

    disabled = CacheStore(tmp_path / "disabled", enabled=False)
    assert disabled.put("one", b"1") is None
    assert disabled.get("one") is None
    assert disabled.status()["enabled"] is False


def test_cache_startup_repairs_missing_index_rows_and_orphan_files(tmp_path):
    root = tmp_path / "cache"
    cache = CacheStore(root)
    cache.put("indexed", b"1")
    orphan = cache.objects_dir / "orphan.bin"
    orphan.write_bytes(b"orphan")
    indexed_path = cache.get("indexed").path
    indexed_path.unlink()

    repaired = CacheStore(root)

    assert repaired.get("indexed") is None
    assert not orphan.exists()


@pytest.mark.parametrize("maintenance", ["gc", "clear"])
def test_cache_maintenance_does_not_unlink_active_temporary_write(tmp_path, monkeypatch, maintenance):
    cache = CacheStore(tmp_path / "cache")
    replacing = Event()
    release = Event()
    original_replace = cache_module.os.replace

    def suspended_replace(source, target):
        if str(source).endswith(".tmp"):
            replacing.set()
            assert release.wait(timeout=5)
        return original_replace(source, target)

    monkeypatch.setattr(cache_module.os, "replace", suspended_replace)
    with ThreadPoolExecutor(max_workers=2) as executor:
        writer = executor.submit(cache.put, "active", b"intact")
        assert replacing.wait(timeout=5)
        cleaner = executor.submit(getattr(cache, maintenance))
        with suppress(TimeoutError):
            cleaner.result(timeout=0.1)
        release.set()
        entry = writer.result(timeout=5)
        assert entry is not None
        cleaner.result(timeout=5)
        if maintenance == "gc":
            assert entry.read_bytes() == b"intact"
