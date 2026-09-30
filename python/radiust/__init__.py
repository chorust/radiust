"""Independent radar data acquisition and processing toolkit.

The package root keeps its historical public names while resolving them on
first access. This matters for the ``radiust`` console entry point: Python must
load this module before ``radiust.cli.main``, and the CLI immediately forwards
to Rust without needing to import the Python SDK, configuration parser, or
optional scientific adapters.
"""

from __future__ import annotations

from importlib import import_module
from typing import Any

__version__ = "0.1.0"

_EXPORTS = {
    # Public convenience functions.
    "adownload": (".api", "adownload"),
    "afetch": (".api", "afetch"),
    "afetch_many": (".api", "afetch_many"),
    "aiter_fetch": (".api", "aiter_fetch"),
    "download": (".api", "download"),
    "fetch": (".api", "fetch"),
    "fetch_many": (".api", "fetch_many"),
    "iter_fetch": (".api", "iter_fetch"),
    # Rust-backed SDK facade.
    "AsyncClient": (".client", "AsyncClient"),
    "Client": (".client", "Client"),
    # Stable Python value objects and error classes.
    "Artifact": (".models", "Artifact"),
    "BatchResult": (".models", "BatchResult"),
    "DownloadReport": (".models", "DownloadReport"),
    "FrameRef": (".models", "FrameRef"),
    "FrameResult": (".models", "FrameResult"),
    "OutputRequest": (".models", "OutputRequest"),
    "ProcessingSpec": (".models", "ProcessingSpec"),
    "ProductInfo": (".models", "ProductInfo"),
    "Query": (".models", "Query"),
    "SourceInfo": (".models", "SourceInfo"),
    "StationInfo": (".models", "StationInfo"),
    "AmbiguousFrameError": (".errors", "AmbiguousFrameError"),
    "AsyncContextError": (".errors", "AsyncContextError"),
    "AuthenticationError": (".errors", "AuthenticationError"),
    "BatchError": (".errors", "BatchError"),
    "ConfigError": (".errors", "ConfigError"),
    "DecodeError": (".errors", "DecodeError"),
    "DuplicateSourceError": (".errors", "DuplicateSourceError"),
    "GeoreferencingError": (".errors", "GeoreferencingError"),
    "GridError": (".errors", "GridError"),
    "IntegrityError": (".errors", "IntegrityError"),
    "MissingDependencyError": (".errors", "MissingDependencyError"),
    "NoDataError": (".errors", "NoDataError"),
    "OutputConflict": (".errors", "OutputConflict"),
    "OutputLockedError": (".errors", "OutputLockedError"),
    "RadiustError": (".errors", "RadiustError"),
    "ResourceLimitError": (".errors", "ResourceLimitError"),
    "StaleFrameError": (".errors", "StaleFrameError"),
    "StorageError": (".errors", "StorageError"),
    "TransportError": (".errors", "TransportError"),
    "UnknownColorError": (".errors", "UnknownColorError"),
    "UnsupportedQueryError": (".errors", "UnsupportedQueryError"),
    # Explicit Python interoperability boundary.
    "to_xarray": (".science_adapters", "to_xarray"),
}

_OPTIONAL_EXPORTS = {
    "Grid": ("radiust.grids", "Grid"),
    "GeographicGrid": ("radiust.grids", "GeographicGrid"),
    "CartesianGrid": ("radiust.grids", "CartesianGrid"),
    "PolarGrid": ("radiust.grids", "PolarGrid"),
    "CurvilinearGrid": ("radiust.grids", "CurvilinearGrid"),
}

__all__ = [
    "__version__",
    "Client",
    "AsyncClient",
    "Query",
    "FrameRef",
    "SourceInfo",
    "ProductInfo",
    "StationInfo",
    "Artifact",
    "ProcessingSpec",
    "OutputRequest",
    "FrameResult",
    "BatchResult",
    "DownloadReport",
    "RadarField",
    "RadarDataset",
    "Grid",
    "GeographicGrid",
    "CartesianGrid",
    "PolarGrid",
    "CurvilinearGrid",
    "fetch",
    "afetch",
    "fetch_many",
    "afetch_many",
    "iter_fetch",
    "aiter_fetch",
    "download",
    "adownload",
    "to_xarray",
    "RadiustError",
    "ConfigError",
    "MissingDependencyError",
    "UnsupportedQueryError",
    "AsyncContextError",
    "NoDataError",
    "StaleFrameError",
    "AmbiguousFrameError",
    "AuthenticationError",
    "TransportError",
    "IntegrityError",
    "ResourceLimitError",
    "UnknownColorError",
    "DecodeError",
    "GridError",
    "GeoreferencingError",
    "OutputConflict",
    "OutputLockedError",
    "StorageError",
    "DuplicateSourceError",
    "BatchError",
]


def __getattr__(name: str) -> Any:
    target = _EXPORTS.get(name)
    if target is not None:
        module_name, attribute = target
        value = getattr(import_module(module_name, __name__), attribute)
        globals()[name] = value
        return value

    if name in {"RadarField", "RadarDataset"}:
        bridge = import_module("._bridge", __name__)
        if bridge._core is None:
            raise ImportError(f"{name} requires the compiled radiust Rust extension")
        value = getattr(bridge._core, name)
        globals()[name] = value
        return value

    optional = _OPTIONAL_EXPORTS.get(name)
    if optional is not None:
        module_name, attribute = optional
        try:
            value = getattr(import_module(module_name), attribute)
        except ImportError as exc:
            raise ImportError(
                f"{name} requires optional scientific dependencies; install radiust[science]"
            ) from exc
        globals()[name] = value
        return value

    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def __dir__() -> list[str]:
    return sorted(set(globals()) | set(_EXPORTS) | set(_OPTIONAL_EXPORTS) | {"RadarField", "RadarDataset"})
