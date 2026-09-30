"""Public SDK clients backed by one persistent Rust Engine per client."""

from __future__ import annotations

import asyncio
import contextvars
import json
import os
import threading
import uuid
from collections.abc import Iterable, Mapping
from contextlib import asynccontextmanager, suppress
from pathlib import Path
from typing import Any

from . import _bridge
from .config import EffectiveConfig, load_config
from .errors import (
    AmbiguousFrameError,
    AsyncContextError,
    AuthenticationError,
    BatchError,
    ErrorContext,
    NoDataError,
    RadiustError,
    ResourceLimitError,
    StaleFrameError,
    StorageError,
    TransportError,
    UnsupportedQueryError,
)
from .models import BatchResult, DownloadReport, FrameRef, FrameResult, Query

ProgressCallback = Any
_DOWNLOAD_PROCESSING_KEYS = {
    "variable",
    "grid",
    "bbox",
    "resolution",
    "resampling",
    "encoder_options",
}
_SDK_ERROR_CODES = {"unsupported": "unsupported_query", "internal": "decode"}


def _native_download_processing(values: Mapping[str, Any]) -> dict[str, Any]:
    unknown = sorted(set(values) - _DOWNLOAD_PROCESSING_KEYS)
    if unknown:
        raise ValueError(f"unsupported processing option(s): {', '.join(unknown)}")
    encoder_options = values.get("encoder_options")
    if encoder_options is not None and not isinstance(encoder_options, Mapping):
        raise TypeError("encoder_options must be a mapping")
    if encoder_options:
        raise UnsupportedQueryError("encoder_options are not supported by Rust downloads yet")
    return {key: value for key, value in values.items() if key != "encoder_options"}


_ACTIVE_OPERATION_GATE: contextvars.ContextVar[tuple[Any, str] | None] = contextvars.ContextVar(
    "radiust_active_operation_gate", default=None
)


class _OperationGate:
    """Allow ordinary calls to overlap while isolating event-backed calls."""

    def __init__(self) -> None:
        self._condition = asyncio.Condition()
        self._readers = 0
        self._writer = False
        self._waiting_writers = 0

    @asynccontextmanager
    async def shared(self) -> Any:
        async with self._hold("shared"):
            yield

    @asynccontextmanager
    async def exclusive(self) -> Any:
        async with self._hold("exclusive"):
            yield

    @asynccontextmanager
    async def _hold(self, mode: str) -> Any:
        active = _ACTIVE_OPERATION_GATE.get()
        if active is not None and active[0] is self:
            if mode == "exclusive" and active[1] != "exclusive":
                raise RuntimeError("cannot enable progress callbacks inside an active operation")
            yield
            return

        async with self._condition:
            if mode == "exclusive":
                self._waiting_writers += 1
                try:
                    await self._condition.wait_for(lambda: not self._writer and self._readers == 0)
                    self._writer = True
                finally:
                    self._waiting_writers -= 1
                    self._condition.notify_all()
            else:
                await self._condition.wait_for(
                    lambda: not self._writer and self._waiting_writers == 0
                )
                self._readers += 1

        token = _ACTIVE_OPERATION_GATE.set((self, mode))
        try:
            yield
        finally:
            _ACTIVE_OPERATION_GATE.reset(token)
            async with self._condition:
                if mode == "exclusive":
                    self._writer = False
                else:
                    self._readers -= 1
                self._condition.notify_all()


class _ProgressEventBridge:
    """Translate this operation's Rust lifecycle events to the SDK callback."""

    _DOWNLOAD_OPERATIONS = {
        "download_raw_only",
        "download_png",
        "download_netcdf",
        "download_geotiff",
        "download_zarr",
    }

    def __init__(self, callback: ProgressCallback, mode: str) -> None:
        self.callback = callback
        self.mode = mode
        self.total: int | None = None
        self.discovered_count: int | None = None
        self.discovery_finished = False
        self.discovery_final_sent = False
        self.fetch_started = False
        self.fetch_finished: set[Any] = set()
        self.download_started: set[Any] = set()
        self.download_finished: set[Any] = set()
        self.terminal: set[Any] = set()
        self.last: tuple[str, int, int | None] | None = None
        self.callback_error: Exception | None = None

    def _emit(self, stage: str, completed: int, total: int | None) -> None:
        event = (stage, completed, total)
        if self.callback_error is not None or event == self.last:
            return
        self.last = event
        try:
            self.callback(*event)
        except Exception as exc:
            self.callback_error = exc

    def set_total(self, total: int) -> None:
        self.total = total

    def set_discovered_count(self, count: int) -> None:
        self.discovered_count = count
        self._finish_discovery()

    def set_fetch_count(self, count: int) -> None:
        self._emit("fetch", count, self.total)

    def set_download_count(self, count: int) -> None:
        self._emit("download", count, self.total)

    def _finish_discovery(self) -> None:
        if (
            self.discovery_finished
            and self.discovered_count is not None
            and not self.discovery_final_sent
        ):
            self.discovery_final_sent = True
            self._emit("discover", self.discovered_count, self.discovered_count)

    def consume_json(self, payload: str) -> None:
        if self.callback_error is not None:
            return
        for event in json.loads(payload):
            operation_id = event.get("operation_id")
            operation = event.get("operation")
            stage = event.get("stage")
            progress = event.get("progress") or {}

            if self.mode in {"discover", "fetch"} and operation == "discover":
                if stage == "started":
                    self._emit("discover", 0, None)
                elif stage == "progress":
                    self._emit(
                        "discover_targets",
                        progress["completed"],
                        progress.get("total"),
                    )
                elif stage in {"completed", "failed", "cancelled"}:
                    self.discovery_finished = stage == "completed"
                    self._finish_discovery()
                continue

            if self.mode == "fetch":
                if operation == "fetch_raw":
                    if stage == "started":
                        self._emit("acquire", 0, 1)
                    elif stage == "progress":
                        self._emit("acquire", progress["completed"], progress.get("total"))
                    elif stage == "completed":
                        self.terminal.add(operation_id)
                        self._emit("acquire", 1, 1)
                elif operation == "decode_science":
                    if stage == "started":
                        self._emit("decode", 0, 1)
                    elif stage == "completed":
                        self.terminal.add(operation_id)
                        self._emit("decode", 1, 1)
                continue

            if self.mode == "fetch_many" and operation == "fetch_raw":
                if stage == "started" and not self.fetch_started:
                    self.fetch_started = True
                    self._emit("fetch", 0, self.total)
                elif stage in {"completed", "failed", "cancelled"}:
                    if operation_id not in self.fetch_finished:
                        self.fetch_finished.add(operation_id)
                        self._emit("fetch", len(self.fetch_finished), self.total)
                continue

            if self.mode == "download" and operation in self._DOWNLOAD_OPERATIONS:
                if stage == "started":
                    self.download_started.add(operation_id)
                    self._emit("download", 0, self.total)
                elif stage == "completed":
                    if operation_id not in self.download_finished:
                        self.download_finished.add(operation_id)
                        self._emit("download", self.total or 0, self.total)
                continue


async def _watch_operation_events(events: Any, bridge: _ProgressEventBridge) -> None:
    while True:
        bridge.consume_json(events.drain_json())
        await asyncio.sleep(0.005)


@asynccontextmanager
async def _operation_scope(client: Any, progress: ProgressCallback | None, mode: str) -> Any:
    owner = getattr(client, "_async_owner", client)
    operations = getattr(owner, "_operations", None)
    task = asyncio.current_task()
    registered = operations is not None and task not in operations
    if operations is not None:
        owner._ensure_open()
        if registered:
            operations[task] = client._session
    try:
        async with _operation_scope_inner(client, progress, mode) as events:
            yield events
    finally:
        if registered:
            operations.pop(task, None)


@asynccontextmanager
async def _operation_scope_inner(client: Any, progress: ProgressCallback | None, mode: str) -> Any:
    gate: _OperationGate = client._event_gate
    if progress is None:
        async with gate.shared():
            yield None
        return

    async with gate.exclusive():
        events = client._session.subscribe_events()
        events.drain_json()  # discard completed events from earlier operations
        bridge = _ProgressEventBridge(progress, mode)
        watcher = asyncio.create_task(_watch_operation_events(events, bridge))
        operation_error: BaseException | None = None
        try:
            yield bridge
        except BaseException as exc:
            operation_error = exc
            raise
        finally:
            watcher.cancel()
            with suppress(asyncio.CancelledError):
                await watcher
            bridge.consume_json(events.drain_json())
            if operation_error is None and bridge.callback_error is not None:
                raise bridge.callback_error


def _json_value(value: Any) -> Any:
    if isinstance(value, Path):
        return os.fspath(value)
    if isinstance(value, Mapping):
        return {str(key): _json_value(child) for key, child in value.items()}
    if isinstance(value, (tuple, list)):
        return [_json_value(child) for child in value]
    if callable(getattr(value, "isoformat", None)):
        return value.isoformat()
    return value


def _effective_config(config: EffectiveConfig | Mapping[str, Any] | None) -> EffectiveConfig:
    return config if isinstance(config, EffectiveConfig) else load_config(config)


def _frame_document(frame: Any) -> dict[str, Any]:
    serializer = getattr(frame, "to_json", None)
    if callable(serializer):
        value = json.loads(serializer())
        if not isinstance(value, dict):
            raise UnsupportedQueryError("native FrameRef returned invalid JSON")
        return value
    if isinstance(frame, Mapping):
        return dict(frame)
    if isinstance(frame, FrameRef):
        return {
            "source": frame.source,
            "product": frame.product,
            "station": frame.station,
            "valid_time": frame.valid_time.isoformat(),
            "base_time": frame.base_time.isoformat() if frame.base_time else None,
            "logical_id": frame.logical_id,
            "revision": frame.revision,
            "locator_version": frame.locator_version,
        }
    raise TypeError("expected a Query or FrameRef")


def _is_native_frame(value: Any) -> bool:
    core = _bridge._core
    return core is not None and isinstance(value, core.FrameRef)


def _is_frame(value: Any) -> bool:
    return (
        isinstance(value, FrameRef)
        or _is_native_frame(value)
        or (isinstance(value, Mapping) and "logical_id" in value)
    )


def _is_query(value: Any) -> bool:
    return isinstance(value, Query) or (
        isinstance(value, Mapping) and "source" in value and "logical_id" not in value
    )


def _frame_key(frame: Any) -> tuple[str, str | None]:
    value = _frame_document(frame)
    return str(value.get("logical_id", "")), value.get("revision")


def _frame_error(message: str | None) -> dict[str, Any] | None:
    if not message:
        return None
    return {"code": "native_error", "message": message}


async def _discover_frames(session: _bridge.CoreEngineSession, query: Any) -> list[Any]:
    report = await session.discover_report(query)
    document = json.loads(report.to_json())
    frames: list[Any] = []
    for index, item in enumerate(document.get("items", ())):
        status = item.get("status")
        if status == "success" and item.get("frame") is not None:
            frames.append(report.frame(index))
        elif status != "no_data":
            error = item.get("error") or {}
            message = error.get("message") or f"discovery {status}"
            if status in {"cancelled", "not_started"}:
                raise asyncio.CancelledError(message)
            error_type = {
                "stale": StaleFrameError,
                "ambiguous": AmbiguousFrameError,
                "missing_credentials": AuthenticationError,
                "retired": UnsupportedQueryError,
                "network_restricted": TransportError,
                "timeout": TransportError,
                "upstream_failed": TransportError,
            }.get(status, RadiustError)
            if status == "upstream_failed":
                error_type = {
                    "unsupported": UnsupportedQueryError,
                    "resource_limit": ResourceLimitError,
                    "storage": StorageError,
                    "cache": StorageError,
                }.get(error.get("code"), error_type)
            raise error_type(message, context=ErrorContext(
                stage=error.get("stage", "discover"),
                source=(item.get("target") or {}).get("source"),
                retryable=error.get("retryable", False),
            ))
    return frames


async def _resolve_frames(
    session: _bridge.CoreEngineSession,
    query_or_refs: Any,
    *,
    progress: ProgressCallback | None = None,
) -> list[Any]:
    if _is_query(query_or_refs):
        values: Iterable[Any] = (query_or_refs,)
    elif _is_frame(query_or_refs):
        values = (query_or_refs,)
    else:
        if isinstance(query_or_refs, (str, bytes, bytearray)):
            raise TypeError("batch input must contain Query or FrameRef values")
        try:
            values = iter(query_or_refs)
        except TypeError as exc:
            raise TypeError("batch input must contain Query or FrameRef values") from exc

    if progress is not None:
        progress("resolve", 0, None)
    frames: list[Any] = []
    for value in values:
        if _is_query(value):
            frames.extend(await _discover_frames(session, value))
        elif _is_frame(value):
            frames.extend(_bridge._native_frames([value]))
        else:
            raise TypeError("batch input must contain Query or FrameRef values")
        if progress is not None:
            progress("resolve", len(frames), None)

    if not frames:
        raise NoDataError("batch input returned no frames")
    keys = [_frame_key(frame) for frame in frames]
    if len(keys) != len(set(keys)):
        raise ValueError("duplicate frame identity in batch")
    return frames


def _single(frames: list[Any], *, context: str = "query") -> Any:
    if not frames:
        raise NoDataError(f"{context} returned no frames")
    if len(frames) != 1:
        raise AmbiguousFrameError(
            f"{context} returned {len(frames)} frames; select a single time or station"
        )
    return frames[0]


def _download_report(
    native: Any, *, query: Any = None, command: str = "download"
) -> DownloadReport:
    items: list[FrameResult] = []
    for index in range(native.total):
        item = native.item(index)
        error_json = item.error_json()
        error = json.loads(error_json) if error_json else None
        items.append(
            FrameResult(
                item.frame(),
                item.status,
                output_uri=item.output_uri,
                error=error,
            )
        )
    return DownloadReport(
        command,
        uuid.uuid4().hex,
        tuple(items),
        query=query,
        interrupted=native.interrupted,
    )


def _write_field(value: Any, *, variable: str | None) -> Any:
    core = _bridge._core
    if core is None or not hasattr(core, "RadarField"):
        raise StorageError("the compiled Rust science model is unavailable")
    if isinstance(value, core.RadarField):
        if variable is not None and variable != value.name:
            raise ValueError(f"RadarField has no variable {variable!r}")
        return value
    if isinstance(value, core.RadarDataset):
        matches = []
        for index in range(value.field_count):
            field = value.field(index)
            if variable is None or field.name == variable:
                matches.append(field)
        if variable is not None:
            if not matches:
                raise ValueError(f"RadarDataset has no variable {variable!r}")
            if len(matches) > 1:
                raise ValueError(f"RadarDataset variable {variable!r} is ambiguous")
            return matches[0]
        if len(matches) != 1:
            raise ValueError("writing a multi-variable RadarDataset requires variable")
        return matches[0]
    raise TypeError("write expects a Rust RadarField or RadarDataset")


def _download_policy(on_error: str) -> str:
    if on_error in {"collect", "continue"}:
        return "collect"
    if on_error in {"raise", "stop"}:
        return "stop"
    raise ValueError("on_error must be collect/continue or raise/stop")


def _query_record(query: Any) -> dict[str, Any] | None:
    if not _is_query(query):
        return None
    if isinstance(query, Query):
        return {
            "source": query.source,
            "product": query.product,
            "stations": list(query.stations),
            "latest": query.latest,
            "at": query.at.isoformat() if query.at else None,
            "start": query.start.isoformat() if query.start else None,
            "end": query.end.isoformat() if query.end else None,
            "base_time": query.base_time.isoformat() if query.base_time else None,
            "max_age_secs": query.max_age.total_seconds() if query.max_age else None,
        }
    return _json_value(dict(query))


class _NativeAcquireContext:
    def __init__(self, client: Any, frame: Any):
        self.client = client
        self.frame = frame
        self.raw: Any = None

    def __enter__(self) -> Any:
        self.raw = self.client._run(self._afetch_raw())
        return self.raw

    async def _afetch_raw(self) -> Any:
        async with _operation_scope(self.client, None, "fetch"):
            return await self.client._session.fetch_raw(self.frame)

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        try:
            close = getattr(self.raw, "close", None)
            if close is not None:
                close()
        finally:
            self.raw = None

    async def __aenter__(self) -> Any:
        self.raw = await self._afetch_raw()
        return self.raw

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        try:
            close = getattr(self.raw, "close", None)
            if close is not None:
                close()
        finally:
            self.raw = None


class Client:
    """Synchronous facade whose I/O, decode and output path run in Rust."""

    def __init__(self, *, config: EffectiveConfig | Mapping[str, Any] | None = None):
        self.config = _effective_config(config)
        self._session = _bridge.CoreEngineSession(self.config)
        self._loop = asyncio.new_event_loop()
        self._thread_id = threading.get_ident()
        self._closed = False
        self._active_task: asyncio.Task[Any] | None = None
        self._streams: set[Any] = set()
        self._event_gate = _OperationGate()

    def __enter__(self) -> Client:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    def _run(self, awaitable: Any) -> Any:
        if self._closed:
            close = getattr(awaitable, "close", None)
            if close is not None:
                close()
            raise RuntimeError("Client is closed")
        if threading.get_ident() != self._thread_id:
            close = getattr(awaitable, "close", None)
            if close is not None:
                close()
            raise AsyncContextError("Client cannot be used from another thread")
        try:
            asyncio.get_running_loop()
        except RuntimeError:
            pass
        else:
            close = getattr(awaitable, "close", None)
            if close is not None:
                close()
            raise AsyncContextError("a running event loop is active; use AsyncClient")
        task = asyncio.ensure_future(awaitable, loop=self._loop)
        self._active_task = task
        try:
            return self._loop.run_until_complete(task)
        finally:
            self._active_task = None

    def cancel(self) -> None:
        self._session.cancel()
        task = self._active_task
        if task is not None and not self._loop.is_closed():
            with suppress(RuntimeError):
                self._loop.call_soon_threadsafe(task.cancel)

    async def _adiscover(
        self, query: Any, *, progress: ProgressCallback | None = None
    ) -> list[Any]:
        async with _operation_scope(self, progress, "discover") as events:
            frames = await _discover_frames(self._session, query)
            if events is not None:
                events.set_discovered_count(len(frames))
            return frames

    def discover(self, query: Any, *, progress: ProgressCallback | None = None) -> list[Any]:
        return self._run(self._adiscover(query, progress=progress))

    def acquire(self, ref: Any) -> _NativeAcquireContext:
        return _NativeAcquireContext(self, _bridge._native_frames([ref])[0])

    def decode(self, raw: Any) -> Any:
        coroutine = self._adecode(raw)
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return self._run(coroutine)
        if loop is self._loop:
            return coroutine
        coroutine.close()
        raise AsyncContextError("Client cannot be used from another event loop; use AsyncClient")

    async def _adecode(self, raw: Any) -> Any:
        async with _operation_scope(self, None, "decode"):
            return await self._session.decode_science(raw)

    def fetch(self, query: Any, *, progress: ProgressCallback | None = None) -> Any:
        return self._run(self._afetch(query, progress=progress))

    async def _afetch(self, query: Any, *, progress: ProgressCallback | None) -> Any:
        async with _operation_scope(self, progress, "fetch") as events:
            if _is_frame(query):
                ref = _bridge._native_frames([query])[0]
            else:
                refs = await _discover_frames(self._session, query)
                if events is not None:
                    events.set_discovered_count(len(refs))
                ref = _single(refs)
            async with _NativeAcquireContext(self, ref) as raw:
                return await self._session.decode_science(raw)

    def fetch_many(
        self,
        query_or_refs: Any,
        *,
        on_error: str = "collect",
        max_concurrency: int | None = None,
        progress: ProgressCallback | None = None,
    ) -> BatchResult:
        return self._run(
            self._afetch_many(
                query_or_refs,
                on_error=on_error,
                max_concurrency=max_concurrency,
                progress=progress,
            )
        )

    async def _afetch_many(
        self,
        query_or_refs: Any,
        *,
        on_error: str,
        max_concurrency: int | None,
        progress: ProgressCallback | None,
    ) -> BatchResult:
        async with _operation_scope(self, progress, "fetch_many") as events:
            return await self._afetch_many_inner(
                query_or_refs,
                on_error=on_error,
                max_concurrency=max_concurrency,
                progress=progress,
                events=events,
            )

    async def _afetch_many_inner(
        self,
        query_or_refs: Any,
        *,
        on_error: str,
        max_concurrency: int | None,
        progress: ProgressCallback | None,
        events: _ProgressEventBridge | None,
    ) -> BatchResult:
        if on_error not in {"collect", "raise", "continue", "stop"}:
            raise ValueError("on_error must be collect/continue or raise/stop")
        if max_concurrency is not None and max_concurrency < 1:
            raise ValueError("max_concurrency must be positive")
        frames = await _resolve_frames(self._session, query_or_refs, progress=progress)
        if events is not None:
            events.set_total(len(frames))
        native_report = await self._session.fetch_many_decoded(
            frames,
            on_error=_download_policy(on_error),
            max_concurrency=max_concurrency,
        )
        items: list[FrameResult] = []
        first_error: Exception | None = None
        for index in range(native_report.total):
            native_item = native_report.item(index)
            status = native_item.status
            frame = native_item.frame()
            error_details = native_item.error_details()
            if error_details:
                error = json.loads(error_details)
                error["code"] = _SDK_ERROR_CODES.get(error.get("code"), error.get("code"))
                error.setdefault("source", frame.source)
            else:
                error = _frame_error(native_item.error)
            if status == "failed" and first_error is None:
                first_error = RuntimeError(native_item.error or "native batch operation failed")
            items.append(
                FrameResult(native_item.frame(), status, error=error, data=native_item.data())
            )
        if events is not None:
            completed = sum(item.status != "not_started" for item in items)
            events.set_fetch_count(completed)
        result = BatchResult(tuple(items))
        if on_error in {"raise", "stop"} and result.failed:
            raise BatchError(
                "batch stopped after the first failure",
                partial_result=result,
                cause=first_error,
            ) from first_error
        return result

    def iter_fetch(
        self,
        query_or_refs: Any,
        *,
        on_error: str = "collect",
        max_prefetch: int | None = None,
    ) -> Any:
        from .streaming import SyncFetchStream

        stream = SyncFetchStream(
            self,
            query_or_refs,
            on_error=on_error,
            max_prefetch=max_prefetch,
        )
        return stream

    def _register_stream(self, stream: Any) -> None:
        self._streams.add(stream)

    def _unregister_stream(self, stream: Any) -> None:
        self._streams.discard(stream)

    def download(
        self,
        query_or_refs: Any,
        *,
        output: str | os.PathLike[str] = "./data",
        format: str = "netcdf",
        raw: bool = False,
        raw_only: bool = False,
        overwrite: bool = False,
        output_template: str | None = None,
        on_error: str = "collect",
        config: EffectiveConfig | Mapping[str, Any] | None = None,
        progress: ProgressCallback | None = None,
        **processing: Any,
    ) -> DownloadReport:
        if config is not None and config is not self.config:
            self.config = _effective_config(config)
            self._session = _bridge.CoreEngineSession(self.config)
        return self._run(
            self._adownload(
                query_or_refs,
                output=output,
                format=format,
                raw=raw,
                raw_only=raw_only,
                overwrite=overwrite,
                output_template=output_template,
                on_error=on_error,
                progress=progress,
                processing=processing,
            )
        )

    async def _adownload(
        self,
        query_or_refs: Any,
        *,
        output: str | os.PathLike[str],
        format: str,
        raw: bool,
        raw_only: bool,
        overwrite: bool,
        output_template: str | None,
        on_error: str,
        progress: ProgressCallback | None,
        processing: Mapping[str, Any],
    ) -> DownloadReport:
        async with _operation_scope(self, progress, "download") as events:
            return await self._adownload_inner(
                query_or_refs,
                output=output,
                format=format,
                raw=raw,
                raw_only=raw_only,
                overwrite=overwrite,
                output_template=output_template,
                on_error=on_error,
                progress=progress,
                processing=processing,
                events=events,
            )

    async def _adownload_inner(
        self,
        query_or_refs: Any,
        *,
        output: str | os.PathLike[str],
        format: str,
        raw: bool,
        raw_only: bool,
        overwrite: bool,
        output_template: str | None,
        on_error: str,
        progress: ProgressCallback | None,
        processing: Mapping[str, Any],
        events: _ProgressEventBridge | None,
    ) -> DownloadReport:
        if raw and raw_only:
            raise ValueError("raw and raw_only are mutually exclusive")
        if raw_only and output_template is not None:
            raise UnsupportedQueryError("output_template currently applies to decoded outputs only")
        native_processing = _native_download_processing(processing)
        if raw_only and (
            native_processing.get("variable") is not None
            or native_processing.get("grid", "native") != "native"
            or native_processing.get("bbox") is not None
            or native_processing.get("resolution") is not None
            or native_processing.get("resampling", "nearest") != "nearest"
        ):
            raise UnsupportedQueryError(
                "raw_only cannot be combined with decoded processing options"
            )
        if format not in {"png", "netcdf", "geotiff", "zarr"}:
            raise ValueError("format must be png, netcdf, geotiff, or zarr")
        output_text = os.fspath(output)
        if "://" in output_text and not output_text.lower().startswith(("s3://", "oss://")):
            raise StorageError("remote output targets require an s3:// or oss:// URI")
        policy = _download_policy(on_error)
        frames = await _resolve_frames(self._session, query_or_refs, progress=progress)
        if events is not None:
            events.set_total(len(frames))
        if raw_only:
            native = await self._session.download_raw_only(
                frames,
                on_error=policy,
                overwrite=overwrite,
                output_root=output_text,
            )
        else:
            method = {
                "png": self._session.download_png,
                "netcdf": self._session.download_netcdf,
                "geotiff": self._session.download_geotiff,
                "zarr": self._session.download_zarr,
            }[format]
            native = await method(
                frames,
                on_error=policy,
                overwrite=overwrite,
                output_root=output_text,
                output_template=output_template,
                include_raw=raw,
                processing=native_processing,
            )
        report = _download_report(native, query=_query_record(query_or_refs))
        if events is not None:
            events.set_download_count(len(report.items))
        if on_error in {"raise", "stop"} and report.counts["failed"]:
            cause = RuntimeError("native download reported failed frames")
            raise BatchError(
                "batch stopped after the first failure",
                partial_result=report,
                cause=cause,
            ) from cause
        return report

    def write(
        self,
        field: Any,
        *,
        output: str | os.PathLike[str] = "./data",
        format: str = "netcdf",
        overwrite: bool = False,
        ref: Any = None,
        raw: bool = False,
        **options: Any,
    ) -> DownloadReport:
        return self._run(
            self._awrite(
                field,
                output=output,
                format=format,
                overwrite=overwrite,
                ref=ref,
                raw=raw,
                options=options,
            )
        )

    async def _awrite(
        self,
        field: Any,
        *,
        output: str | os.PathLike[str],
        format: str,
        overwrite: bool,
        ref: Any,
        raw: bool,
        options: Mapping[str, Any],
    ) -> DownloadReport:
        async with _operation_scope(self, None, "write"):
            return await self._awrite_inner(
                field,
                output=output,
                format=format,
                overwrite=overwrite,
                ref=ref,
                raw=raw,
                options=options,
            )

    async def _awrite_inner(
        self,
        field: Any,
        *,
        output: str | os.PathLike[str],
        format: str,
        overwrite: bool,
        ref: Any,
        raw: bool,
        options: Mapping[str, Any],
    ) -> DownloadReport:
        if raw:
            raise ValueError("write accepts decoded values only; use download(..., raw=True)")
        if ref is None:
            raise ValueError("write requires the source FrameRef used to produce the value")
        if format not in {"png", "netcdf", "geotiff", "zarr"}:
            raise ValueError("format must be png, netcdf, geotiff, or zarr")
        variable = options.get("variable")
        if variable is not None and not isinstance(variable, str):
            raise TypeError("variable must be a string")
        if (
            options.get("grid", "native") != "native"
            or options.get("bbox") is not None
            or options.get("resolution") is not None
            or options.get("resampling", "nearest") != "nearest"
        ):
            raise UnsupportedQueryError(
                "Rust in-memory writes do not yet support grid reprocessing"
            )
        native_field = _write_field(field, variable=variable)
        native_frame = _bridge._native_frames([ref])[0]
        output_text = os.fspath(output)
        if "://" in output_text:
            raise StorageError(
                "in-memory science writes currently require a local output directory"
            )
        try:
            options_json = json.dumps(
                _json_value(dict(options)), ensure_ascii=False, allow_nan=False
            )
        except (TypeError, ValueError) as exc:
            raise ValueError("writer options must be finite JSON values") from exc
        native = await self._session._engine.write_science(
            native_frame,
            native_field,
            format,
            bool(overwrite),
            options_json,
            variable,
            output_text,
        )
        return _download_report(native, command="write")

    def close(self) -> None:
        if self._closed:
            return
        for stream in tuple(self._streams):
            with suppress(Exception):
                self._run(stream.aclose())
        self._streams.clear()
        self._closed = True
        if not self._loop.is_closed():
            self._loop.close()


class AsyncClient:
    """Asynchronous public facade backed by one persistent Rust Engine."""

    def __init__(self, *, config: EffectiveConfig | Mapping[str, Any] | None = None):
        self.config = _effective_config(config)
        self._session = _bridge.CoreEngineSession(self.config)
        self._closed = False
        self._streams: set[Any] = set()
        self._event_gate = _OperationGate()
        self._operations: dict[asyncio.Task[Any], Any] = {}
        self._close_task: asyncio.Task[None] | None = None

    async def __aenter__(self) -> AsyncClient:
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        await self.aclose()

    def _ensure_open(self) -> None:
        if self._closed:
            raise RuntimeError("Client is closed")

    def cancel(self) -> None:
        self._session.cancel()

    async def _adiscover(
        self, query: Any, *, progress: ProgressCallback | None = None
    ) -> list[Any]:
        self._ensure_open()
        return await Client._adiscover(self, query, progress=progress)

    async def discover(self, query: Any, *, progress: ProgressCallback | None = None) -> list[Any]:
        self._ensure_open()
        return await self._adiscover(query, progress=progress)

    def acquire(self, ref: Any) -> _NativeAcquireContext:
        self._ensure_open()
        return _NativeAcquireContext(self, _bridge._native_frames([ref])[0])

    async def decode(self, raw: Any) -> Any:
        self._ensure_open()
        return await Client._adecode(self, raw)

    async def fetch(self, query: Any, *, progress: ProgressCallback | None = None) -> Any:
        self._ensure_open()
        return await Client._afetch(self, query, progress=progress)

    async def fetch_many(
        self,
        query_or_refs: Any,
        *,
        on_error: str = "collect",
        max_concurrency: int | None = None,
        progress: ProgressCallback | None = None,
    ) -> BatchResult:
        self._ensure_open()
        helper = Client.__new__(Client)
        helper.config = self.config
        helper._session = self._session
        helper._event_gate = self._event_gate
        helper._async_owner = self
        return await Client._afetch_many(
            helper,
            query_or_refs,
            on_error=on_error,
            max_concurrency=max_concurrency,
            progress=progress,
        )

    def aiter_fetch(
        self,
        query_or_refs: Any,
        *,
        on_error: str = "collect",
        max_prefetch: int | None = None,
    ) -> Any:
        from .streaming import AsyncFetchStream

        self._ensure_open()
        stream = AsyncFetchStream(
            self,
            query_or_refs,
            on_error=on_error,
            max_prefetch=max_prefetch,
        )
        self._streams.add(stream)
        return stream

    def _register_stream(self, stream: Any) -> None:
        self._streams.add(stream)

    def _unregister_stream(self, stream: Any) -> None:
        self._streams.discard(stream)

    async def download(
        self,
        query_or_refs: Any,
        *,
        output: str | os.PathLike[str] = "./data",
        format: str = "netcdf",
        raw: bool = False,
        raw_only: bool = False,
        overwrite: bool = False,
        output_template: str | None = None,
        on_error: str = "collect",
        config: EffectiveConfig | Mapping[str, Any] | None = None,
        progress: ProgressCallback | None = None,
        **processing: Any,
    ) -> DownloadReport:
        self._ensure_open()
        if config is not None and config is not self.config:
            self.config = _effective_config(config)
            self._session = _bridge.CoreEngineSession(self.config)
        helper = Client.__new__(Client)
        helper.config = self.config
        helper._session = self._session
        helper._event_gate = self._event_gate
        helper._async_owner = self
        return await Client._adownload(
            helper,
            query_or_refs,
            output=output,
            format=format,
            raw=raw,
            raw_only=raw_only,
            overwrite=overwrite,
            output_template=output_template,
            on_error=on_error,
            progress=progress,
            processing=processing,
        )

    async def write(
        self,
        field: Any,
        *,
        output: str | os.PathLike[str] = "./data",
        format: str = "netcdf",
        overwrite: bool = False,
        ref: Any = None,
        raw: bool = False,
        **options: Any,
    ) -> DownloadReport:
        self._ensure_open()
        async with _operation_scope(self, None, "write"):
            return await Client._awrite_inner(
                self,
                field,
                output=output,
                format=format,
                overwrite=overwrite,
                ref=ref,
                raw=raw,
                options=options,
            )

    async def aclose(self) -> None:
        if self._close_task is None:
            self._closed = True
            self._close_task = asyncio.create_task(self._close_operations(asyncio.current_task()))
        await asyncio.shield(self._close_task)

    async def _close_operations(self, caller: Any) -> None:
        operations = tuple(self._operations.items())
        for session in {session for _, session in operations}:
            session.cancel()
        tasks = [task for task, _ in operations if task is not caller]
        for task in tasks:
            task.cancel()
        if tasks:
            await asyncio.gather(*tasks, return_exceptions=True)
        streams = tuple(self._streams)
        try:
            for stream in streams:
                await stream.aclose()
        finally:
            self._streams.clear()


__all__ = ["Client", "AsyncClient"]
