"""Content addressed acquisition cache with explicit ownership boundaries."""

from __future__ import annotations

import fcntl
import hashlib
import os
import sqlite3
import uuid
from contextlib import contextmanager, suppress
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from .errors import ConfigError, IntegrityError, StorageError

UTC = timezone.utc


def _stamp(value: datetime) -> str:
    if value.tzinfo is None or value.utcoffset() is None:
        raise ValueError("cache timestamps require timezone-aware datetimes")
    return value.astimezone(UTC).isoformat()


def _parse_stamp(value: str | None) -> datetime | None:
    if value is None:
        return None
    return datetime.fromisoformat(value).astimezone(UTC)


def _digest_key(key: str) -> str:
    if not isinstance(key, str) or not key or "/" in key or "\\" in key or "\x00" in key:
        raise ValueError("cache key must be a non-empty path-safe string")
    return hashlib.sha256(key.encode("utf-8")).hexdigest()


@dataclass(frozen=True, slots=True)
class CacheEntry:
    key: str
    kind: str
    path: Path
    size_bytes: int
    sha256: str
    validator: str | None
    created_at: datetime
    last_accessed_at: datetime
    revalidated_at: datetime | None
    expires_at: datetime | None

    def read_bytes(self) -> bytes:
        return self.path.read_bytes()


class CacheLease:
    def __init__(self, path: Path | None):
        self.path = path
        self._held = False

    def __enter__(self) -> CacheLease:
        if self.path is None:
            self._held = True
            return self
        try:
            descriptor = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        except FileExistsError as exc:
            raise StorageError(f"cache key is already leased: {self.path.stem}") from exc
        try:
            os.write(descriptor, str(os.getpid()).encode("ascii"))
        finally:
            os.close(descriptor)
        self._held = True
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self._held and self.path is not None:
            with suppress(FileNotFoundError):
                self.path.unlink()
        self._held = False


class CacheStore:
    """Filesystem cache whose GC only owns paths below ``root``."""

    def __init__(
        self,
        root: str | Path,
        *,
        output_root: str | Path | None = None,
        max_bytes: int = 20_000_000_000,
        max_age_days: int = 30,
        enabled: bool = True,
    ) -> None:
        self.root = Path(root).expanduser().resolve()
        self.output_root = Path(output_root).expanduser().resolve() if output_root is not None else None
        if self.output_root is not None and (
            self.root == self.output_root
            or self.root in self.output_root.parents
            or self.output_root in self.root.parents
        ):
            raise ConfigError("cache directory and output root cannot overlap")
        if max_bytes < 0 or max_age_days <= 0:
            raise ValueError("cache limits must be non-negative and max_age_days must be positive")
        self.max_bytes = int(max_bytes)
        self.max_age_days = int(max_age_days)
        self.enabled = bool(enabled)
        self.objects_dir = self.root / "objects"
        self.mosaics_dir = self.root / "mosaics"
        self.tmp_dir = self.root / "tmp"
        self.leases_dir = self.root / "leases"
        self.db_path = self.root / "index.sqlite"
        self.maintenance_lock_path = self.root / ".maintenance.lock"
        if self.enabled:
            for path in (self.objects_dir, self.mosaics_dir, self.tmp_dir, self.leases_dir):
                path.mkdir(parents=True, exist_ok=True)
            with self._maintenance_lock():
                try:
                    self._init_db()
                except sqlite3.DatabaseError:
                    corrupt = self.db_path.with_name(f"{self.db_path.name}.corrupt.{uuid.uuid4().hex}")
                    with suppress(OSError):
                        os.replace(self.db_path, corrupt)
                    self._init_db()
                self._repair()

    @contextmanager
    def _maintenance_lock(self, *, shared: bool = False):
        """Coordinate live writers and maintenance across processes on the same filesystem."""
        with self.maintenance_lock_path.open("a+b") as handle:
            fcntl.flock(handle.fileno(), fcntl.LOCK_SH if shared else fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)

    def _connect(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.db_path)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA busy_timeout=5000")
        return connection

    def _init_db(self) -> None:
        with self._connect() as connection:
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

    def _repair(self) -> None:
        indexed: set[Path] = set()
        with self._connect() as connection:
            rows = connection.execute("SELECT key,path FROM entries").fetchall()
            for row in rows:
                path = Path(str(row["path"])).resolve()
                if self.root not in path.parents or not path.is_file():
                    connection.execute("DELETE FROM entries WHERE key=?", (str(row["key"]),))
                else:
                    indexed.add(path)
        for directory in (self.objects_dir, self.mosaics_dir):
            for path in directory.rglob("*"):
                if path.is_file() and path.resolve() not in indexed:
                    with suppress(OSError):
                        path.unlink()

    def _row(self, row: sqlite3.Row) -> CacheEntry:
        path = Path(str(row["path"])).resolve()
        if self.root not in path.parents:
            raise IntegrityError("cache index points outside cache root")
        return CacheEntry(
            key=str(row["key"]),
            kind=str(row["kind"]),
            path=path,
            size_bytes=int(row["size_bytes"]),
            sha256=str(row["sha256"]),
            validator=row["validator"],
            created_at=_parse_stamp(str(row["created_at"])) or datetime.now(UTC),
            last_accessed_at=_parse_stamp(str(row["last_accessed_at"])) or datetime.now(UTC),
            revalidated_at=_parse_stamp(row["revalidated_at"]),
            expires_at=_parse_stamp(row["expires_at"]),
        )

    def _path_for(self, key: str, kind: str) -> Path:
        if kind not in {"object", "mosaic"}:
            raise ValueError("cache kind must be object or mosaic")
        directory = self.objects_dir if kind == "object" else self.mosaics_dir
        return directory / f"{_digest_key(key)}.bin"

    def put(
        self,
        key: str,
        payload: bytes | bytearray | memoryview | str | Path,
        *,
        kind: str = "object",
        validator: str | None = None,
        expires_at: datetime | None = None,
    ) -> CacheEntry | None:
        if not self.enabled:
            return None
        if expires_at is not None:
            _stamp(expires_at)
        with self._maintenance_lock(shared=True):
            return self._put_locked(key, payload, kind=kind, validator=validator, expires_at=expires_at)

    def _put_locked(
        self,
        key: str,
        payload: bytes | bytearray | memoryview | str | Path,
        *,
        kind: str,
        validator: str | None,
        expires_at: datetime | None,
    ) -> CacheEntry:
        target = self._path_for(key, kind)
        temporary = self.tmp_dir / f".{target.stem}.{uuid.uuid4().hex}.tmp"
        digest = hashlib.sha256()
        size = 0
        try:
            with temporary.open("wb") as destination:
                if isinstance(payload, (bytes, bytearray, memoryview)):
                    chunks = (bytes(payload),)
                else:
                    with Path(payload).open("rb") as source:
                        chunks = iter(lambda: source.read(1024 * 1024), b"")
                        for chunk in chunks:
                            destination.write(chunk)
                            digest.update(chunk)
                            size += len(chunk)
                        chunks = ()
                for chunk in chunks:
                    destination.write(chunk)
                    digest.update(chunk)
                    size += len(chunk)
                destination.flush()
                os.fsync(destination.fileno())
            os.replace(temporary, target)
        except Exception:
            with suppress(FileNotFoundError):
                temporary.unlink()
            raise
        now = datetime.now(UTC)
        entry = CacheEntry(key, kind, target.resolve(), size, digest.hexdigest(), validator, now, now, now, expires_at)
        with self._connect() as connection:
            old = connection.execute("SELECT path FROM entries WHERE key=?", (key,)).fetchone()
            connection.execute(
                """
                INSERT INTO entries
                (key,kind,path,size_bytes,sha256,validator,created_at,last_accessed_at,revalidated_at,expires_at)
                VALUES (?,?,?,?,?,?,?,?,?,?)
                ON CONFLICT(key) DO UPDATE SET
                    kind=excluded.kind,path=excluded.path,size_bytes=excluded.size_bytes,
                    sha256=excluded.sha256,validator=excluded.validator,created_at=excluded.created_at,
                    last_accessed_at=excluded.last_accessed_at,revalidated_at=excluded.revalidated_at,
                    expires_at=excluded.expires_at
                """,
                (key, kind, str(target), size, entry.sha256, validator, _stamp(now), _stamp(now), _stamp(now), _stamp(expires_at) if expires_at else None),
            )
        if old is not None and Path(str(old[0])).resolve() != target.resolve():
            with suppress(OSError):
                Path(str(old[0])).unlink()
        return entry

    def _delete(self, entry: CacheEntry) -> None:
        with suppress(OSError):
            entry.path.unlink()
        with self._connect() as connection:
            connection.execute("DELETE FROM entries WHERE key=?", (entry.key,))

    def get(self, key: str, *, validator: str | None = None) -> CacheEntry | None:
        if not self.enabled:
            return None
        _digest_key(key)
        with self._connect() as connection:
            row = connection.execute("SELECT * FROM entries WHERE key=?", (key,)).fetchone()
        if row is None:
            return None
        entry = self._row(row)
        now = datetime.now(UTC)
        if (validator is not None and entry.validator != validator) or (entry.expires_at is not None and entry.expires_at <= now):
            self._delete(entry)
            return None
        try:
            if entry.path.stat().st_size != entry.size_bytes:
                raise ValueError("size mismatch")
            digest = hashlib.sha256(entry.path.read_bytes()).hexdigest()
            if digest != entry.sha256:
                raise ValueError("hash mismatch")
        except (OSError, ValueError):
            self._delete(entry)
            return None
        with self._connect() as connection:
            connection.execute("UPDATE entries SET last_accessed_at=? WHERE key=?", (_stamp(now), key))
        return CacheEntry(entry.key, entry.kind, entry.path, entry.size_bytes, entry.sha256, entry.validator, entry.created_at, now, entry.revalidated_at, entry.expires_at)

    def lease(self, key: str) -> CacheLease:
        if not self.enabled:
            return CacheLease(None)
        return CacheLease(self.leases_dir / f"{_digest_key(key)}.{uuid.uuid4().hex}.lease")

    def _entries(self) -> list[CacheEntry]:
        if not self.enabled:
            return []
        with self._connect() as connection:
            rows = connection.execute("SELECT * FROM entries").fetchall()
        return [self._row(row) for row in rows]

    def _is_leased(self, key: str) -> bool:
        digest = _digest_key(key)
        if (self.leases_dir / f"{digest}.lease").exists():
            return True
        return any(path.is_file() for path in self.leases_dir.glob(f"{digest}.*.lease"))

    def gc(self, *, dry_run: bool = False, now: datetime | None = None) -> dict[str, Any]:
        if not self.enabled:
            return {"schema_version": 1, "dry_run": dry_run, "removed": [], "stale_tmp": [], "bytes_before": 0, "bytes_after": 0}
        with self._maintenance_lock():
            return self._gc_locked(dry_run=dry_run, now=now)

    def _gc_locked(self, *, dry_run: bool, now: datetime | None) -> dict[str, Any]:
        now = now or datetime.now(UTC)
        entries = self._entries()
        candidates: list[CacheEntry] = []
        max_age = now - timedelta(days=self.max_age_days)
        for entry in entries:
            if (entry.expires_at is not None and entry.expires_at <= now) or entry.created_at < max_age:
                candidates.append(entry)
        total_before = sum(entry.size_bytes for entry in entries)
        remaining = [entry for entry in entries if entry not in candidates]
        total = sum(entry.size_bytes for entry in remaining)
        for entry in sorted(remaining, key=lambda value: value.last_accessed_at):
            if total <= self.max_bytes:
                break
            if self._is_leased(entry.key):
                continue
            candidates.append(entry)
            total -= entry.size_bytes
        candidates = [entry for entry in candidates if not self._is_leased(entry.key)]
        stale_tmp = [path for path in self.tmp_dir.iterdir() if path.is_file()]
        if not dry_run:
            for entry in candidates:
                self._delete(entry)
            for path in stale_tmp:
                with suppress(OSError):
                    path.unlink()
        total_after = total_before - sum(entry.size_bytes for entry in candidates)
        return {
            "schema_version": 1,
            "dry_run": dry_run,
            "removed": [entry.key for entry in candidates],
            "stale_tmp": [path.name for path in stale_tmp],
            "bytes_before": total_before,
            "bytes_after": max(0, total_after),
        }

    def clear(self, *, dry_run: bool = False) -> dict[str, Any]:
        if not self.enabled:
            return {"schema_version": 1, "dry_run": dry_run, "removed": []}
        with self._maintenance_lock():
            return self._clear_locked(dry_run=dry_run)

    def _clear_locked(self, *, dry_run: bool) -> dict[str, Any]:
        entries = [entry for entry in self._entries() if not self._is_leased(entry.key)]
        removed = [entry.key for entry in entries]
        if not dry_run:
            for entry in entries:
                self._delete(entry)
            for path in self.tmp_dir.iterdir():
                if path.is_file():
                    with suppress(OSError):
                        path.unlink()
        return {"schema_version": 1, "dry_run": dry_run, "removed": removed}

    def status(self) -> dict[str, Any]:
        if not self.enabled:
            return {"schema_version": 1, "enabled": False, "root": str(self.root), "entries": 0, "bytes": 0}
        entries = self._entries()
        return {
            "schema_version": 1,
            "enabled": True,
            "root": str(self.root),
            "entries": len(entries),
            "bytes": sum(entry.size_bytes for entry in entries),
            "objects": sum(entry.kind == "object" for entry in entries),
            "mosaics": sum(entry.kind == "mosaic" for entry in entries),
            "tmp": sum(path.is_file() for path in self.tmp_dir.iterdir()),
            "leases": sum(path.is_file() for path in self.leases_dir.iterdir()),
        }


__all__ = ["CacheEntry", "CacheLease", "CacheStore"]
