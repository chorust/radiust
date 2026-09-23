from __future__ import annotations

import io
import sys
from datetime import datetime
from pathlib import Path
from typing import TextIO

from PIL import Image

from ..display.models import RawPreview
from ..rendering.core import RenderedImage, render_field
from ..rendering.palettes import DEFAULT_PALETTE
from .ansi import render_ansi
from .capabilities import capabilities
from .iterm2 import iterm2_sequence
from .kitty import kitty_chunks
from .session import TerminalSession
from .text import _dataset_from_netcdf, summarize


def show(value: object, *, renderer: str = "auto", width: int | None = None, height: int | None = None, stream: TextIO | None = None, **options: object) -> None:
    stream = stream or sys.stdout
    if renderer not in {"auto", "kitty", "iterm2", "ansi", "text"}:
        raise ValueError("renderer must be auto, kitty, iterm2, ansi, or text")
    detected = capabilities(stream)
    chosen = detected.renderer if renderer == "auto" else renderer
    if chosen in {"kitty", "iterm2", "ansi"} and not detected.is_tty:
        raise ValueError(f"renderer {chosen} requires a TTY; use --renderer text for redirected output")
    if renderer != "auto" and chosen == "kitty" and not detected.kitty:
        raise ValueError("Kitty graphics capability was not confirmed")
    if renderer != "auto" and chosen == "iterm2" and not detected.iterm2:
        raise ValueError("iTerm2 inline-image capability was not confirmed")
    if isinstance(value, RawPreview):
        if chosen == "text":
            stream.write(summarize(value) + "\n")
            return
        rendered = RenderedImage(value.rgba, DEFAULT_PALETTE, 0, 1, summarize(value), (), "unknown", None, {})
        if chosen == "ansi":
            stream.write(render_ansi(rendered, width=width or 80, height=height or 24) + "\n")
            return
        output = io.BytesIO()
        Image.fromarray(value.rgba, mode="RGBA").save(output, format="PNG")
        png = output.getvalue()
        sequence = "".join(kitty_chunks(png)) if chosen == "kitty" else iterm2_sequence(png, width=width, height=height)
        with TerminalSession(stream):
            stream.write(sequence)
            stream.flush()
        return
    if isinstance(value, (str, Path)) and Path(value).suffix.lower() in {".nc", ".netcdf"} and chosen != "text":
        value = _dataset_from_netcdf(Path(value), at=options.get("at") if isinstance(options.get("at"), datetime) else None)
    if isinstance(value, (str, Path)) and Path(value).suffix.lower() == ".png":
        png = Path(value).read_bytes()
        if chosen == "text":
            stream.write(summarize(value) + "\n")
            return
        if chosen == "kitty":
            payload = "".join(kitty_chunks(png))
        elif chosen == "iterm2":
            payload = iterm2_sequence(png, width=width, height=height)
        else:
            with Image.open(value) as image:
                rendered = _render_png(image)
            payload = render_ansi(rendered, width=width or 80, height=height or 24)
        with TerminalSession(stream):
            stream.write(payload)
            stream.flush()
        return
    if chosen == "text":
        stream.write(summarize(value, at=options.get("at") if isinstance(options.get("at"), datetime) else None, variable=options.get("variable")) + "\n")
        return
    rendered = value if isinstance(value, RenderedImage) else render_field(value, variable=options.get("variable"), palette=options.get("palette"), vmin=options.get("vmin"), vmax=options.get("vmax"))
    if chosen == "ansi":
        stream.write(render_ansi(rendered, width=width or 80, height=height or 24) + "\n")
    else:
        import PIL.Image

        image = PIL.Image.fromarray(rendered.rgba, mode="RGBA")
        buffer = io.BytesIO()
        image.save(buffer, format="PNG")
        png = buffer.getvalue()
        payload = "".join(kitty_chunks(png)) if chosen == "kitty" else iterm2_sequence(png, width=width, height=height)
        with TerminalSession(stream):
            stream.write(payload)
            stream.flush()


def _render_png(image: Image.Image) -> RenderedImage:
    import numpy as np

    from ..rendering.palettes import DEFAULT_PALETTE

    rgba = np.asarray(image.convert("RGBA"), dtype=np.uint8).copy()
    return RenderedImage(rgba, DEFAULT_PALETTE, 0, 1, "image (metadata unknown)", ("unknown",), "unknown", None, {})
