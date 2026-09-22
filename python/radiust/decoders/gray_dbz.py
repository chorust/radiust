"""Decode the grayscale reflectivity encoding used by the legacy scraper."""

from __future__ import annotations

import numpy as np

from ..errors import UnknownColorError
from .exact import QUALITY_MISSING, QUALITY_UNKNOWN


class LegacyGrayDbzDecoder:
    """Decode ``旧项目`` grayscale pixels into reflectivity.

    The legacy post-processing step stored reflectivity as ``gray = dBZ *
    16 / 5`` and clipped the result to ``0..224``.  The inverse therefore is
    ``dBZ = gray / 16 * 5``.  Opaque black is a valid ``0 dBZ`` sample; only
    transparent pixels are treated as missing by this decoder.
    """

    def __init__(self, *, strict: bool = True, max_gray: int = 224) -> None:
        if not 0 <= max_gray <= 255:
            raise ValueError("max_gray must be between 0 and 255")
        self.strict = strict
        self.max_gray = int(max_gray)

    def decode(self, pixels: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        array = np.asarray(pixels)
        if array.ndim == 2:
            gray = array
            alpha = np.full(gray.shape, 255, dtype=np.uint8)
            grayscale = np.ones(gray.shape, dtype=bool)
        elif array.ndim == 3 and array.shape[2] in {3, 4}:
            rgb = array[..., :3]
            gray = rgb[..., 0]
            alpha = array[..., 3] if array.shape[2] == 4 else np.full(gray.shape, 255, dtype=np.uint8)
            grayscale = np.all(rgb == gray[..., None], axis=2)
        else:
            raise ValueError("legacy gray input must have shape (height, width) or (height, width, 3|4)")
        if array.dtype.kind not in "biuf" or not np.isfinite(array).all() or np.any(array < 0) or np.any(array > 255):
            raise ValueError("legacy gray pixels must be finite values in the range 0..255")

        gray = np.asarray(gray, dtype=np.uint8)
        alpha = np.asarray(alpha, dtype=np.uint8)
        visible = alpha != 0
        unknown = visible & (~grayscale | (gray > self.max_gray))
        if unknown.any() and self.strict:
            raise UnknownColorError(f"{int(unknown.sum())} pixel(s) are not valid legacy grayscale reflectivity")

        values = np.full(gray.shape, np.nan, dtype=np.float32)
        valid = visible & ~unknown
        values[valid] = gray[valid].astype(np.float32) / 16.0 * 5.0
        quality = np.zeros(gray.shape, dtype="uint16")
        quality[~visible] = QUALITY_MISSING
        quality[unknown] = QUALITY_UNKNOWN
        return values, quality


__all__ = ["LegacyGrayDbzDecoder"]
