"""Iterator facades for the native completion-ordered fetch stream."""

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator, Iterable, Iterator
from typing import Any

from .errors import BatchError
from .models import BatchResult, FrameRef, FrameResult, Query

BatchInput = Query | FrameRef


def _fetch_item_result(item: Any) -> FrameResult:
    ref = item.frame()
    status = item.status
    if status == "cancelled":
        error = {"code": "cancelled", "message": item.error or "operation cancelled"}
    elif status == "not_started":
        error = {
            "code": "not_started",
            "message": item.error or "not started after batch failure",
        }
    else:
        details = item.error_details()
        if details:
            error = json.loads(details)
            error["code"] = {
                "unsupported": "unsupported_query",
                "internal": "decode",
            }.get(error.get("code"), error.get("code"))
            error.setdefault("source", ref.source)
        elif item.error:
            error = {"code": "native_error", "message": item.error}
        else:
            error = None
    raster_result = getattr(item, "raster_result", None)
    gray_result = getattr(item, "gray_result", None)
    data = raster_result() if callable(raster_result) else None
    if data is None and callable(gray_result):
        data = gray_result()
    elif data is None and not callable(gray_result):
        data = item.data()
    mode_info_json = getattr(item, "mode_info_json", None)
    mode_info = json.loads(mode_info_json()) if callable(mode_info_json) and mode_info_json() else None
    return FrameResult(ref, status, error=error, data=data, mode_info=mode_info)


def _batch_result(report: Any) -> BatchResult:
    return BatchResult(
        tuple(_fetch_item_result(report.item(index)) for index in range(report.total))
    )


class AsyncFetchStream(AsyncIterator[FrameResult]):
    def __init__(
        self,
        client: Any,
        query_or_refs: BatchInput | Iterable[BatchInput],
        *,
        on_error: str,
        max_prefetch: int | None,
        mode: str = "science",
    ):
        if on_error not in {"collect", "raise"}:
            raise ValueError("on_error must be collect or raise for iter_fetch")
        self.client = client
        self.query_or_refs = query_or_refs
        self.on_error = on_error
        self.max_prefetch = max_prefetch
        self.mode = mode
        self._refs: list[FrameRef] | None = None
        self._native: Any = None
        self._closed = False
        register = getattr(client, "_register_stream", None)
        if register is not None:
            register(self)

    async def _start(self) -> None:
        if self._native is not None:
            return
        from .rust_client import _resolve_frames

        self._refs = await _resolve_frames(self.client._session, self.query_or_refs)
        limit = self.max_prefetch or int(
            self.client.config.values["runtime"].get("frame_concurrency", 2)
        )
        if limit < 1:
            raise ValueError("max_prefetch must be positive")
        self.max_prefetch = limit
        if self.mode == "science":
            self._native = self.client._session.open_fetch_stream(
                self._refs,
                on_error=self.on_error,
                max_concurrency=limit,
            )
        else:
            self._native = self.client._session.open_fetch_mode_stream(
                self._refs,
                mode=self.mode,
                on_error=self.on_error,
                max_concurrency=limit,
            )

    async def __anext__(self) -> FrameResult:
        if self._closed:
            raise StopAsyncIteration
        try:
            await self._start()
            native_item, partial_report, cause = await self._native.next()
        except asyncio.CancelledError:
            await self.aclose()
            raise
        except BaseException:
            await self.aclose()
            raise

        if partial_report is not None:
            cause_error = RuntimeError(cause or "native stream stopped after a failure")
            partial = _batch_result(partial_report)
            await self.aclose()
            raise BatchError(
                "stream stopped after the first failure",
                partial_result=partial,
                cause=cause_error,
            ) from cause_error
        if native_item is None:
            await self.aclose()
            raise StopAsyncIteration
        return _fetch_item_result(native_item)

    async def aclose(self) -> None:
        if self._closed:
            return
        self._closed = True
        native = self._native
        self._native = None
        try:
            if native is not None:
                await native.close()
        finally:
            unregister = getattr(self.client, "_unregister_stream", None)
            if unregister is not None:
                unregister(self)

    async def __aenter__(self) -> AsyncFetchStream:
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        await self.aclose()


class SyncFetchStream(Iterator[FrameResult]):
    def __init__(
        self,
        client: Any,
        query_or_refs: BatchInput | Iterable[BatchInput],
        *,
        on_error: str,
        max_prefetch: int | None,
        mode: str = "science",
    ):
        self.client = client
        self._async = AsyncFetchStream(
            client,
            query_or_refs,
            on_error=on_error,
            max_prefetch=max_prefetch,
            mode=mode,
        )
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
