"""Opt-in source-specific recovery protocol."""

from __future__ import annotations

from typing import Protocol

import numpy as np

from ..errors import MissingDependencyError
from .exact import QUALITY_RECOVERED


class RecoveryStrategy(Protocol):
    version: str

    def recover(self, values: np.ndarray, quality: np.ndarray) -> tuple[np.ndarray, np.ndarray]: ...


def require_recovery_dependency() -> None:
    try:
        import scipy  # noqa: F401
    except ImportError as exc:
        raise MissingDependencyError("recovery requires the 'recovery' extra") from exc


def recover_with_fill(values: np.ndarray, quality: np.ndarray, *, max_gap: int = 1) -> tuple[np.ndarray, np.ndarray]:
    """Fill only short horizontal gaps and mark every changed pixel."""
    require_recovery_dependency()
    result = np.asarray(values, dtype=np.float32).copy()
    flags = np.asarray(quality, dtype=np.uint16).copy()
    for row in range(result.shape[0]):
        for col in range(1, result.shape[1] - 1):
            if max_gap >= 1 and not np.isfinite(result[row, col]) and np.isfinite(result[row, col - 1]) and np.isfinite(result[row, col + 1]):
                result[row, col] = (result[row, col - 1] + result[row, col + 1]) / 2
                flags[row, col] = np.uint16(flags[row, col] | QUALITY_RECOVERED)
    return result, flags
