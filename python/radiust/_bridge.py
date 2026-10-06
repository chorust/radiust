"""Private facade around the optional compiled Rust extension."""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
from collections.abc import Mapping
from pathlib import Path
from typing import Any

try:
    from . import _core
except ImportError:  # source-tree development before a wheel is built
    _core = None

from .errors import (
    ConfigError,
    DecodeError,
    ErrorContext,
    GridError,
    IntegrityError,
    OperationCancelled,
    RadiustError,
    ResourceLimitError,
    StorageError,
    UnsupportedQueryError,
)


def version() -> str:
    return _core.version() if _core is not None else "python-fallback"


def _native_error_context(exc: Exception, stage: str, *, source: str | None = None) -> ErrorContext:
    native_stage = getattr(exc, "radiust_stage", stage)
    native_code = getattr(exc, "radiust_code", None)
    native_retryable = getattr(exc, "radiust_retryable", False)
    return ErrorContext(
        stage=str(native_stage),
        source=source,
        retryable=bool(native_retryable),
        code=str(native_code) if native_code else None,
    )


def _raise_if_cancelled(exc: Exception, stage: str) -> None:
    context = _native_error_context(exc, stage)
    if context.code == "cancelled":
        raise OperationCancelled(
            "operation cancelled", context=context, cause=exc
        ) from exc


def sha256(data: bytes) -> str:
    return _core.sha256(data) if _core is not None else hashlib.sha256(data).hexdigest()


_DIRECT_DBZ_PATHS = {
    ("rainviewer", "composite"),
    ("tw", "grid"),
    ("rdcap", "reflectivity"),
}


def _legacy_direct_dbz_fallback_allowed(
    source: str,
    product: str,
    *,
    extension: Any = None,
    required_method: str = "decode_dbz",
) -> bool:
    """Allow an older extension only when a direct native mode is absent.

    This predicate never invokes the missing capability, so a runtime failure
    from an available method cannot be mistaken for a compatibility fallback.
    """
    target = _core if extension is None else extension
    return (source, product) in _DIRECT_DBZ_PATHS and not callable(
        getattr(target, required_method, None)
    )


def _decode_gray_array_native(
    width: int,
    height: int,
    channels: int,
    values: list[float],
    *,
    alpha_bit_depth: int = 0,
    historical: bool = False,
    strict: bool = True,
    max_gray: int = 224,
) -> tuple[list[float], list[int]]:
    if _core is None:
        raise UnsupportedQueryError(
            "the compiled Rust extension is required for gray decoding",
            context=ErrorContext(stage="validate", code="unsupported"),
        )
    method_name = "decode_gray_array_historical" if historical else "decode_gray_array"
    method = getattr(_core, method_name, None)
    if not callable(method):
        raise UnsupportedQueryError(
            f"the installed Rust extension does not support {method_name}",
            context=ErrorContext(stage="validate", code="unsupported"),
        )
    try:
        if historical:
            return method(width, height, channels, values, strict, max_gray)
        return method(width, height, channels, values, alpha_bit_depth)
    except (OSError, ValueError) as exc:
        _raise_if_cancelled(exc, "decode")
        context = _native_error_context(exc, "decode")
        if context.code == "resource_limit":
            raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
        if context.code == "unsupported":
            raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
        raise DecodeError(str(exc), context=context, cause=exc) from exc


def validate_size(size: int, limit: int) -> int:
    try:
        if _core is not None:
            return int(_core.validate_size(size, limit))
    except ValueError as exc:
        raise ResourceLimitError(
            str(exc), context=ErrorContext(stage="acquire"), cause=exc
        ) from exc
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
        raise StorageError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc


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
        raise StorageError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc


async def sleep(seconds: float) -> None:
    """Await a Rust future when available, retaining a source-tree fallback."""
    if _core is not None and hasattr(_core, "sleep"):
        await _core.sleep(float(seconds))
    else:
        await asyncio.sleep(seconds)


def native_source_catalog() -> dict[str, Any]:
    """Read the embedded Rust catalog when the extension is installed."""
    if _core is None or not hasattr(_core, "source_catalog_json"):
        raise StorageError(
            "the compiled Rust source catalog is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return json.loads(_core.source_catalog_json())


def _native_query_payload(query: Any) -> dict[str, Any]:
    """Convert public Query objects and mapping inputs to the core JSON shape."""
    if isinstance(query, Mapping):
        values = dict(query)
    else:
        values = {
            name: getattr(query, name)
            for name in (
                "source",
                "sources",
                "product",
                "stations",
                "latest",
                "at",
                "start",
                "end",
                "base_time",
                "max_age",
            )
            if hasattr(query, name)
        }

    selector = values.get("selector")
    if not isinstance(selector, Mapping):
        at = values.pop("at", None)
        start = values.pop("start", None)
        end = values.pop("end", None)
        values.pop("latest", None)
        if at is not None:
            selector = {"kind": "at", "time": at}
        elif start is not None or end is not None:
            selector = {"kind": "range", "start": start, "end": end}
        else:
            selector = {"kind": "latest"}
    else:
        selector = dict(selector)

    def time_text(value: Any) -> Any:
        return value.isoformat() if callable(getattr(value, "isoformat", None)) else value

    selector = {key: time_text(value) for key, value in selector.items()}
    max_age = values.pop("max_age", values.get("max_age_secs"))
    if callable(getattr(max_age, "total_seconds", None)):
        max_age = max_age.total_seconds()
    payload = {
        key: values[key] for key in ("source", "sources", "product", "stations") if key in values
    }
    payload["selector"] = selector
    payload["base_time"] = time_text(values.get("base_time"))
    payload["max_age_secs"] = max_age
    return payload


def _json_safe(value: Any) -> Any:
    if isinstance(value, Path):
        return os.fspath(value)
    if isinstance(value, Mapping):
        return {str(key): _json_safe(child) for key, child in value.items()}
    if isinstance(value, (tuple, list)):
        return [_json_safe(child) for child in value]
    if callable(getattr(value, "isoformat", None)):
        return value.isoformat()
    return value


def redact_config_values(values: Mapping[str, Any]) -> dict[str, Any]:
    """Apply Rust's configuration secret-redaction rules to SDK values."""
    if _core is None or not hasattr(_core, "redact_config_values_json"):
        raise ConfigError(
            "the compiled Rust config binding is unavailable",
            context=ErrorContext(stage="validate"),
        )
    try:
        payload = json.dumps(_json_safe(dict(values)), ensure_ascii=False, allow_nan=False)
        redacted = json.loads(_core.redact_config_values_json(payload))
        if not isinstance(redacted, dict):
            raise ValueError("Rust returned invalid redacted configuration values")
        return redacted
    except (TypeError, ValueError) as exc:
        raise ConfigError(
            str(exc) or "configuration values could not be redacted",
            context=ErrorContext(stage="validate"),
            cause=exc,
        ) from exc


def resolve_config(
    overrides: Mapping[str, Any] | None = None,
    *,
    path: str | os.PathLike[str] | None = None,
    environ: Mapping[str, str] | None = None,
) -> tuple[Any, dict[str, Any], dict[str, str], dict[str, Any]]:
    """Resolve SDK configuration in Rust and return its native handle and views."""
    if _core is None or not hasattr(_core, "resolve_config"):
        raise ConfigError(
            "the compiled Rust config binding is unavailable",
            context=ErrorContext(stage="validate"),
        )

    try:
        overrides_json = None
        if overrides:
            overrides_json = json.dumps(
                _json_safe(dict(overrides)), ensure_ascii=False, allow_nan=False
            )
        source_environment = os.environ if environ is None else environ
        environment_items: list[tuple[str, str]] = []
        for key, value in source_environment.items():
            if not isinstance(key, str) or not isinstance(value, str):
                raise TypeError("configuration environment must contain string keys and values")
            if key == "HOME" or key.startswith("RADIUST_"):
                environment_items.append((key, value))
        environment_json = json.dumps(environment_items, ensure_ascii=False)
        path_text = None if path is None else os.path.expanduser(os.fspath(path))
        native = _core.resolve_config(overrides_json, path_text, environment_json)
        values = json.loads(native.values_json())
        origins = json.loads(native.origins_json())
        redacted = json.loads(native.redacted_json())
        if (
            not isinstance(values, dict)
            or not isinstance(origins, dict)
            or not isinstance(redacted, dict)
        ):
            raise ValueError("Rust returned an invalid configuration result")
        return native, values, origins, redacted
    except ConfigError:
        raise
    except (TypeError, ValueError, OSError) as exc:
        message = str(exc) or "configuration could not be resolved"
        raise ConfigError(
            message,
            context=ErrorContext(stage="validate"),
            cause=exc,
        ) from exc


class CoreEngineSession:
    """A persistent Python handle to one reusable Rust Engine instance.

    Keep a session for a client's lifetime so the Tokio runtime, HTTP transport,
    source registry, and cancellation state are shared across calls.
    """

    def __init__(self, config: Any):
        if _core is None or not hasattr(_core, "Engine"):
            raise StorageError(
                "the compiled Rust Engine is unavailable",
                context=ErrorContext(stage="validate"),
            )
        try:
            native_config = getattr(config, "_native_config", None)
            resolved_values = getattr(config, "_resolved_values_snapshot", None)
            config_values = (
                config if isinstance(config, Mapping) else getattr(config, "values", config)
            )
            if native_config is not None and resolved_values == config_values:
                self._engine = _core.Engine.from_resolved_config(native_config)
            else:
                config_json = json.dumps(
                    _json_safe(dict(config_values)), ensure_ascii=False, allow_nan=False
                )
                self._engine = _core.Engine(config_json)
        except (TypeError, ValueError) as exc:
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    @property
    def cancelled(self) -> bool:
        return bool(self._engine.cancelled)

    def cancel(self) -> None:
        self._engine.cancel()

    def subscribe_events(self) -> Any:
        """Subscribe to bounded, redacted lifecycle events for this engine."""
        return self._engine.subscribe_events()

    async def discover_report(self, query: Any) -> Any:
        try:
            native_query = _core.Query(json.dumps(_native_query_payload(query), ensure_ascii=False))
        except (TypeError, ValueError) as exc:
            raise UnsupportedQueryError(
                str(exc), context=ErrorContext(stage="validate"), cause=exc
            ) from exc
        try:
            return await self._engine.discover(native_query)
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "discover")
            native_code = getattr(exc, "radiust_code", None)
            error_type = RadiustError if native_code else ConfigError
            raise error_type(
                str(exc), context=_native_error_context(exc, "discover"), cause=exc
            ) from exc

    async def fetch_raw(self, frame: Any) -> Any:
        native_frame = _native_frames([frame])[0]
        try:
            return await self._engine.fetch_raw(native_frame)
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "acquire")
            raise UnsupportedQueryError(
                str(exc), context=_native_error_context(exc, "acquire"), cause=exc
            ) from exc

    async def load_raw_manifest(self, manifest_path: str | os.PathLike[str]) -> Any:
        try:
            return await self._engine.load_raw_manifest(os.fspath(manifest_path))
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "validate")
            raise IntegrityError(
                "raw manifest or retained artifacts failed Rust validation",
                context=_native_error_context(exc, "validate"),
                cause=exc,
            ) from exc

    async def decode_science(self, raw_frame: Any) -> Any:
        try:
            return await self._engine.decode_science(raw_frame)
        except ValueError as exc:
            _raise_if_cancelled(exc, "decode")
            message = str(exc)
            context = _native_error_context(exc, "decode")
            if "resource limit" in message:
                raise ResourceLimitError(message, context=context, cause=exc) from exc
            if "scientific decoding is not available" in message:
                raise UnsupportedQueryError(
                    message, context=context, cause=exc
                ) from exc
            raise DecodeError(message, context=context, cause=exc) from exc
        except OSError as exc:
            _raise_if_cancelled(exc, "decode")
            raise DecodeError(
                str(exc), context=_native_error_context(exc, "decode"), cause=exc
            ) from exc

    async def decode_gray(self, raw_frame: Any) -> Any:
        method = getattr(self._engine, "decode_gray", None)
        if not callable(method):
            raise UnsupportedQueryError(
                "the installed Rust extension does not support source gray decoding",
                context=ErrorContext(stage="validate", code="unsupported"),
            )
        try:
            return await method(raw_frame)
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "decode")
            context = _native_error_context(exc, "decode")
            if context.code == "resource_limit":
                raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
            if context.code == "unsupported":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            if context.code == "integrity":
                raise IntegrityError(str(exc), context=context, cause=exc) from exc
            raise DecodeError(str(exc), context=context, cause=exc) from exc

    async def decode_dbz(self, raw_frame: Any) -> Any:
        method = getattr(self._engine, "decode_dbz", None)
        if callable(method):
            try:
                return await method(raw_frame)
            except (OSError, ValueError) as exc:
                _raise_if_cancelled(exc, "decode")
                context = _native_error_context(exc, "decode")
                if context.code == "resource_limit":
                    raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
                if context.code == "unsupported":
                    raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
                if context.code == "integrity":
                    raise IntegrityError(str(exc), context=context, cause=exc) from exc
                raise DecodeError(str(exc), context=context, cause=exc) from exc

        frame = raw_frame.frame()
        source = str(frame.source)
        product = str(frame.product)
        if _legacy_direct_dbz_fallback_allowed(
            source, product, extension=type(self._engine), required_method="decode_dbz"
        ):
            native = await self.decode_science(raw_frame)
            if getattr(native, "name", None) == "reflectivity" and getattr(native, "units", None) == "dBZ":
                return native
            raise DecodeError(
                "legacy native decoder did not return reflectivity in dBZ",
                context=ErrorContext(stage="decode", code="unit_mismatch", source=source),
            )
        raise UnsupportedQueryError(
            "the installed Rust extension does not support source dBZ decoding",
            context=ErrorContext(stage="validate", code="unsupported", source=source),
        )

    async def decode_gray_file(
        self, path: str | os.PathLike[str], frame_index: int | None = None
    ) -> Any:
        method = getattr(self._engine, "decode_gray_file", None)
        if not callable(method):
            raise UnsupportedQueryError(
                "the installed Rust extension does not support local gray dBZ decoding",
                context=ErrorContext(stage="validate", code="unsupported"),
            )
        try:
            return await method(os.fspath(path), frame_index)
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "decode")
            context = _native_error_context(exc, "decode")
            if context.code == "resource_limit":
                raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
            if context.code == "unsupported":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            raise DecodeError(str(exc), context=context, cause=exc) from exc

    async def read_dbz_file(
        self,
        path: str | os.PathLike[str],
        *,
        variable: str | None = None,
        valid_time: str | None = None,
    ) -> Any:
        method = getattr(self._engine, "read_dbz_file", None)
        if not callable(method):
            raise UnsupportedQueryError(
                "the installed Rust extension does not support receipt-bound numeric dBZ reads",
                context=ErrorContext(stage="validate", code="unsupported"),
            )
        try:
            return await method(os.fspath(path), variable, valid_time)
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "decode")
            context = _native_error_context(exc, "decode")
            if context.code == "resource_limit":
                raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
            if context.code == "unsupported":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            if context.code == "unit_mismatch":
                raise DecodeError(str(exc), context=context, cause=exc) from exc
            if context.code == "integrity":
                raise IntegrityError(str(exc), context=context, cause=exc) from exc
            raise DecodeError(str(exc), context=context, cause=exc) from exc

    async def write_raster(
        self,
        result: Any,
        *,
        output_name: str,
        format: str,
        options_json: str,
        overwrite: bool,
        output_root: str | os.PathLike[str] | None = None,
        ref: Any = None,
    ) -> dict[str, Any]:
        method = getattr(self._engine, "write_raster_to", None)
        if not callable(method):
            raise UnsupportedQueryError(
                "the installed Rust extension does not support receipt-bound raster writes",
                context=ErrorContext(stage="validate", code="unsupported"),
            )
        try:
            ref_json = None
            if ref is not None:
                native_ref = _native_frames([ref])[0]
                ref_json = native_ref.to_json()
            encoded = await method(
                result,
                output_name,
                format,
                options_json,
                bool(overwrite),
                None if output_root is None else os.fspath(output_root),
                ref_json,
            )
            payload = json.loads(encoded)
            if not isinstance(payload, dict) or payload.get("status") not in {"written", "skipped"}:
                raise ValueError("Rust returned an invalid raster write report")
            return payload
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "commit")
            context = _native_error_context(exc, "commit")
            if context.code == "resource_limit":
                raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
            if context.code == "output_conflict":
                raise StorageError(str(exc), context=context, cause=exc) from exc
            if context.code == "invalid_grid":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            if context.code == "unsupported":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            raise StorageError(str(exc), context=context, cause=exc) from exc

    async def decode_gray_values(
        self,
        width: int,
        height: int,
        values: Any,
        *,
        alpha_json: str | None = None,
        declared_encoding: str = "gray-dbz-v1",
    ) -> Any:
        method = getattr(self._engine, "decode_gray_values", None)
        if not callable(method):
            raise UnsupportedQueryError(
                "the installed Rust extension does not support strict gray array decoding",
                context=ErrorContext(stage="validate", code="unsupported"),
            )
        try:
            return await method(width, height, values, alpha_json, declared_encoding)
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "decode")
            context = _native_error_context(exc, "decode")
            if context.code == "resource_limit":
                raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
            if context.code == "unsupported":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            raise DecodeError(str(exc), context=context, cause=exc) from exc

    async def replay_raw_manifest(
        self, manifest_path: str | os.PathLike[str], *, mode: str | None = None
    ) -> Any:
        try:
            method = self._engine.replay_raw_manifest
            if mode is None:
                return await method(os.fspath(manifest_path))
            if mode not in {"gray", "dbz"}:
                raise ValueError("mode must be gray or dbz")
            try:
                return await method(os.fspath(manifest_path), mode)
            except TypeError as exc:
                raise UnsupportedQueryError(
                    f"the installed Rust extension does not support replay mode={mode}",
                    context=ErrorContext(stage="validate", code="unsupported"),
                    cause=exc,
                ) from exc
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "decode" if mode else "validate")
            context = _native_error_context(exc, "decode" if mode else "validate")
            if context.code == "integrity" or (mode is None and context.code is None):
                raise IntegrityError(
                    "raw manifest or retained artifacts failed Rust validation",
                    context=context,
                    cause=exc,
                ) from exc
            if context.code == "resource_limit":
                raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
            if context.code == "unsupported":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            if context.code in {"invalid_gray_encoding", "unit_mismatch", "decode_unverified"}:
                raise DecodeError(str(exc), context=context, cause=exc) from exc
            raise DecodeError(str(exc), context=context, cause=exc) from exc

    async def fetch_many_raw(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        dry_run: bool = False,
        max_concurrency: int | None = None,
    ) -> Any:
        native_frames = _native_frames(frames)
        try:
            return await self._engine.fetch_many_raw(
                native_frames, on_error, bool(dry_run), max_concurrency
            )
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    async def fetch_many_decoded(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        dry_run: bool = False,
        max_concurrency: int | None = None,
    ) -> Any:
        native_frames = _native_frames(frames)
        try:
            return await self._engine.fetch_many_decoded(
                native_frames, on_error, bool(dry_run), max_concurrency
            )
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    async def fetch_many_mode(
        self,
        frames: list[Any],
        *,
        mode: str,
        on_error: str = "collect",
        dry_run: bool = False,
        max_concurrency: int | None = None,
    ) -> Any:
        if mode not in {"gray", "dbz"}:
            raise ValueError("mode must be gray or dbz")
        method = getattr(self._engine, "fetch_many_mode", None)
        if not callable(method):
            raise UnsupportedQueryError(
                "the installed Rust extension does not support gray/dbz batch decoding",
                context=ErrorContext(stage="validate", code="unsupported"),
            )
        native_frames = _native_frames(frames)
        try:
            return await method(native_frames, mode, on_error, bool(dry_run), max_concurrency)
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "decode")
            context = _native_error_context(exc, "decode")
            if context.code == "resource_limit":
                raise ResourceLimitError(str(exc), context=context, cause=exc) from exc
            if context.code == "unsupported":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            raise DecodeError(str(exc), context=context, cause=exc) from exc

    def open_fetch_mode_stream(
        self,
        frames: list[Any],
        *,
        mode: str,
        on_error: str = "collect",
        max_concurrency: int | None = None,
    ) -> Any:
        if mode not in {"gray", "dbz"}:
            raise ValueError("mode must be gray or dbz")
        method = getattr(self._engine, "open_fetch_mode_stream", None)
        if not callable(method):
            raise UnsupportedQueryError(
                "the installed Rust extension does not support gray/dbz fetch streams",
                context=ErrorContext(stage="validate", code="unsupported"),
            )
        try:
            return method(_native_frames(frames), mode, on_error, max_concurrency)
        except (OSError, ValueError) as exc:
            _raise_if_cancelled(exc, "decode")
            context = _native_error_context(exc, "decode")
            if context.code == "unsupported":
                raise UnsupportedQueryError(str(exc), context=context, cause=exc) from exc
            raise DecodeError(str(exc), context=context, cause=exc) from exc

    def open_fetch_stream(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        max_concurrency: int | None = None,
    ) -> Any:
        native_frames = _native_frames(frames)
        try:
            return self._engine.open_fetch_stream(native_frames, on_error, max_concurrency)
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    async def download_raw_only(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        dry_run: bool = False,
        overwrite: bool = False,
        output_root: str | os.PathLike[str] | None = None,
    ) -> Any:
        native_frames = _native_frames(frames)
        try:
            return await self._engine.download_raw_only(
                native_frames,
                on_error,
                bool(dry_run),
                bool(overwrite),
                os.fspath(output_root) if output_root is not None else None,
            )
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    async def download_dbz_mode(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        dry_run: bool = False,
        overwrite: bool = False,
        output_root: str | os.PathLike[str] | None = None,
        format: str = "netcdf",
        include_raw: bool = False,
    ) -> Any:
        native_frames = _native_frames(frames)
        method = getattr(self._engine, "download_dbz_mode", None)
        if not callable(method):
            raise UnsupportedQueryError(
                "the installed Rust extension does not support source dBZ downloads",
                context=ErrorContext(stage="validate"),
            )
        try:
            return await method(
                native_frames,
                on_error,
                bool(dry_run),
                bool(overwrite),
                os.fspath(output_root) if output_root is not None else None,
                format,
                bool(include_raw),
            )
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    async def download_png(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        dry_run: bool = False,
        overwrite: bool = False,
        output_root: str | os.PathLike[str] | None = None,
        output_template: str | None = None,
        include_raw: bool = False,
        processing: Mapping[str, Any] | None = None,
    ) -> Any:
        native_frames = _native_frames(frames)
        try:
            return await self._engine.download_png(
                native_frames,
                on_error,
                bool(dry_run),
                bool(overwrite),
                os.fspath(output_root) if output_root is not None else None,
                output_template,
                bool(include_raw),
                json.dumps(processing or {}),
            )
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    async def download_netcdf(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        dry_run: bool = False,
        overwrite: bool = False,
        output_root: str | os.PathLike[str] | None = None,
        output_template: str | None = None,
        include_raw: bool = False,
        processing: Mapping[str, Any] | None = None,
    ) -> Any:
        native_frames = _native_frames(frames)
        try:
            return await self._engine.download_netcdf(
                native_frames,
                on_error,
                bool(dry_run),
                bool(overwrite),
                os.fspath(output_root) if output_root is not None else None,
                output_template,
                bool(include_raw),
                json.dumps(processing or {}),
            )
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    async def download_geotiff(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        dry_run: bool = False,
        overwrite: bool = False,
        output_root: str | os.PathLike[str] | None = None,
        output_template: str | None = None,
        include_raw: bool = False,
        processing: Mapping[str, Any] | None = None,
    ) -> Any:
        native_frames = _native_frames(frames)
        try:
            return await self._engine.download_geotiff(
                native_frames,
                on_error,
                bool(dry_run),
                bool(overwrite),
                os.fspath(output_root) if output_root is not None else None,
                output_template,
                bool(include_raw),
                json.dumps(processing or {}),
            )
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc

    async def download_zarr(
        self,
        frames: list[Any],
        *,
        on_error: str = "collect",
        dry_run: bool = False,
        overwrite: bool = False,
        output_root: str | os.PathLike[str] | None = None,
        output_template: str | None = None,
        include_raw: bool = False,
        processing: Mapping[str, Any] | None = None,
    ) -> Any:
        native_frames = _native_frames(frames)
        try:
            return await self._engine.download_zarr(
                native_frames,
                on_error,
                bool(dry_run),
                bool(overwrite),
                os.fspath(output_root) if output_root is not None else None,
                output_template,
                bool(include_raw),
                json.dumps(processing or {}),
            )
        except ValueError as exc:
            if "on_error" in str(exc):
                raise UnsupportedQueryError(
                    str(exc), context=ErrorContext(stage="validate"), cause=exc
                ) from exc
            raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc


async def native_discover(config: Mapping[str, Any], query: Mapping[str, Any]) -> dict[str, Any]:
    """Run the Rust Engine discovery API and decode its stable JSON report."""
    if _core is None or not hasattr(_core, "discover_json"):
        raise StorageError(
            "the compiled Rust discovery engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    if hasattr(_core, "Engine") and hasattr(_core, "Query"):
        report = await native_discover_report(config, query)
        return json.loads(report.to_json())

    config_json = json.dumps(dict(config), ensure_ascii=False)
    query_json = json.dumps(dict(query), ensure_ascii=False)
    try:
        if hasattr(_core, "query_validate_json"):
            _core.query_validate_json(query_json)
    except ValueError as exc:
        raise UnsupportedQueryError(
            str(exc), context=ErrorContext(stage="validate"), cause=exc
        ) from exc
    try:
        return json.loads(await _core.discover_json(config_json, query_json))
    except ValueError as exc:
        raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc


async def native_discover_report(config: Mapping[str, Any], query: Mapping[str, Any]) -> Any:
    """Return the typed Rust report so frame references retain private locators."""
    if _core is None or not hasattr(_core, "Engine") or not hasattr(_core, "Query"):
        raise StorageError(
            "the typed Rust discovery engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).discover_report(query)


async def native_fetch_many_raw(
    config: Mapping[str, Any],
    frames: list[Any],
    *,
    on_error: str = "collect",
    dry_run: bool = False,
    max_concurrency: int | None = None,
) -> Any:
    """Fetch an ordered batch through the Rust Engine and preserve partial results."""
    if _core is None or not hasattr(_core, "Engine") or not hasattr(_core.Engine, "fetch_many_raw"):
        raise StorageError(
            "the Rust batch acquisition engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).fetch_many_raw(
        frames,
        on_error=on_error,
        dry_run=dry_run,
        max_concurrency=max_concurrency,
    )


async def native_fetch_many_decoded(
    config: Mapping[str, Any],
    frames: list[Any],
    *,
    on_error: str = "collect",
    dry_run: bool = False,
    max_concurrency: int | None = None,
) -> Any:
    """Acquire and decode an ordered batch entirely in the Rust Engine."""
    if (
        _core is None
        or not hasattr(_core, "Engine")
        or not hasattr(_core.Engine, "fetch_many_decoded")
    ):
        raise StorageError(
            "the Rust batch decode engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).fetch_many_decoded(
        frames,
        on_error=on_error,
        dry_run=dry_run,
        max_concurrency=max_concurrency,
    )


def _native_frames(frames: list[Any]) -> list[Any]:
    native_frames = []
    for frame in frames:
        if hasattr(frame, "to_json") and hasattr(frame, "source"):
            native_frames.append(frame)
        elif isinstance(frame, Mapping):
            try:
                native_frames.append(
                    _core.FrameRef(json.dumps(_json_safe(dict(frame)), ensure_ascii=False))
                )
            except (TypeError, ValueError) as exc:
                raise UnsupportedQueryError(
                    "frame data cannot be converted to a native FrameRef",
                    context=ErrorContext(stage="validate"),
                    cause=exc,
                ) from exc
        elif all(
            hasattr(frame, name)
            for name in ("source", "product", "station", "valid_time", "logical_id")
        ):

            def time_text(value: Any) -> Any:
                return value.isoformat() if callable(getattr(value, "isoformat", None)) else value

            payload = {
                "source": frame.source,
                "product": frame.product,
                "station": frame.station,
                "valid_time": time_text(frame.valid_time),
                "base_time": time_text(getattr(frame, "base_time", None)),
                "logical_id": frame.logical_id,
                "revision": getattr(frame, "revision", None),
                "locator_version": getattr(frame, "locator_version", "1"),
                "locator": _json_safe(getattr(frame, "locator", {})),
            }
            try:
                native_frames.append(_core.FrameRef(json.dumps(payload, ensure_ascii=False)))
            except (TypeError, ValueError) as exc:
                raise UnsupportedQueryError(
                    "frame data cannot be converted to a native FrameRef",
                    context=ErrorContext(stage="validate"),
                    cause=exc,
                ) from exc
        else:
            raise TypeError("frames must contain native FrameRef objects or frame mappings")
    return native_frames


async def native_download_raw_only(
    config: Mapping[str, Any],
    frames: list[Any],
    *,
    on_error: str = "collect",
    dry_run: bool = False,
    overwrite: bool = False,
) -> Any:
    """Commit raw artifacts through the reusable Rust Engine."""
    if (
        _core is None
        or not hasattr(_core, "Engine")
        or not hasattr(_core.Engine, "download_raw_only")
    ):
        raise StorageError(
            "the Rust raw download engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).download_raw_only(
        frames, on_error=on_error, dry_run=dry_run, overwrite=overwrite
    )


async def native_download_png(
    config: Mapping[str, Any],
    frames: list[Any],
    *,
    on_error: str = "collect",
    dry_run: bool = False,
    overwrite: bool = False,
    include_raw: bool = False,
    processing: Mapping[str, Any] | None = None,
) -> Any:
    """Decode only validated science and commit PNG artifacts in Rust."""
    if _core is None or not hasattr(_core, "Engine") or not hasattr(_core.Engine, "download_png"):
        raise StorageError(
            "the Rust PNG download engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).download_png(
        frames,
        on_error=on_error,
        dry_run=dry_run,
        overwrite=overwrite,
        include_raw=include_raw,
        processing=processing,
    )


async def native_download_netcdf(
    config: Mapping[str, Any],
    frames: list[Any],
    *,
    on_error: str = "collect",
    dry_run: bool = False,
    overwrite: bool = False,
    include_raw: bool = False,
    processing: Mapping[str, Any] | None = None,
) -> Any:
    """Decode validated science and commit NetCDF4 artifacts in Rust."""
    if (
        _core is None
        or not hasattr(_core, "Engine")
        or not hasattr(_core.Engine, "download_netcdf")
    ):
        raise StorageError(
            "the Rust NetCDF download engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).download_netcdf(
        frames,
        on_error=on_error,
        dry_run=dry_run,
        overwrite=overwrite,
        include_raw=include_raw,
        processing=processing,
    )


async def native_download_geotiff(
    config: Mapping[str, Any],
    frames: list[Any],
    *,
    on_error: str = "collect",
    dry_run: bool = False,
    overwrite: bool = False,
    include_raw: bool = False,
    processing: Mapping[str, Any] | None = None,
) -> Any:
    """Decode validated science and commit GeoTIFF artifact groups in Rust."""
    if (
        _core is None
        or not hasattr(_core, "Engine")
        or not hasattr(_core.Engine, "download_geotiff")
    ):
        raise StorageError(
            "the Rust GeoTIFF download engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).download_geotiff(
        frames,
        on_error=on_error,
        dry_run=dry_run,
        overwrite=overwrite,
        include_raw=include_raw,
        processing=processing,
    )


async def native_download_zarr(
    config: Mapping[str, Any],
    frames: list[Any],
    *,
    on_error: str = "collect",
    dry_run: bool = False,
    overwrite: bool = False,
    include_raw: bool = False,
    processing: Mapping[str, Any] | None = None,
) -> Any:
    """Decode validated science and commit Zarr v2 stores in Rust."""
    if _core is None or not hasattr(_core, "Engine") or not hasattr(_core.Engine, "download_zarr"):
        raise StorageError(
            "the Rust Zarr download engine is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).download_zarr(
        frames,
        on_error=on_error,
        dry_run=dry_run,
        overwrite=overwrite,
        include_raw=include_raw,
        processing=processing,
    )


async def native_decode_science(config: Mapping[str, Any], raw_frame: Any) -> Any:
    """Decode a Rust-owned raw frame through the source's verified Rust decoder."""
    if _core is None or not hasattr(_core, "Engine") or not hasattr(_core.Engine, "decode_science"):
        raise StorageError(
            "the Rust scientific decoder is unavailable",
            context=ErrorContext(stage="validate"),
        )
    return await CoreEngineSession(config).decode_science(raw_frame)


async def native_regrid(
    config: Mapping[str, Any],
    field: Any,
    target_grid: Mapping[str, Any],
    *,
    method: str = "nearest",
) -> Any:
    """Regrid a regular Rust RadarField in a bounded core worker."""
    if _core is None or not hasattr(_core, "Engine") or not hasattr(_core.Engine, "regrid"):
        raise StorageError(
            "the Rust regridding operation is unavailable",
            context=ErrorContext(stage="validate"),
        )
    if method not in {"nearest", "bilinear"}:
        raise GridError("resampling must be nearest or bilinear")
    try:
        native_field = (
            field
            if isinstance(field, _core.RadarField)
            else _core.RadarField(json.dumps(dict(field), ensure_ascii=False))
        )
        engine = _core.Engine(json.dumps(dict(config), ensure_ascii=False))
    except (TypeError, ValueError) as exc:
        raise ConfigError(str(exc), context=ErrorContext(stage="validate"), cause=exc) from exc
    target_json = json.dumps(dict(target_grid), ensure_ascii=False)
    try:
        return await engine.regrid(native_field, target_json, method)
    except ValueError as exc:
        message = str(exc)
        if "resource limit" in message:
            raise ResourceLimitError(
                message, context=ErrorContext(stage="process"), cause=exc
            ) from exc
        if "resampling must" in message:
            raise GridError(message, context=ErrorContext(stage="validate"), cause=exc) from exc
        raise GridError(message, context=ErrorContext(stage="process"), cause=exc) from exc
    except OSError as exc:
        raise GridError(str(exc), context=ErrorContext(stage="process"), cause=exc) from exc


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
        raise ResourceLimitError(
            str(exc), context=ErrorContext(stage="acquire"), cause=exc
        ) from exc
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
