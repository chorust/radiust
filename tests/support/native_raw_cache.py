"""Seed the shared Python/Rust cache with a verified raw-frame manifest."""

from __future__ import annotations

import hashlib
import json
import sqlite3
from collections.abc import Iterable
from datetime import datetime, timezone
from pathlib import Path

from radiust.identity import cache_key
from radiust.models import FrameRef


def seed_native_cache_entry(
    cache_root: Path,
    key: str,
    payload: bytes,
    *,
    kind: str = "object",
) -> Path:
    """Write one Rust-readable entry in the legacy Python-compatible layout."""
    if kind not in {"object", "mosaic"}:
        raise ValueError("fixture cache kind must be object or mosaic")

    root = Path(cache_root)
    for name in ("objects", "mosaics", "tmp", "leases"):
        (root / name).mkdir(parents=True, exist_ok=True)

    directory = "mosaics" if kind == "mosaic" else "objects"
    path = root / directory / f"{hashlib.sha256(key.encode('utf-8')).hexdigest()}.bin"
    path.write_bytes(payload)
    now = datetime.now(timezone.utc).isoformat()
    with sqlite3.connect(root / "index.sqlite") as connection:
        connection.execute(
            """
            CREATE TABLE IF NOT EXISTS entries (
                key TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                path TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                validator TEXT,
                created_at TEXT NOT NULL,
                last_accessed_at TEXT NOT NULL,
                revalidated_at TEXT,
                expires_at TEXT
            )
            """
        )
        connection.execute(
            """
            INSERT INTO entries (
                key, kind, path, size_bytes, sha256, validator,
                created_at, last_accessed_at, revalidated_at, expires_at
            ) VALUES (?, ?, ?, ?, ?, NULL, ?, ?, ?, NULL)
            ON CONFLICT(key) DO UPDATE SET
                kind=excluded.kind,
                path=excluded.path,
                size_bytes=excluded.size_bytes,
                sha256=excluded.sha256,
                validator=NULL,
                created_at=excluded.created_at,
                last_accessed_at=excluded.last_accessed_at,
                revalidated_at=excluded.revalidated_at,
                expires_at=NULL
            """,
            (
                key,
                kind,
                str(path),
                len(payload),
                hashlib.sha256(payload).hexdigest(),
                now,
                now,
                now,
            ),
        )
    return path


def seed_native_raw_cache(
    cache_root: Path,
    ref: FrameRef,
    artifacts: Iterable[tuple[str, str, bytes]],
) -> dict[str, Path]:
    """Write Rust Engine raw-cache entries using the legacy compatible index."""
    frame_key = cache_key(ref, ref.revision)
    descriptors = []
    paths = {}
    for name, media_type, payload in artifacts:
        artifact_key = f"raw:{frame_key}:artifact:{name}"
        path = seed_native_cache_entry(cache_root, artifact_key, payload)
        descriptors.append(
            {
                "name": name,
                "media_type": media_type,
                "size_bytes": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        )
        paths[artifact_key] = path

    manifest_key = f"raw:{frame_key}:manifest"
    manifest = {
        "schema_version": 1,
        "logical_id": ref.logical_id,
        "revision": ref.revision,
        "ref": {},
        "artifacts": descriptors,
    }
    paths[manifest_key] = seed_native_cache_entry(
        cache_root,
        manifest_key,
        json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode("utf-8"),
    )
    return paths
