from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from radiust import _bridge
from radiust.errors import ResourceLimitError, StorageError


def test_query_tile_viewport_and_local_zoom_reach_native_payloads() -> None:
    from radiust import LocalViewRequest, Query

    tiles = {"bbox": (170.0, 1.0, -170.0, 10.0), "zoom": 3, "resolution_m": None}
    query = Query("rainviewer", latest=True, tiles=tiles)
    payload = _bridge._native_query_payload(query)
    assert payload["tiles"] == {**tiles, "bbox": list(tiles["bbox"])}
    assert payload["selector"] == {"kind": "latest"}
    assert _bridge._native_query_payload({"source": "windy", "tiles": tiles})["tiles"] == payload["tiles"]
    assert LocalViewRequest(zoom=6).as_dict()["zoom"] == 6
    assert "tiles" not in _bridge._native_query_payload(Query("rainviewer", latest=True))


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


def test_query_tiles_are_validated_in_python() -> None:
    from radiust import Query
    from radiust.errors import UnsupportedQueryError

    assert Query(source="rainviewer", latest=True, tiles={"zoom": 3}).tiles["zoom"] == 3
    for bad in ({"zoom": 99}, {"zoom": True}, {"bbox": (1, 2, 3)}, {"resolution_m": 0}, {"x": 1}):
        with pytest.raises(UnsupportedQueryError):
            Query(source="rainviewer", latest=True, tiles=bad)
