from __future__ import annotations

from .core import RenderedImage, render_field


def render(value: object, **options: object) -> RenderedImage:
    return render_field(value, **options)  # type: ignore[arg-type]

