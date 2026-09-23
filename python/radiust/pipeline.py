"""The shared discover → acquire → decode → stage → commit pipeline."""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import shutil
import tempfile
import threading
import uuid
from collections.abc import Callable, Iterable, Mapping
from contextlib import suppress
from pathlib import Path
from typing import Any

from .batch import BatchInput, fetch_many_async, resolve_refs
from .cache import CacheStore
from .config import EffectiveConfig, load_config
from .context import Cancellation, SourceContext
from .errors import (
    AsyncContextError,
    BatchError,
    IntegrityError,
    NoDataError,
    StorageError,
    error_from_exception,
)
from .identity import (
    artifact_bytes,
    cache_key,
    canonical_json,
    output_id,
    processing_hash,
    processing_identity,
    resolved_revision,
    safe_ref,
)
from .models import (
    Artifact,
    BatchResult,
    DownloadReport,
    FrameRef,
    FrameResult,
    ProcessingSpec,
    Query,
)
from .outputs.registry import check_encoder_dependencies, encoder_for
from .query import select_one
from .raw import RawFrame, raw_manifest
from .registry import get_source
from .storage.commit import RemoteArtifact, RemoteBackend, RemoteCommitRequest, RemoteCommitter
from .storage.local import LocalStore, local_output_path
from .storage.naming import output_relative_location
from .storage.object import parse_object_uri
from .streaming import AsyncFetchStream, SyncFetchStream

_ALLOWED_DOWNLOAD_PROCESSING = {"variable", "grid", "bbox", "resolution", "resampling", "encoder_options"}
ProgressCallback = Callable[[str, int, int | None], None]


def _progress(callback: ProgressCallback | None, stage: str, completed: int, total: int | None = None) -> None:
    if callback is not None:
        callback(stage, completed, total)


class _RemoteOutput:
    def __init__(self, uri: str, backend: RemoteBackend) -> None:
        location = parse_object_uri(uri)
        self.uri = uri.rstrip("/")
        self.prefix = location.key.rstrip("/")
        self.backend = backend

    def pointer_key(self, relative: str) -> str:
        return f"{self.prefix}/{relative}.manifest.json"

    def pointer_uri(self, key: str) -> str:
        return f"{self.uri}/{key.removeprefix(self.prefix + '/') if self.prefix else key}"


def _normalize_download_processing(values: Mapping[str, Any]) -> dict[str, Any]:
    unknown = sorted(set(values) - _ALLOWED_DOWNLOAD_PROCESSING)
    if unknown:
        raise ValueError(f"unsupported processing option(s): {', '.join(unknown)}")
    options = values.get("encoder_options", {})
    if options is None:
        options = {}
    if not isinstance(options, Mapping):
        raise TypeError("encoder_options must be a mapping")
    return {"variable": values.get("variable"), "grid": values.get("grid", "native"), "bbox": values.get("bbox"), "resolution": values.get("resolution"), "resampling": values.get("resampling", "nearest"), "options": dict(options)}


def _as_config(config: EffectiveConfig | dict[str, Any] | None) -> EffectiveConfig:
    return config if isinstance(config, EffectiveConfig) else load_config(config)


def _new_http_transport(config: EffectiveConfig) -> Any:
    from .transport import HTTPTransport

    runtime = config.values["runtime"]
    return HTTPTransport(
        allow_network=bool(runtime.get("allow_network", False)),
        max_bytes=int(runtime["max_artifact_bytes"]),
        timeout=float(runtime["request_timeout"]),
        request_concurrency=int(runtime["request_concurrency"]),
        host_concurrency=int(runtime["host_concurrency"]),
    )


def _source_temp_root(runtime: Mapping[str, Any]) -> Path | None:
    configured = runtime.get("temp_root")
    if configured is None:
        return None
    parent = Path(configured)
    parent.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix="radiust-source-", dir=parent))


def _raw_cache_keys(ref: FrameRef) -> tuple[str, str] | None:
    if ref.revision is None:
        # A latest ref without an upstream revision must be rediscovered and
        # revalidated on every operation; caching it would freeze a moving URL.
        return None
    frame_key = cache_key(ref, ref.revision)
    return f"raw:{frame_key}:manifest", frame_key


def _load_cached_raw(cache: CacheStore, ref: FrameRef) -> RawFrame | None:
    keys = _raw_cache_keys(ref)
    if keys is None:
        return None
    manifest_key, frame_key = keys
    leases: list[Any] = []
    try:
        manifest_lease = cache.lease(manifest_key)
        manifest_lease.__enter__()
        leases.append(manifest_lease)
        entry = cache.get(manifest_key, validator=ref.revision)
        if entry is None:
            raise LookupError("cached raw manifest is missing")
        document = json.loads(entry.read_bytes())
        if document.get("schema_version") != 1 or document.get("logical_id") != ref.logical_id:
            raise ValueError("cache manifest identity mismatch")
        descriptors = document.get("artifacts")
        if not isinstance(descriptors, list) or not descriptors:
            raise ValueError("cache manifest has no artifacts")
        artifacts: list[Artifact] = []
        for descriptor in descriptors:
            name = str(descriptor["name"])
            artifact_key = f"raw:{frame_key}:artifact:{name}"
            artifact_lease = cache.lease(artifact_key)
            artifact_lease.__enter__()
            leases.append(artifact_lease)
            artifact_entry = cache.get(artifact_key, validator=str(descriptor["sha256"]))
            if artifact_entry is None:
                raise LookupError(f"cached artifact is missing: {name}")
            if artifact_entry.sha256 != str(descriptor["sha256"]):
                raise ValueError("cached artifact hash mismatch")
            artifacts.append(
                Artifact(
                    name=name,
                    role=str(descriptor["role"]),
                    media_type=str(descriptor["media_type"]),
                    payload=artifact_entry.path,
                    source_revision=descriptor.get("source_revision"),
                    size_bytes=int(descriptor["size_bytes"]),
                    sha256=artifact_entry.sha256,
                )
            )
        metadata = document.get("metadata", {})
        if not isinstance(metadata, dict):
            raise ValueError("cache metadata must be a mapping")
        return RawFrame(ref, tuple(artifacts), metadata=metadata, _cache_leases=tuple(leases))
    except (IntegrityError, KeyError, LookupError, TypeError, ValueError, json.JSONDecodeError):
        for lease in reversed(leases):
            with suppress(Exception):
                lease.__exit__(None, None, None)
        return None


def _store_cached_raw(cache: CacheStore, ref: FrameRef, raw: RawFrame) -> None:
    keys = _raw_cache_keys(ref)
    if keys is None or any(artifact.role == "tile" for artifact in raw.artifacts):
        return
    manifest_key, frame_key = keys
    try:
        metadata = json.loads(canonical_json(raw.metadata))
        descriptors: list[dict[str, Any]] = []
        for artifact in raw.artifacts:
            payload = artifact_bytes(artifact)
            entry = cache.put(
                f"raw:{frame_key}:artifact:{artifact.name}",
                payload,
                kind="object",
                validator=hashlib.sha256(payload).hexdigest(),
            )
            if entry is None:
                return
            descriptors.append(
                {
                    "name": artifact.name,
                    "role": artifact.role,
                    "media_type": artifact.media_type,
                    "size_bytes": len(payload),
                    "sha256": entry.sha256,
                    "source_revision": artifact.source_revision,
                }
            )
        document = {
            "schema_version": 1,
            "logical_id": ref.logical_id,
            "ref": safe_ref(ref),
            "metadata": metadata,
            "artifacts": descriptors,
        }
        cache.put(
            manifest_key,
            canonical_json(document).encode("utf-8"),
            kind="object",
            validator=ref.revision,
        )
    except (OSError, TypeError, ValueError):
        # Cache is an optimization. A cache write failure must not turn a
        # verified acquisition into a source failure.
        return


class _AcquireContext:
    def __init__(self, client: Client, ref: FrameRef):
        self.client = client
        self.ref = ref
        self.raw: RawFrame | None = None

    def __enter__(self) -> RawFrame:
        self.raw = self.client._run(self.client._aacquire(self.ref))
        return self.raw

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self.raw is not None:
            self.raw.close()

    async def __aenter__(self) -> RawFrame:
        self.raw = await self.client._aacquire(self.ref)
        return self.raw

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        if self.raw is not None:
            await self.raw.aclose()


class Client:
    """Synchronous facade backed by one private event loop and one thread."""

    def __init__(self, *, config: EffectiveConfig | dict[str, Any] | None = None, _cancellation: Cancellation | None = None, _remote_backend: RemoteBackend | None = None, _transport: Any = None):
        self.config = _as_config(config)
        self._cancellation = _cancellation
        self._remote_backend = _remote_backend
        self._loop = asyncio.new_event_loop()
        self._thread_id = threading.get_ident()
        self._closed = False
        self._transport = _transport
        self._active_task: asyncio.Task[Any] | None = None
        self._cache: CacheStore | None = None
        self._streams: set[AsyncFetchStream] = set()

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
        """Request cancellation of the active operation from another thread."""
        if self._cancellation is not None:
            self._cancellation.cancel()
        task = self._active_task
        if task is not None and not self._loop.is_closed():
            with suppress(RuntimeError):
                self._loop.call_soon_threadsafe(task.cancel)

    def _context(self, source_id: str) -> SourceContext:
        runtime = self.config.values["runtime"]
        if self._transport is None:
            self._transport = _new_http_transport(self.config)
        return SourceContext(
            self.config,
            source_id,
            cancellation=self._cancellation or Cancellation(),
            cache=self._cache_store(),
            temp_root=_source_temp_root(runtime),
            transport=self._transport,
        )

    def _cache_store(self) -> CacheStore:
        if self._cache is None:
            values = self.config.values
            try:
                self._cache = CacheStore(
                    self.config.cache_dir,
                    output_root=self.config.output_root,
                    max_bytes=int(values["cache"]["max_bytes"]),
                    max_age_days=int(values["cache"]["max_age_days"]),
                    enabled=bool(values["cache"].get("enabled", True)),
                )
            except OSError:
                # Cache is optional; an unwritable cache must not block a
                # source acquisition or make the default SDK unusable.
                self._cache = CacheStore(self.config.cache_dir, enabled=False)
        return self._cache

    async def _download_raw(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        try:
            cached = _load_cached_raw(context.cache, ref) if isinstance(context.cache, CacheStore) else None
        except (OSError, StorageError, IntegrityError):
            cached = None
        if cached is not None:
            context.metadata["cache_hit"] = True
            context.check_artifacts(cached.artifacts)
            return cached
        raw = await get_source(ref.source).download(ref, context)
        context.cancellation.check()
        context.check_artifacts(raw.artifacts)
        if isinstance(context.cache, CacheStore):
            _store_cached_raw(context.cache, ref, raw)
        return raw

    async def _adiscover(self, query: Query) -> list[FrameRef]:
        context = self._context(query.source)
        try:
            context.cancellation.check()
            return await get_source(query.source).discover(query, context)
        finally:
            context.close()

    async def _aacquire(self, ref: FrameRef) -> RawFrame:
        context = self._context(ref.source)
        try:
            raw = await self._download_raw(ref, context)
            try:
                context.check_artifacts(raw.artifacts)
            except BaseException:
                await raw.aclose()
                raise
            raw._adopt_temp_root(context.detach_temp_root())
            return raw
        finally:
            context.close()

    def discover(self, query: Query, *, progress: ProgressCallback | None = None) -> list[FrameRef]:
        _progress(progress, "discover", 0)
        refs = self._run(self._adiscover(query))
        _progress(progress, "discover", len(refs), len(refs))
        return refs

    def acquire(self, ref: FrameRef) -> _AcquireContext:
        return _AcquireContext(self, ref)

    def decode(self, raw: RawFrame) -> Any:
        context = self._context(raw.ref.source)
        try:
            value = get_source(raw.ref.source).decode(raw, context)
            context.check_value(value)
            return value
        finally:
            context.close()

    def fetch(self, query: Query, *, progress: ProgressCallback | None = None) -> Any:
        refs = self.discover(query, progress=progress)
        ref = _one(refs)
        _progress(progress, "acquire", 0, 1)
        with self.acquire(ref) as raw:
            _progress(progress, "acquire", 1, 1)
            _progress(progress, "decode", 0, 1)
            value = self.decode(raw)
            _progress(progress, "decode", 1, 1)
            return value

    def fetch_many(
        self,
        query_or_refs: BatchInput | Iterable[BatchInput],
        *,
        on_error: str = "collect",
        max_concurrency: int | None = None,
        progress: ProgressCallback | None = None,
    ) -> BatchResult:
        return self._run(fetch_many_async(self, query_or_refs, on_error=on_error, max_concurrency=max_concurrency, progress=progress))

    def iter_fetch(
        self,
        query_or_refs: BatchInput | Iterable[BatchInput],
        *,
        on_error: str = "collect",
        max_prefetch: int | None = None,
    ) -> SyncFetchStream:
        return SyncFetchStream(self, query_or_refs, on_error=on_error, max_prefetch=max_prefetch)

    def _register_stream(self, stream: AsyncFetchStream) -> None:
        self._streams.add(stream)

    def _unregister_stream(self, stream: AsyncFetchStream) -> None:
        self._streams.discard(stream)

    def write(self, field: Any, *, output: str | Path = "./data", format: str = "netcdf", overwrite: bool = False, ref: FrameRef | None = None, raw: bool = False, **options: Any) -> DownloadReport:
        if raw:
            raise ValueError("write accepts decoded values only; use download(..., raw=True)")
        if ref is None:
            raise ValueError("write requires the source FrameRef used to produce the value")
        return self._run(self._awrite(field, ref=ref, output=output, format=format, overwrite=overwrite, options=options))

    async def _awrite(self, field: Any, *, ref: FrameRef, output: str | Path, format: str, overwrite: bool, options: dict[str, Any]) -> DownloadReport:
        spec = ProcessingSpec(format=format, variable=options.get("variable"), grid=options.get("grid", "native"), bbox=options.get("bbox"), resolution=options.get("resolution"), resampling=options.get("resampling", "nearest"), options=options)
        field = _apply_processing(field, spec)
        # A manually supplied field has no acquisition artifacts; its revision is its serialized scientific identity.
        revision = resolved_revision((), ref.revision) if ref.revision else output_id(ref, ref.logical_id, spec)
        status, path = await self._commit_value(field, ref, revision, spec, self._output_target(output), overwrite=overwrite, raw=None)
        return DownloadReport("write", uuid.uuid4().hex, (FrameResult(ref, status, str(path) if path else None),))

    def download(self, query_or_refs: BatchInput | Iterable[BatchInput], *, output: str | Path = "./data", format: str = "netcdf", raw: bool = False, raw_only: bool = False, overwrite: bool = False, output_template: str | None = None, on_error: str = "collect", config: EffectiveConfig | None = None, progress: ProgressCallback | None = None, **processing: Any) -> DownloadReport:
        if config is not None and config is not self.config:
            self.config = _as_config(config)
            self._cache = None
            self._transport = None
        return self._run(self._adownload(query_or_refs, output=output, format=format, raw=raw, raw_only=raw_only, overwrite=overwrite, output_template=output_template, on_error=on_error, processing=processing, progress=progress))

    async def _adownload(self, query_or_refs: BatchInput | Iterable[BatchInput], *, output: str | Path, format: str, raw: bool, raw_only: bool, overwrite: bool, output_template: str | None, on_error: str, processing: dict[str, Any], progress: ProgressCallback | None = None) -> DownloadReport:
        normalized = _normalize_download_processing(processing)
        if raw and raw_only:
            raise ValueError("raw and raw_only are mutually exclusive")
        if on_error not in {"collect", "raise", "continue", "stop"}:
            raise ValueError("on_error must be collect or raise")
        if not raw_only:
            check_encoder_dependencies(format)
        output_root = self._output_target(output)
        operation_context: SourceContext | None = None
        try:
            if isinstance(query_or_refs, Query):
                _progress(progress, "discover", 0)
                operation_context = self._context(query_or_refs.source)
                refs = await get_source(query_or_refs.source).discover(query_or_refs, operation_context)
                if not refs:
                    raise NoDataError("batch input returned no frames")
                keys = [(ref.logical_id, ref.revision) for ref in refs]
                if len(keys) != len(set(keys)):
                    raise ValueError("duplicate frame identity in batch")
            else:
                refs = await resolve_refs(self, query_or_refs)
            _progress(progress, "discover", len(refs), len(refs))
            spec = ProcessingSpec(output_kind="raw-only" if raw_only else "decoded", format=format, variable=normalized["variable"], grid=normalized["grid"], bbox=normalized["bbox"], resolution=normalized["resolution"], resampling=normalized["resampling"], options=normalized["options"])
            results: list[FrameResult] = []
            _progress(progress, "download", 0, len(refs))
            for ref in refs:
                try:
                    result = await self._adownload_one(
                        ref,
                        spec,
                        output_root,
                        raw=raw,
                        overwrite=overwrite,
                        output_template=output_template,
                        context=operation_context if operation_context is not None and ref.source == operation_context.source_id else None,
                        progress=progress,
                    )
                    results.append(result)
                except asyncio.CancelledError:
                    results.append(FrameResult(ref, "cancelled", error={"code": "cancelled", "message": "operation cancelled"}))
                    results.extend(FrameResult(other, "not_started", error={"code": "not_started", "message": "cancelled before start"}) for other in refs[len(results):])
                    break
                except Exception as exc:
                    error = error_from_exception(exc, stage="process", source=ref.source).as_dict()
                    results.append(FrameResult(ref, "failed", error=error))
                    if on_error in {"raise", "stop"}:
                        partial = BatchResult(tuple(results))
                        raise BatchError("batch stopped after the first failure", partial_result=partial, cause=exc) from exc
                _progress(progress, "download", len(results), len(refs))
            return DownloadReport("download", uuid.uuid4().hex, tuple(results), query=_query_dict(query_or_refs) if isinstance(query_or_refs, Query) else None)
        finally:
            if operation_context is not None:
                operation_context.close()

    async def _adownload_one(self, ref: FrameRef, spec: ProcessingSpec, output: Path, *, raw: bool, overwrite: bool, output_template: str | None, context: SourceContext | None = None, progress: ProgressCallback | None = None) -> FrameResult:
        close_context = context is None
        if context is None:
            context = self._context(ref.source)
        raw_frame: RawFrame | None = None
        try:
            _progress(progress, "acquire", 0, 1)
            raw_frame = await self._download_raw(ref, context)
            _progress(progress, "acquire", 1, 1)
            revision = resolved_revision(raw_frame.artifacts, ref.revision)
            if spec.output_kind == "raw-only":
                value = None
            else:
                _progress(progress, "decode", 0, 1)
                value = get_source(ref.source).decode(raw_frame, context)
                value = _apply_processing(value, spec)
                context.check_value(value)
                _progress(progress, "decode", 1, 1)
            formal_raw = raw_frame if raw or spec.output_kind == "raw-only" else None
            _progress(progress, "commit", 0, 1)
            status, path = await self._commit_value(value, ref, revision, spec, output, overwrite=overwrite, raw=formal_raw, output_template=output_template)
            _progress(progress, "commit", 1, 1)
            return FrameResult(ref, status, str(path) if path else None)
        finally:
            if raw_frame is not None:
                await raw_frame.aclose()
            if close_context:
                context.close()

    def _output_target(self, output: str | Path) -> Path | _RemoteOutput:
        value = os.fspath(output)
        if value.lower().startswith(("s3://", "oss://")):
            backend = self._remote_backend
            if backend is None:
                from .storage.object import RustObjectBackend

                storage = self.config.values["storage"]
                runtime = self.config.values["runtime"]
                backend = RustObjectBackend(
                    value,
                    endpoint=storage.get("endpoint"),
                    region=storage.get("region"),
                    access_key_id=storage.get("access_key"),
                    secret_access_key=storage.get("secret_key"),
                    anonymous=bool(storage.get("anonymous", False)),
                    max_bytes=int(runtime["max_frame_bytes"]),
                )
            return _RemoteOutput(value, backend)
        if value.lower().startswith(("http://", "https://")):
            raise StorageError("remote output targets require an s3:// or oss:// URI")
        return local_output_path(output)

    async def _commit_value(self, value: Any, ref: FrameRef, revision: str, spec: ProcessingSpec, output: Path | _RemoteOutput, *, overwrite: bool, raw: RawFrame | None, output_template: str | None = None) -> tuple[str, Path | str | None]:
        from .identity import output_id as make_output_id

        if self._cancellation is not None:
            self._cancellation.check()
        oid = make_output_id(ref, revision, spec)
        if isinstance(output, _RemoteOutput):
            return await self._commit_remote_value(value, ref, revision, spec, oid, output, overwrite=overwrite, raw=raw, output_template=output_template)
        store = LocalStore(output)
        staged = store.stage(ref, oid, spec, template=output_template)
        try:
            if value is not None:
                target = staged.directory / staged.final_path.name
                encoder = encoder_for(spec.format)
                produced = encoder.writer(value, target, options=dict(spec.options))
                for item in produced:
                    item = Path(item)
                    media_type = "application/json" if item.suffix == ".json" else {
                        "netcdf": "application/x-netcdf",
                        "png": "image/png",
                        "geotiff": "image/tiff",
                        "zarr": "application/vnd+zarr",
                    }[spec.format]
                    role = "metadata" if item.suffix == ".json" else "data"
                    if item.is_dir():
                        store.track_directory(staged, item.name, item, role=role, media_type=media_type)
                    else:
                        store.write_path(staged, item.name, item, role=role, media_type=media_type)
            if self._cancellation is not None:
                self._cancellation.check()
            raw_data = raw_manifest(raw) if raw is not None else None
            status, _manifest, _skipped = store.commit(
                staged,
                logical_id=ref.logical_id,
                revision=revision,
                processing_spec=processing_identity(spec),
                processing_hash=processing_hash(spec),
                raw_complete=raw is not None,
                raw_manifest=raw_data,
                raw_artifacts=raw.artifacts if raw is not None else (),
                overwrite=overwrite,
                cancellation=self._cancellation.check if self._cancellation is not None else None,
            )
            return status, staged.final_path if status == "written" else staged.final_path
        except BaseException:
            store.abort(staged)
            raise

    async def _commit_remote_value(self, value: Any, ref: FrameRef, revision: str, spec: ProcessingSpec, oid: str, output: _RemoteOutput, *, overwrite: bool, raw: RawFrame | None, output_template: str | None) -> tuple[str, str]:
        stage_root = Path(tempfile.mkdtemp(prefix="radiust-remote-stage-"))
        store = LocalStore(stage_root)
        staged = store.stage(ref, oid, spec, template=output_template)
        try:
            if value is not None:
                target = staged.directory / staged.final_path.name
                encoder = encoder_for(spec.format)
                produced = encoder.writer(value, target, options=dict(spec.options))
                for item in produced:
                    item = Path(item)
                    media_type = "application/json" if item.suffix == ".json" else {
                        "netcdf": "application/x-netcdf",
                        "png": "image/png",
                        "geotiff": "image/tiff",
                        "zarr": "application/vnd+zarr",
                    }[spec.format]
                    role = "metadata" if item.suffix == ".json" else "data"
                    if item.is_dir():
                        store.track_directory(staged, item.name, item, role=role, media_type=media_type)
                    else:
                        store.write_path(staged, item.name, item, role=role, media_type=media_type)
            if self._cancellation is not None:
                self._cancellation.check()

            artifacts: dict[str, RemoteArtifact] = {}
            for file_info in staged.files:
                source = Path(file_info["path"])
                if file_info.get("directory"):
                    for child in sorted(source.rglob("*")):
                        if child.is_file():
                            relative = child.relative_to(source).as_posix()
                            artifacts[f"{file_info['name']}/{relative}"] = RemoteArtifact(child.read_bytes(), file_info["role"], file_info["media_type"])
                else:
                    artifacts[str(file_info["name"])] = RemoteArtifact(source.read_bytes(), file_info["role"], file_info["media_type"])
            if raw is not None:
                raw_prefix = f"raw/{staged.final_path.stem}"
                for artifact in raw.artifacts:
                    artifacts[f"{raw_prefix}/{artifact.name}"] = RemoteArtifact(artifact_bytes(artifact), artifact.role, artifact.media_type)
                raw_document = raw_manifest(raw)
                artifacts[f"{raw_prefix}/raw-manifest.json"] = RemoteArtifact(
                    json.dumps(raw_document, ensure_ascii=False, sort_keys=True, indent=2).encode("utf-8"),
                    "metadata",
                    "application/json",
                )
            relative = output_relative_location(ref, oid, spec, template=output_template)
            request = RemoteCommitRequest(
                pointer_key=output.pointer_key(relative),
                logical_id=ref.logical_id,
                revision=revision,
                output_id=oid,
                processing_spec=processing_identity(spec),
                processing_hash=processing_hash(spec),
                artifacts=artifacts,
                raw_complete=raw is not None,
                overwrite=overwrite,
            )
            result = await RemoteCommitter(output.backend).commit(
                request,
                cancellation=self._cancellation.check if self._cancellation is not None else None,
            )
            status = "written" if result.status == "committed" else result.status
            return status, output.pointer_uri(result.pointer_key)
        finally:
            shutil.rmtree(stage_root, ignore_errors=True)

    def close(self) -> None:
        if self._closed:
            return
        for stream in tuple(self._streams):
            with suppress(RuntimeError, AsyncContextError):
                self._run(stream.aclose())
        self._streams.clear()
        self._closed = True
        self._loop.close()


class AsyncClient:
    def __init__(self, *, config: EffectiveConfig | dict[str, Any] | None = None, _remote_backend: RemoteBackend | None = None):
        self.config = _as_config(config)
        self._remote_backend = _remote_backend
        self._closed = False
        self._cache: CacheStore | None = None
        self._transport = None
        self._streams: set[AsyncFetchStream] = set()
        self._workers: dict[asyncio.Task[Any], Cancellation] = {}

    async def __aenter__(self) -> AsyncClient:
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        await self.aclose()

    def _ensure_open(self) -> None:
        if self._closed:
            raise RuntimeError("Client is closed")

    def _context(self, source_id: str) -> SourceContext:
        runtime = self.config.values["runtime"]
        if self._transport is None:
            self._transport = _new_http_transport(self.config)
        return SourceContext(
            self.config,
            source_id,
            cache=self._cache_store(),
            temp_root=_source_temp_root(runtime),
            transport=self._transport,
        )

    def _cache_store(self) -> CacheStore:
        if self._cache is None:
            values = self.config.values
            try:
                self._cache = CacheStore(
                    self.config.cache_dir,
                    output_root=self.config.output_root,
                    max_bytes=int(values["cache"]["max_bytes"]),
                    max_age_days=int(values["cache"]["max_age_days"]),
                    enabled=bool(values["cache"].get("enabled", True)),
                )
            except OSError:
                self._cache = CacheStore(self.config.cache_dir, enabled=False)
        return self._cache

    async def _download_raw(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        try:
            cached = _load_cached_raw(context.cache, ref) if isinstance(context.cache, CacheStore) else None
        except (OSError, StorageError, IntegrityError):
            cached = None
        if cached is not None:
            context.metadata["cache_hit"] = True
            context.check_artifacts(cached.artifacts)
            return cached
        raw = await get_source(ref.source).download(ref, context)
        context.cancellation.check()
        context.check_artifacts(raw.artifacts)
        if isinstance(context.cache, CacheStore):
            _store_cached_raw(context.cache, ref, raw)
        return raw

    async def discover(self, query: Query) -> list[FrameRef]:
        self._ensure_open()
        context = self._context(query.source)
        try:
            return await get_source(query.source).discover(query, context)
        finally:
            context.close()

    def acquire(self, ref: FrameRef) -> _AcquireContext:
        self._ensure_open()
        return _AcquireContext(self, ref)

    async def _aacquire(self, ref: FrameRef) -> RawFrame:
        self._ensure_open()
        context = self._context(ref.source)
        try:
            raw = await self._download_raw(ref, context)
            try:
                context.check_artifacts(raw.artifacts)
            except BaseException:
                await raw.aclose()
                raise
            raw._adopt_temp_root(context.detach_temp_root())
            return raw
        finally:
            context.close()

    async def decode(self, raw: RawFrame) -> Any:
        self._ensure_open()
        context = self._context(raw.ref.source)
        try:
            value = await asyncio.to_thread(get_source(raw.ref.source).decode, raw, context)
            context.check_value(value)
            return value
        finally:
            context.close()

    async def fetch(self, query: Query) -> Any:
        refs = await self.discover(query)
        ref = _one(refs)
        async with self.acquire(ref) as raw:
            return await self.decode(raw)

    async def _run_sync_worker(self, operation: Any) -> Any:
        cancellation = Cancellation()
        config = self.config
        if self._transport is None:
            self._transport = _new_http_transport(config)
        transport = self._transport

        def run_in_worker() -> Any:
            sync = Client(config=config, _cancellation=cancellation, _remote_backend=self._remote_backend, _transport=transport)
            try:
                return operation(sync)
            finally:
                sync.close()

        task = asyncio.create_task(asyncio.to_thread(run_in_worker))
        self._workers[task] = cancellation
        try:
            return await asyncio.shield(task)
        except asyncio.CancelledError:
            cancellation.cancel()
            with suppress(BaseException):
                await asyncio.shield(task)
            raise
        finally:
            self._workers.pop(task, None)

    async def write(
        self,
        field: Any,
        *,
        output: str | Path = "./data",
        format: str = "netcdf",
        overwrite: bool = False,
        ref: FrameRef | None = None,
        raw: bool = False,
        **options: Any,
    ) -> DownloadReport:
        self._ensure_open()
        if raw:
            raise ValueError("write accepts decoded values only; use download(..., raw=True)")
        if ref is None:
            raise ValueError("write requires the source FrameRef used to produce the value")

        return await self._run_sync_worker(
            lambda sync: sync.write(field, output=output, format=format, overwrite=overwrite, ref=ref, **options)
        )

    async def fetch_many(
        self,
        query_or_refs: BatchInput | Iterable[BatchInput],
        *,
        on_error: str = "collect",
        max_concurrency: int | None = None,
        progress: ProgressCallback | None = None,
    ) -> BatchResult:
        self._ensure_open()
        return await fetch_many_async(self, query_or_refs, on_error=on_error, max_concurrency=max_concurrency, progress=progress)

    def aiter_fetch(
        self,
        query_or_refs: BatchInput | Iterable[BatchInput],
        *,
        on_error: str = "collect",
        max_prefetch: int | None = None,
    ) -> AsyncFetchStream:
        self._ensure_open()
        return AsyncFetchStream(self, query_or_refs, on_error=on_error, max_prefetch=max_prefetch)

    def _register_stream(self, stream: AsyncFetchStream) -> None:
        self._streams.add(stream)

    def _unregister_stream(self, stream: AsyncFetchStream) -> None:
        self._streams.discard(stream)

    async def download(
        self,
        query_or_refs: BatchInput | Iterable[BatchInput],
        *,
        output: str | Path = "./data",
        format: str = "netcdf",
        raw: bool = False,
        raw_only: bool = False,
        overwrite: bool = False,
        output_template: str | None = None,
        on_error: str = "collect",
        config: EffectiveConfig | None = None,
        progress: ProgressCallback | None = None,
        **processing: Any,
    ) -> DownloadReport:
        self._ensure_open()
        # Reuse the same pipeline while retaining the caller's async event loop.
        if config is not None and config is not self.config:
            self.config = _as_config(config)
            self._cache = None
            self._transport = None

        return await self._run_sync_worker(
            lambda sync: sync._run(
                sync._adownload(
                    query_or_refs,
                    output=output,
                    format=format,
                    raw=raw,
                    raw_only=raw_only,
                    overwrite=overwrite,
                    output_template=output_template,
                    on_error=on_error,
                    processing=processing,
                    progress=progress,
                )
            )
        )

    async def aclose(self) -> None:
        if self._closed:
            return
        streams = tuple(self._streams)
        for stream in streams:
            await stream.aclose()
        self._streams.clear()
        workers = tuple(self._workers.items())
        for _task, cancellation in workers:
            cancellation.cancel()
        if workers:
            await asyncio.gather(*(task for task, _cancellation in workers), return_exceptions=True)
        self._workers.clear()
        self._closed = True


def _one(refs: list[FrameRef]) -> FrameRef:
    return select_one(refs)


def _apply_processing(value: Any, spec: ProcessingSpec) -> Any:
    from .field import RadarDataset, RadarField

    if spec.variable is not None:
        if not isinstance(value, (RadarField, RadarDataset)):
            raise TypeError("variable selection requires RadarField or RadarDataset")
        value = value.select(spec.variable)
    if spec.grid != "geographic":
        return value
    if spec.bbox is None or spec.resolution is None:
        raise ValueError("geographic processing requires bbox and resolution")
    if isinstance(value, RadarField):
        return value.to_geographic(bbox=spec.bbox, resolution=spec.resolution, method=spec.resampling)
    if isinstance(value, RadarDataset):
        return value.to_geographic(bbox=spec.bbox, resolution=spec.resolution, method=spec.resampling)
    raise TypeError("geographic processing requires RadarField or RadarDataset")


def _query_dict(query: Query) -> dict[str, Any]:
    return {"source": query.source, "product": query.product, "stations": list(query.stations), "latest": query.latest, "at": query.at.isoformat() if query.at else None, "start": query.start.isoformat() if query.start else None, "end": query.end.isoformat() if query.end else None}
