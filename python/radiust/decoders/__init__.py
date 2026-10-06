"""Minimal lazy exports for the Rust-backed gray decoders."""

from __future__ import annotations

from importlib import import_module
from typing import Any

__all__ = ["GrayDbzDecoder", "LegacyGrayDbzDecoder"]


def __getattr__(name: str) -> Any:
    if name in __all__:
        return getattr(import_module(".gray_dbz", __name__), name)
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def __dir__() -> list[str]:
    return sorted(set(globals()) | set(__all__))
