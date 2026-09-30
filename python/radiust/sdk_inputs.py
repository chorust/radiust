"""Query and frame input validation shared by the native SDK facades."""

from __future__ import annotations

from collections.abc import Callable, Iterable
from typing import Any

from .errors import NoDataError
from .models import FrameRef, Query

BatchInput = Query | FrameRef
ProgressCallback = Callable[[str, int, int | None], None]


def _is_native_frame(value: Any) -> bool:
    from . import _bridge

    return _bridge._core is not None and isinstance(value, _bridge._core.FrameRef)


def _is_frame(value: Any) -> bool:
    return isinstance(value, FrameRef) or _is_native_frame(value)


async def _discover(client: Any, query: Query) -> list[FrameRef]:
    private = getattr(client, "_adiscover", None)
    if private is not None:
        return list(await private(query))
    return list(await client.discover(query))


async def resolve_refs(
    client: Any,
    query_or_refs: BatchInput | Iterable[BatchInput],
    *,
    progress: ProgressCallback | None = None,
) -> list[FrameRef]:
    """Expand queries and validate ordered frame identities before I/O."""
    if isinstance(query_or_refs, Query) or _is_frame(query_or_refs):
        values: Iterable[BatchInput] = (query_or_refs,)
    else:
        if isinstance(query_or_refs, (str, bytes, bytearray)):
            raise TypeError("batch input must contain Query or FrameRef values")
        values = query_or_refs
    refs: list[FrameRef] = []
    if progress is not None:
        progress("resolve", 0, None)
    for value in values:
        if isinstance(value, Query):
            refs.extend(await _discover(client, value))
        elif _is_frame(value):
            refs.append(value)
        else:
            raise TypeError("batch input must contain Query or FrameRef values")
        if progress is not None:
            progress("resolve", len(refs), None)
    if not refs:
        raise NoDataError("batch input returned no frames")
    keys = [(ref.logical_id, getattr(ref, "revision", None)) for ref in refs]
    if len(keys) != len(set(keys)):
        raise ValueError("duplicate frame identity in batch")
    return refs


__all__ = ["BatchInput", "ProgressCallback", "resolve_refs"]
