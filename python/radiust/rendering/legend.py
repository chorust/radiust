from __future__ import annotations

from .palettes import Palette


def legend_labels(palette: Palette, *, vmin: float, vmax: float, count: int = 5) -> tuple[str, ...]:
    if count < 2:
        raise ValueError("legend requires at least two labels")
    return tuple(f"{vmin + (vmax - vmin) * index / (count - 1):g}" for index in range(count))
