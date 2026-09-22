"""Versioned palettes used by PNG and terminal rendering."""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np


@dataclass(frozen=True, slots=True)
class Palette:
    id: str
    version: str
    colors: tuple[tuple[int, int, int], ...]
    categorical: bool = False


DEFAULT_PALETTE = Palette(
    "default",
    "1",
    ((0, 0, 0), (43, 131, 186), (171, 221, 164), (255, 255, 191), (253, 174, 97), (215, 25, 28)),
)


def get_palette(name: str | None = None) -> Palette:
    if name in {None, "default"}:
        return DEFAULT_PALETTE
    raise ValueError(f"unknown palette: {name}")


def map_values(values: np.ndarray, palette: Palette, *, vmin: float, vmax: float) -> np.ndarray:
    normalized = np.clip((values - vmin) / (vmax - vmin), 0, 1)
    stops = np.linspace(0, 1, len(palette.colors))
    colors = np.asarray(palette.colors, dtype=np.float32)
    channels = [np.interp(normalized, stops, colors[:, channel]) for channel in range(3)]
    return np.stack(channels, axis=-1).astype(np.uint8)
