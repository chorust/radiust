"""Private facade around the optional compiled Rust extension."""

from __future__ import annotations

import asyncio
import hashlib
import os
from pathlib import Path

try:
    from . import _core
except ImportError:  # source-tree development before a wheel is built
    _core = None

from .errors import ErrorContext, ResourceLimitError, StorageError


def version() -> str:
    return _core.version() if _core is not None else "python-fallback"


def sha256(data: bytes) -> str:
    return _core.sha256(data) if _core is not None else hashlib.sha256(data).hexdigest()


def validate_size(size: int, limit: int) -> int:
    try:
        if _core is not None:
            return int(_core.validate_size(size, limit))
    except ValueError as exc:
        raise ResourceLimitError(str(exc), context=ErrorContext(stage="acquire"), cause=exc) from exc
    if size > limit:
        raise ResourceLimitError(
            f"payload size {size} exceeds configured limit {limit}",
            context=ErrorContext(stage="acquire"),
        )
    return size


def managed_path(root: str | os.PathLike[str], candidate: str | os.PathLike[str]) -> Path:
    root_text = os.fspath(root)
    candidate_text = os.fspath(candidate)
    try:
        if _core is not None and hasattr(_core, "managed_path"):
            return Path(_core.managed_path(root_text, candidate_text))
        root_path = Path(root_text).expanduser().resolve(strict=True)
        candidate_path = Path(candidate_text).expanduser().resolve(strict=True)
        if not candidate_path.is_relative_to(root_path):
            raise ValueError(f"managed path escapes root: {candidate_path}")
        return candidate_path
    except (OSError, ValueError) as exc:
        raise StorageError(
            str(exc), context=ErrorContext(stage="validate"), cause=exc
        ) from exc


def path_is_within(root: str | os.PathLike[str], candidate: str | os.PathLike[str]) -> bool:
    root_text = os.fspath(root)
    candidate_text = os.fspath(candidate)
    try:
        if _core is not None and hasattr(_core, "path_is_within"):
            return bool(_core.path_is_within(root_text, candidate_text))
        root_path = Path(root_text).expanduser().resolve(strict=True)
        candidate_path = Path(candidate_text).expanduser().resolve(strict=True)
        return candidate_path.is_relative_to(root_path)
    except (OSError, ValueError) as exc:
        raise StorageError(
            str(exc), context=ErrorContext(stage="validate"), cause=exc
        ) from exc


async def sleep(seconds: float) -> None:
    """Await a Rust future when available, retaining a source-tree fallback."""
    if _core is not None and hasattr(_core, "sleep"):
        await _core.sleep(float(seconds))
    else:
        await asyncio.sleep(seconds)


def _require_object_storage() -> object:
    if _core is None or not hasattr(_core, "object_read") or not hasattr(_core, "object_write"):
        raise StorageError(
            "the compiled Rust object-storage adapter is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return _core


async def object_read(
    provider: str,
    bucket: str,
    prefix: str,
    key: str,
    *,
    endpoint: str | None = None,
    region: str | None = None,
    access_key_id: str | None = None,
    secret_access_key: str | None = None,
    anonymous: bool = False,
    max_bytes: int = 512 * 1024 * 1024,
) -> tuple[bytes, int, str] | None:
    core = _require_object_storage()
    try:
        return await core.object_read(
            provider,
            bucket,
            prefix,
            key,
            endpoint,
            region,
            access_key_id,
            secret_access_key,
            anonymous,
            int(max_bytes),
        )
    except ValueError as exc:
        raise ResourceLimitError(str(exc), context=ErrorContext(stage="acquire"), cause=exc) from exc
    except OSError as exc:
        raise StorageError(str(exc), context=ErrorContext(stage="acquire"), cause=exc) from exc


async def object_write(
    provider: str,
    bucket: str,
    prefix: str,
    key: str,
    chunks: list[bytes],
    *,
    endpoint: str | None = None,
    region: str | None = None,
    access_key_id: str | None = None,
    secret_access_key: str | None = None,
    anonymous: bool = False,
    overwrite: bool = False,
    max_bytes: int = 512 * 1024 * 1024,
) -> tuple[int, str]:
    core = _require_object_storage()
    try:
        return await core.object_write(
            provider,
            bucket,
            prefix,
            key,
            chunks,
            endpoint,
            region,
            access_key_id,
            secret_access_key,
            anonymous,
            overwrite,
            int(max_bytes),
        )
    except ValueError as exc:
        raise ResourceLimitError(str(exc), context=ErrorContext(stage="commit"), cause=exc) from exc
    except OSError as exc:
        raise StorageError(str(exc), context=ErrorContext(stage="commit"), cause=exc) from exc
