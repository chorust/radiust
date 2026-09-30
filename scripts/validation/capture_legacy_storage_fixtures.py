"""Create deterministic legacy v1 output and Python cache fixtures for Rust migration."""

from __future__ import annotations

import hashlib
import json
import shutil
import sqlite3
from datetime import datetime, timezone
from pathlib import Path

from radiust.cache import CacheStore
from radiust.storage.manifest import inspect_manifest, load_manifest

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/rust-migration"
OUTPUT = FIXTURES / "output"
CACHE = FIXTURES / "cache/legacy-v1"
KEY = "legacy-frame-key"
PAYLOAD = b"legacy cached radar frame\n"


def _write_output_readback() -> None:
    legacy = OUTPUT / "legacy-v1"
    manifest = json.loads((legacy / "frame.bin.manifest.json").read_text(encoding="utf-8"))

    damaged = OUTPUT / "damaged-v1"
    shutil.rmtree(damaged, ignore_errors=True)
    damaged.mkdir(parents=True)
    (damaged / "frame.bin").write_bytes(b"damaged legacy output\n")
    shutil.copy2(legacy / "frame.bin.manifest.json", damaged / "frame.bin.manifest.json")

    conflict = OUTPUT / "conflict-v1"
    shutil.rmtree(conflict, ignore_errors=True)
    conflict.mkdir(parents=True)
    shutil.copy2(legacy / "frame.bin", conflict / "frame.bin")
    conflict_manifest = dict(manifest)
    conflict_manifest["logical_id"] = "b" * 64
    conflict_manifest["output_id"] = "c" * 64
    (conflict / "frame.bin.manifest.json").write_text(
        json.dumps(conflict_manifest, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )

    paths = {
        "legal_v1": legacy / "frame.bin.manifest.json",
        "damaged_v1": damaged / "frame.bin.manifest.json",
        "conflict_v1": conflict / "frame.bin.manifest.json",
    }
    report = {
        "reader": "python/radiust/storage/manifest.py",
        "results": {
            "legal_v1": {
                "status": inspect_manifest(paths["legal_v1"]),
                "logical_id": load_manifest(paths["legal_v1"]).logical_id,
                "output_id": load_manifest(paths["legal_v1"]).output_id,
            },
            "damaged_v1": {"status": inspect_manifest(paths["damaged_v1"])},
            "conflict_v1": {
                "status": inspect_manifest(paths["conflict_v1"]),
                "logical_id": load_manifest(paths["conflict_v1"]).logical_id,
                "output_id": load_manifest(paths["conflict_v1"]).output_id,
                "note": "artifact integrity is valid; identity differs from the legal fixture",
            },
        },
    }
    (OUTPUT / "readback-python.json").write_text(
        json.dumps(report, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )


def _write_cache_fixture() -> None:
    shutil.rmtree(CACHE, ignore_errors=True)
    store = CacheStore(CACHE)
    expiry = datetime(2030, 1, 1, tzinfo=timezone.utc)
    entry = store.put(KEY, PAYLOAD, kind="object", validator='"fixture-etag"', expires_at=expiry)
    assert entry is not None
    digest = hashlib.sha256(PAYLOAD).hexdigest()
    object_name = hashlib.sha256(KEY.encode("utf-8")).hexdigest()
    relative_object = f"objects/{object_name}.bin"
    object_path = f"/FIXTURE_ROOT/{relative_object}"
    with sqlite3.connect(CACHE / "index.sqlite") as connection:
        connection.execute(
            "UPDATE entries SET path=?,created_at=?,last_accessed_at=?,revalidated_at=NULL,expires_at=? WHERE key=?",
            (object_path, "2025-01-01T00:00:00+00:00", "2025-01-01T00:00:00+00:00", expiry.isoformat(), KEY),
        )
    (CACHE / ".maintenance.lock").unlink(missing_ok=True)
    report = {
        "reader": "python/radiust/cache.py",
        "key": KEY,
        "kind": entry.kind,
        "relative_object": relative_object,
        "size_bytes": entry.size_bytes,
        "sha256": digest,
        "validator": entry.validator,
        "expires_at": expiry.isoformat(),
        "bytes_hex": PAYLOAD.hex(),
    }
    (CACHE / "readback-python.json").write_text(
        json.dumps(report, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )


def main() -> None:
    _write_output_readback()
    _write_cache_fixture()


if __name__ == "__main__":
    main()
