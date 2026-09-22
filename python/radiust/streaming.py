"""Completion ordered, bounded-prefetch fetch iterators."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator, Iterable, Iterator
from typing import Any

from .batch import BatchInput, _cancelled, _failed, fetch_ref, resolve_refs
from .errors import BatchError
from .models import BatchResult, FrameRef, FrameResult


class AsyncFetchStream(AsyncIterator[FrameResult]):
    def __init__(self, client: Any, query_or_refs: BatchInput | Iterable[BatchInput], *, on_error: str, max_prefetch: int | None):
        if on_error not in {"collect", "raise"}:
            raise ValueError("on_error must be collect or raise for iter_fetch")
        self.client = client
        self.query_or_refs = query_or_refs
        self.on_error = on_error
        self.max_prefetch = max_prefetch
        self._refs: list[FrameRef] | None = None
        self._next_index = 0
        self._pending: dict[int, asyncio.Task[FrameResult]] = {}
        self._ready: asyncio.Queue[int] = asyncio.Queue()
        self._emitted: dict[int, FrameResult] = {}
        self._closed = False
        register = getattr(client, "_register_stream", None)
        if register is not None:
            register(self)

    async def _start(self) -> None:
        if self._refs is not None:
            return
        self._refs = await resolve_refs(self.client, self.query_or_refs)
        limit = self.max_prefetch or int(self.client.config.values["runtime"].get("frame_concurrency", 2))
        if limit < 1:
            raise ValueError("max_prefetch must be positive")
        self.max_prefetch = limit
        self._fill()

    def _fill(self) -> None:
        assert self._refs is not None
        while len(self._pending) < int(self.max_prefetch) and self._next_index < len(self._refs):
            index = self._next_index
            self._next_index += 1
            task = asyncio.create_task(fetch_ref(self.client, self._refs[index]))
            self._pending[index] = task
            task.add_done_callback(lambda _task, item=index: self._ready.put_nowait(item))

    async def __anext__(self) -> FrameResult:
        if self._closed:
            raise StopAsyncIteration
        try:
            await self._start()
        except BaseException:
            await self.aclose()
            raise
        assert self._refs is not None
        if not self._pending:
            self._closed = True
            raise StopAsyncIteration
        try:
            index = await self._ready.get()
        except asyncio.CancelledError:
            await self.aclose()
            raise
        task = self._pending.pop(index)
        try:
            result = task.result()
        except asyncio.CancelledError:
            result = _cancelled(self._refs[index], started=True)
        except Exception as exc:
            result = _failed(self._refs[index], exc)
            self._emitted[index] = result
            if self.on_error == "raise":
                active_indices = set(self._pending)
                partial = BatchResult(
                    tuple(
                        self._emitted.get(item, _cancelled(ref, started=item in active_indices or item < self._next_index))
                        for item, ref in enumerate(self._refs)
                    )
                )
                await self.aclose()
                raise BatchError("stream stopped after the first failure", partial_result=partial, cause=exc) from exc
        except BaseException:
            await self.aclose()
            raise
        self._emitted[index] = FrameResult(result.ref, result.status, result.output_uri, result.error)
        self._fill()
        return result

    async def aclose(self) -> None:
        if self._closed:
            return
        self._closed = True
        tasks = tuple(self._pending.values())
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        self._pending.clear()
        self._emitted.clear()
        unregister = getattr(self.client, "_unregister_stream", None)
        if unregister is not None:
            unregister(self)

    async def __aenter__(self) -> AsyncFetchStream:
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        await self.aclose()


class SyncFetchStream(Iterator[FrameResult]):
    def __init__(self, client: Any, query_or_refs: BatchInput | Iterable[BatchInput], *, on_error: str, max_prefetch: int | None):
        self.client = client
        self._async = AsyncFetchStream(client, query_or_refs, on_error=on_error, max_prefetch=max_prefetch)
        self._closed = False

    def __iter__(self) -> SyncFetchStream:
        return self

    def __next__(self) -> FrameResult:
        if self._closed:
            raise StopIteration
        try:
            return self.client._run(self._async.__anext__())
        except StopAsyncIteration:
            self.close()
            raise StopIteration from None
        except BaseException:
            self.close()
            raise

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        try:
            self.client._run(self._async.aclose())
        finally:
            unregister = getattr(self.client, "_unregister_stream", None)
            if unregister is not None:
                unregister(self._async)
            owner = getattr(self, "_owner_client", None)
            if owner is not None:
                owner.close()

    def __enter__(self) -> SyncFetchStream:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()


__all__ = ["AsyncFetchStream", "SyncFetchStream"]
