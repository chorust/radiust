from __future__ import annotations

import pytest
from radiust import _bridge
from radiust.errors import StorageError

core = pytest.importorskip("radiust._core")


def test_rust_object_storage_binding_is_exposed() -> None:
    assert callable(core.object_read)
    assert callable(core.object_write)


@pytest.mark.asyncio
async def test_rust_object_binding_rejects_an_unsupported_provider() -> None:
    with pytest.raises(StorageError, match="unsupported object storage provider"):
        await _bridge.object_read("unsupported", "bucket", "", "object.bin")


@pytest.mark.asyncio
async def test_rust_object_binding_rejects_unsafe_keys_before_network_access() -> None:
    with pytest.raises(StorageError, match="safe relative path"):
        await _bridge.object_read(
            "s3", "bucket", "", "../escape", region="us-east-1"
        )
