"""Thin Rust-backed adapters for canonical and historical gray decoding."""

from __future__ import annotations

from typing import TYPE_CHECKING

from .. import _bridge
from ..errors import DecodeError, ErrorContext, MissingDependencyError, UnknownColorError

if TYPE_CHECKING:
    import numpy as np


def _array_shape(pixels: np.ndarray) -> tuple[int, int, int]:
    if pixels.ndim == 2:
        return pixels.shape[1], pixels.shape[0], 1
    if pixels.ndim == 3 and pixels.shape[2] in {3, 4}:
        return pixels.shape[1], pixels.shape[0], pixels.shape[2]
    raise ValueError("gray input must have shape (height, width) or (height, width, 3|4)")


def _decode_array(
    pixels: np.ndarray,
    *,
    historical: bool,
    strict: bool,
    max_gray: int,
) -> tuple[np.ndarray, np.ndarray]:
    try:
        import numpy as np
    except ImportError as exc:
        raise MissingDependencyError(
            "gray array decoding requires NumPy; install radiust[science]", cause=exc
        ) from exc
    array = np.asarray(pixels)
    if array.dtype.kind not in "biuf":
        raise ValueError("gray pixels must be numeric")
    width, height, channels = _array_shape(array)
    alpha_bit_depth = 0
    if not historical and channels == 4:
        if array.dtype == np.dtype("uint8"):
            alpha_bit_depth = 8
        elif array.dtype == np.dtype("uint16"):
            alpha_bit_depth = 16
    decoded, quality = _bridge._decode_gray_array_native(
        width,
        height,
        channels,
        array.reshape(-1).tolist(),
        alpha_bit_depth=alpha_bit_depth,
        historical=historical,
        strict=strict,
        max_gray=max_gray,
    )
    values = np.asarray(decoded, dtype=np.float32).reshape(height, width)
    flags = np.asarray(quality, dtype=np.uint16).reshape(height, width)
    return values, flags


class GrayDbzDecoder:
    """Strict ``gray-dbz-v1`` decoder returning ``(dBZ, quality)`` arrays."""

    def decode(self, pixels: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        return _decode_array(pixels, historical=False, strict=True, max_gray=224)


class LegacyGrayDbzDecoder:
    """Compatibility adapter for the former configurable uint8 decoder."""

    def __init__(self, *, strict: bool = True, max_gray: int = 224) -> None:
        if not 0 <= max_gray <= 255:
            raise ValueError("max_gray must be between 0 and 255")
        self.strict = bool(strict)
        self.max_gray = int(max_gray)

    def decode(self, pixels: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        try:
            return _decode_array(
                pixels,
                historical=True,
                strict=self.strict,
                max_gray=self.max_gray,
            )
        except DecodeError as exc:
            if self.strict and "pixel(s) are not valid legacy grayscale reflectivity" in str(exc):
                raise UnknownColorError(
                    str(exc), context=ErrorContext(stage="decode", code="unknown_color"), cause=exc
                ) from exc
            raise ValueError(str(exc)) from exc


__all__ = ["GrayDbzDecoder", "LegacyGrayDbzDecoder"]
