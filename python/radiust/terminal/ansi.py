from __future__ import annotations

import numpy as np

from ..rendering.core import RenderedImage


def _resize(rgba: np.ndarray, width: int, height: int) -> np.ndarray:
    if width < 1 or height < 1:
        raise ValueError("width and height must be positive")
    source_height, source_width = rgba.shape[:2]
    x = np.minimum((np.arange(width) * source_width // width), source_width - 1)
    y = np.minimum((np.arange(height) * source_height // height), source_height - 1)
    return rgba[np.ix_(y, x)].copy()


def render_ansi(image: RenderedImage, *, width: int = 80, height: int = 24) -> str:
    # Each terminal row carries two source rows via the upper-half block.
    raster = _resize(image.rgba, width, max(1, height * 2))
    lines = [image.title, "legend=" + " ".join(image.legend)]
    for row in range(0, raster.shape[0] - 1, 2):
        chunks: list[str] = []
        for upper, lower in zip(raster[row], raster[row + 1], strict=True):
            if upper[3] == 0 and lower[3] == 0:
                chunks.append(" ")
            elif lower[3] == 0:
                chunks.append(f"\x1b[38;2;{upper[0]};{upper[1]};{upper[2]}m▀\x1b[0m")
            elif upper[3] == 0:
                chunks.append(f"\x1b[38;2;{lower[0]};{lower[1]};{lower[2]}m▄\x1b[0m")
            else:
                chunks.append(f"\x1b[38;2;{upper[0]};{upper[1]};{upper[2]}m\x1b[48;2;{lower[0]};{lower[1]};{lower[2]}m▀\x1b[0m")
        lines.append("".join(chunks))
    return "\n".join(lines)
