from __future__ import annotations

from collections.abc import Iterable, Mapping

import numpy as np

from ..errors import UnknownColorError
from .exact import QUALITY_UNKNOWN


class NearestPaletteDecoder:
    def __init__(self, palette: Mapping[Iterable[int], float], *, threshold: float, strict: bool = True):
        if threshold < 0:
            raise ValueError("threshold must be non-negative")
        entries = [(np.asarray(tuple(color), dtype=np.float32), float(value)) for color, value in palette.items()]
        if not entries:
            raise ValueError("palette cannot be empty")
        self.colors = np.stack([entry[0] for entry in entries])
        self.values = np.asarray([entry[1] for entry in entries], dtype=np.float32)
        self.threshold = float(threshold)
        self.strict = strict

    def decode(self, pixels: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        array = np.asarray(pixels, dtype=np.float32)
        if array.ndim != 3 or array.shape[2] != self.colors.shape[1]:
            raise ValueError("pixels must be an image with the same channel count as the palette")
        distances = np.linalg.norm(array[..., None, :] - self.colors[None, None, :, :], axis=-1)
        indices = distances.argmin(axis=-1)
        best = distances.min(axis=-1)
        unknown = best > self.threshold
        if unknown.any() and self.strict:
            raise UnknownColorError(f"{int(unknown.sum())} pixel(s) exceed palette threshold")
        result = self.values[indices].astype(np.float32)
        result[unknown] = np.nan
        quality = np.where(unknown, QUALITY_UNKNOWN, 0).astype(np.uint16)
        return result, quality
