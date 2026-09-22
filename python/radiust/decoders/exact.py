"""Strict palette decoding with explicit quality flags."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from dataclasses import dataclass

import numpy as np

from ..errors import UnknownColorError

QUALITY_NO_RAIN = np.uint16(0)
QUALITY_MISSING = np.uint16(1)
QUALITY_OUTSIDE = np.uint16(2)
QUALITY_UNKNOWN = np.uint16(4)
QUALITY_RECOVERED = np.uint16(8)
QUALITY_INTERPOLATED = np.uint16(16)
QUALITY_BELOW_DETECTION = np.uint16(32)
QUALITY_PARTIAL = QUALITY_OUTSIDE


@dataclass(frozen=True, slots=True)
class PaletteEntry:
    color: tuple[int, int, int, int]
    value: float | None
    quality: int = int(QUALITY_NO_RAIN)


def _rgba(value: Iterable[int]) -> tuple[int, int, int, int]:
    values = tuple(int(v) for v in value)
    if len(values) == 3:
        return values + (255,)
    if len(values) != 4:
        raise ValueError("palette colors must have three or four channels")
    return values


class ExactPaletteDecoder:
    def __init__(self, palette: Mapping[Iterable[int], float | None | tuple[float | None, int]], *, strict: bool = True):
        self.palette = {}
        for color, value in palette.items():
            if isinstance(value, tuple):
                parsed = PaletteEntry(_rgba(color), value[0], int(value[1]))
            else:
                parsed = PaletteEntry(_rgba(color), value)
            self.palette[parsed.color] = parsed
        self.strict = strict

    def decode(self, rgba: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        pixels = np.asarray(rgba, dtype=np.uint8)
        if pixels.ndim != 3 or pixels.shape[2] not in {3, 4}:
            raise ValueError("RGBA input must have shape (height, width, 3|4)")
        if pixels.shape[2] == 3:
            pixels = np.concatenate([pixels, np.full((*pixels.shape[:2], 1), 255, dtype=np.uint8)], axis=2)
        values = np.full(pixels.shape[:2], np.nan, dtype=np.float32)
        quality = np.full(pixels.shape[:2], QUALITY_UNKNOWN, dtype=np.uint16)
        unknown: list[tuple[int, int, int, int]] = []
        for color in np.unique(pixels.reshape(-1, 4), axis=0):
            rgba_color = tuple(int(v) for v in color)
            entry = self.palette.get(rgba_color)
            mask = np.all(pixels == color, axis=2)
            if entry is None:
                unknown.append(rgba_color)
                continue
            if entry.value is not None:
                values[mask] = np.float32(entry.value)
            quality[mask] = np.uint16(entry.quality)
        if unknown and self.strict:
            colors = ", ".join(str(item) for item in unknown[:5])
            raise UnknownColorError(f"unknown palette color(s): {colors}")
        return values, quality


def decode_image(image: object, palette: Mapping[Iterable[int], float | None | tuple[float | None, int]], *, strict: bool = True) -> tuple[np.ndarray, np.ndarray]:
    """Decode a Pillow image or RGBA array without changing its dimensions."""
    if hasattr(image, "convert"):
        image = np.asarray(image.convert("RGBA"), dtype=np.uint8)
    return ExactPaletteDecoder(palette, strict=strict).decode(np.asarray(image))
