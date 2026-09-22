from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from PIL import Image

from ..field import RadarField
from ..rendering.core import render_field


def render_rgba(field: RadarField, *, vmin: float | None = None, vmax: float | None = None):
    return render_field(field, vmin=vmin, vmax=vmax).rgba


def write_png(field: RadarField, path: str | Path, *, options: dict[str, Any] | None = None) -> list[Path]:
    options = options or {}
    output = Path(path)
    rendered = render_field(field, palette=options.get("palette"), vmin=options.get("vmin"), vmax=options.get("vmax"))
    rgba = rendered.rgba
    Image.fromarray(rgba, mode="RGBA").save(output, format="PNG")
    sidecar = output.with_name(output.stem + ".render.json")
    sidecar.write_text(json.dumps({"schema_version": 1, "palette": {"id": rendered.palette.id, "version": rendered.palette.version}, "vmin": rendered.vmin, "vmax": rendered.vmax, "shape": list(rgba.shape[:2]), "grid": field.grid.kind, "crs": field.grid.crs, "variable": field.variable, "title": rendered.title, "legend": list(rendered.legend), "provenance": field.provenance}, sort_keys=True, indent=2, allow_nan=False), encoding="utf-8")
    return [output, sidecar]
