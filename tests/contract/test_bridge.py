from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from radiust import _bridge
from radiust.errors import ResourceLimitError, StorageError


def test_managed_path_returns_a_canonical_path_inside_root(tmp_path: Path) -> None:
    root = tmp_path / "managed"
    root.mkdir()
    payload = root / "payload.bin"
    payload.write_bytes(b"payload")

    result = _bridge.managed_path(root, payload)

    assert result == payload.resolve()
    assert isinstance(result, Path)


def test_managed_path_rejects_paths_outside_root(tmp_path: Path) -> None:
    root = tmp_path / "managed"
    root.mkdir()
    outside = tmp_path / "outside.bin"
    outside.write_bytes(b"outside")

    with pytest.raises(StorageError, match="managed path"):
        _bridge.managed_path(root, outside)

    assert _bridge.path_is_within(root, outside) is False


def test_validate_size_uses_the_public_resource_error() -> None:
    with pytest.raises(ResourceLimitError, match="payload size"):
        _bridge.validate_size(2, 1)


@pytest.mark.asyncio
async def test_rust_sleep_awaitable_propagates_python_cancellation() -> None:
    task = asyncio.create_task(_bridge.sleep(60.0))
    await asyncio.sleep(0)
    task.cancel()

    with pytest.raises(asyncio.CancelledError):
        await task
