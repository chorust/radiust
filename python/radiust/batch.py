"""Bounded batch acquisition shared by synchronous and asynchronous clients."""

from __future__ import annotations

import asyncio
import inspect
from collections.abc import Callable, Iterable
from typing import Any

from .errors import BatchError, NoDataError, error_from_exception
from .models import BatchResult, FrameRef, FrameResult, Query

BatchInput = Query | FrameRef
ProgressCallback = Callable[[str, int, int | None], None]


async def _discover(client: Any, query: Query) -> list[FrameRef]:
    private = getattr(client, "_adiscover", None)
    if private is not None:
        return list(await private(query))
    return list(await client.discover(query))


async def resolve_refs(client: Any, query_or_refs: BatchInput | Iterable[BatchInput], *,
                       progress: ProgressCallback | None = None) -> list[FrameRef]:
    """Expand queries and validate ordered frame identities before I/O."""
    if isinstance(query_or_refs, (Query, FrameRef)):
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
        elif isinstance(value, FrameRef):
            refs.append(value)
        else:
            raise TypeError("batch input must contain Query or FrameRef values")
        if progress is not None:
            progress("resolve", len(refs), None)
    if not refs:
        raise NoDataError("batch input returned no frames")
    keys = [(ref.logical_id, ref.revision) for ref in refs]
    if len(keys) != len(set(keys)):
        raise ValueError("duplicate frame identity in batch")
    return refs


async def fetch_ref(client: Any, ref: FrameRef) -> FrameResult:
    """Acquire and decode one frame while keeping its RawFrame alive."""
    async with client.acquire(ref) as raw:
        value = client.decode(raw)
        if inspect.isawaitable(value):
            value = await value
    return FrameResult(ref, "success", data=value)


def _failed(ref: FrameRef, exc: Exception) -> FrameResult:
    return FrameResult(
        ref,
        "failed",
        error=error_from_exception(exc, stage="process", source=ref.source).as_dict(),
    )


def _cancelled(ref: FrameRef, *, started: bool) -> FrameResult:
    status = "cancelled" if started else "not_started"
    code = "cancelled" if started else "not_started"
    message = "operation cancelled" if started else "not started after batch failure"
    return FrameResult(ref, status, error={"code": code, "message": message})


async def fetch_many_async(
    client: Any,
    query_or_refs: BatchInput | Iterable[BatchInput],
    *,
    on_error: str = "collect",
    max_concurrency: int | None = None,
    progress: ProgressCallback | None = None,
) -> BatchResult:
    refs = await resolve_refs(client, query_or_refs, progress=progress)
    if on_error not in {"collect", "raise"}:
        raise ValueError("on_error must be collect or raise for fetch_many")
    limit = max_concurrency or int(client.config.values["runtime"].get("frame_concurrency", 2))
    if limit < 1:
        raise ValueError("max_concurrency must be positive")

    semaphore = asyncio.Semaphore(limit)
    started = [False] * len(refs)
    completed_count = 0
    if progress is not None:
        progress("fetch", 0, len(refs))

    async def run(index: int, ref: FrameRef) -> FrameResult:
        nonlocal completed_count
        async with semaphore:
            started[index] = True
            try:
                return await fetch_ref(client, ref)
            finally:
                completed_count += 1
                if progress is not None:
                    progress("fetch", completed_count, len(refs))

    tasks = [asyncio.create_task(run(index, ref)) for index, ref in enumerate(refs)]
    if on_error == "collect":
        values = await asyncio.gather(*tasks, return_exceptions=True)
        items: list[FrameResult] = []
        for ref, value in zip(refs, values, strict=True):
            if isinstance(value, asyncio.CancelledError):
                items.append(_cancelled(ref, started=started[len(items)]))
            elif isinstance(value, Exception):
                items.append(_failed(ref, value))
            else:
                if isinstance(value, BaseException):
                    raise value
                items.append(value)
        return BatchResult(tuple(items))

    task_indices = {task: index for index, task in enumerate(tasks)}
    completed: dict[int, FrameResult] = {}
    pending = set(tasks)
    first_error: Exception | None = None
    try:
        while pending:
            done, pending = await asyncio.wait(pending, return_when=asyncio.FIRST_COMPLETED)
            for task in done:
                index = task_indices[task]
                try:
                    completed[index] = task.result()
                except asyncio.CancelledError:
                    completed[index] = _cancelled(refs[index], started=started[index])
                except Exception as exc:  # noqa: PERF203 - each task needs its frame context
                    completed[index] = _failed(refs[index], exc)
                    if first_error is None:
                        first_error = exc
            if first_error is not None:
                for task in pending:
                    task.cancel()
                await asyncio.gather(*pending, return_exceptions=True)
                break
    except asyncio.CancelledError:
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)
        raise
    except BaseException:
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)
        raise

    if first_error is not None:
        partial = BatchResult(
            tuple(
                completed.get(index, _cancelled(ref, started=started[index]))
                for index, ref in enumerate(refs)
            )
        )
        raise BatchError("batch stopped after the first failure", partial_result=partial, cause=first_error) from first_error
    return BatchResult(tuple(completed[index] for index in range(len(refs))))


__all__ = ["BatchInput", "fetch_many_async", "fetch_ref", "resolve_refs"]
