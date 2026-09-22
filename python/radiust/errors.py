"""Stable domain errors exposed by the SDK and CLI."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

__all__ = [
    "RadiustError", "ConfigError", "MissingDependencyError", "UnsupportedQueryError",
    "AsyncContextError", "NoDataError", "StaleFrameError", "AmbiguousFrameError",
    "AuthenticationError", "TransportError", "IntegrityError", "ResourceLimitError",
    "UnknownColorError", "DecodeError", "GridError", "GeoreferencingError",
    "OutputConflict", "OutputLockedError", "StorageError", "CommitOutcomeUnknown", "DuplicateSourceError",
    "BatchError", "ErrorContext", "error_from_exception",
]


@dataclass
class ErrorContext:
    stage: str = "validate"
    source: str | None = None
    frame_id: str | None = None
    retryable: bool = False


class RadiustError(Exception):
    """Base class whose public attributes are safe to put in a report."""

    code = "radiust_error"
    default_stage = "validate"
    default_retryable = False

    def __init__(self, message: str, *, context: ErrorContext | None = None, cause: Exception | None = None):
        super().__init__(message)
        self.message = message
        self.context = context or ErrorContext(
            stage=self.default_stage, retryable=self.default_retryable
        )
        self.cause = cause

    @property
    def stage(self) -> str:
        return self.context.stage

    @property
    def source(self) -> str | None:
        return self.context.source

    @property
    def frame_id(self) -> str | None:
        return self.context.frame_id

    @property
    def retryable(self) -> bool:
        return self.context.retryable

    def as_dict(self) -> dict[str, Any]:
        result: dict[str, Any] = {
            "code": self.code,
            "message": self.message,
            "stage": self.stage,
            "retryable": self.retryable,
        }
        if self.source:
            result["source"] = self.source
        if self.frame_id:
            result["frame_id"] = self.frame_id
        return result


class ConfigError(RadiustError):
    code = "config_error"


class MissingDependencyError(RadiustError):
    code = "missing_dependency"


class UnsupportedQueryError(RadiustError):
    code = "unsupported_query"


class AsyncContextError(RadiustError):
    code = "async_context"


class NoDataError(RadiustError):
    code = "no_data"
    default_stage = "discover"


class StaleFrameError(RadiustError):
    code = "stale_frame"
    default_stage = "discover"


class AmbiguousFrameError(RadiustError):
    code = "ambiguous_frame"
    default_stage = "discover"


class AuthenticationError(RadiustError):
    code = "authentication"
    default_stage = "acquire"


class TransportError(RadiustError):
    code = "transport"
    default_stage = "acquire"


class IntegrityError(RadiustError):
    code = "integrity"
    default_stage = "validate"


class ResourceLimitError(RadiustError):
    code = "resource_limit"
    default_stage = "validate"


class UnknownColorError(RadiustError):
    code = "unknown_color"
    default_stage = "decode"


class DecodeError(RadiustError):
    code = "decode"
    default_stage = "decode"


class GridError(RadiustError):
    code = "grid"
    default_stage = "regrid"


class GeoreferencingError(RadiustError):
    code = "georeferencing"
    default_stage = "render"


class OutputConflict(RadiustError):
    code = "output_conflict"
    default_stage = "commit"


class OutputLockedError(RadiustError):
    code = "output_locked"
    default_stage = "commit"


class StorageError(RadiustError):
    code = "storage"
    default_stage = "commit"


class CommitOutcomeUnknown(StorageError):
    code = "commit_outcome_unknown"


class DuplicateSourceError(RadiustError):
    code = "duplicate_source"
    default_stage = "registry"


class BatchError(RadiustError):
    code = "batch_error"
    default_stage = "process"

    def __init__(self, message: str, *, partial_result: Any = None, **kwargs: Any):
        super().__init__(message, **kwargs)
        self.partial_result = partial_result


def error_from_exception(exc: Exception, *, stage: str, source: str | None = None) -> RadiustError:
    if isinstance(exc, RadiustError):
        return exc
    return RadiustError(
        str(exc) or exc.__class__.__name__,
        context=ErrorContext(stage=stage, source=source),
        cause=exc,
    )
