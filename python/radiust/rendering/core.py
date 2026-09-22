"""Pure rendering calculations; no terminal writes and no in-place Field mutation."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import numpy as np

from ..field import RadarDataset, RadarField
from ..grids import CartesianGrid, GeographicGrid
from ..models import format_time
from .legend import legend_labels
from .palettes import Palette, get_palette, map_values


@dataclass(frozen=True, slots=True)
class RenderedImage:
    rgba: np.ndarray
    palette: Palette
    vmin: float
    vmax: float
    title: str
    legend: tuple[str, ...]
    grid_kind: str
    crs: str | None
    provenance: dict[str, Any]

    def __post_init__(self) -> None:
        if self.rgba.ndim != 3 or self.rgba.shape[2] != 4 or self.rgba.dtype != np.dtype("uint8"):
            raise ValueError("RenderedImage rgba must be uint8 HxWx4")
        object.__setattr__(self, "rgba", self.rgba.copy())


def _select(value: RadarField | RadarDataset, variable: str | None) -> RadarField:
    if isinstance(value, RadarField):
        if variable is not None and variable != value.variable:
            raise KeyError(variable)
        return value
    if not isinstance(value, RadarDataset):
        raise TypeError("render expects RadarField or RadarDataset")
    if variable is None:
        variables = [name for name in value.data.data_vars if name != "quality"]
        if len(variables) != 1:
            raise ValueError("multi-variable dataset requires variable")
        variable = variables[0]
    return value.select(variable)


def _title(field: RadarField) -> str:
    provenance = field.provenance
    valid_time = provenance.get("valid_time", "unknown")
    if hasattr(valid_time, "isoformat"):
        valid_time = format_time(valid_time)
    unit = field.data.attrs.get("units", "unknown")
    return f"{provenance.get('source', 'unknown')} / {provenance.get('product', 'unknown')} / {provenance.get('station', 'unknown')} / {valid_time} / {field.variable} [{unit}]"


def render_field(value: RadarField | RadarDataset, *, variable: str | None = None, palette: str | Palette | None = None, vmin: float | None = None, vmax: float | None = None) -> RenderedImage:
    field = _select(value, variable)
    field.validate()
    selected_palette = get_palette(palette) if isinstance(palette, (str, type(None))) else palette
    values = np.asarray(field.data.values, dtype=np.float32)
    x_axis: tuple[float, ...] | None = None
    y_axis: tuple[float, ...] | None = None
    if isinstance(field.grid, GeographicGrid):
        x_axis, y_axis = field.grid.longitude, field.grid.latitude
    elif isinstance(field.grid, CartesianGrid):
        x_axis, y_axis = field.grid.x, field.grid.y
    if y_axis is not None and y_axis[0] < y_axis[-1]:
        values = values[::-1, :]
    if x_axis is not None and x_axis[0] > x_axis[-1]:
        values = values[:, ::-1]
    finite = np.isfinite(values)
    lo = float(np.nanmin(values)) if vmin is None and finite.any() else float(vmin if vmin is not None else 0)
    hi = float(np.nanmax(values)) if vmax is None and finite.any() else float(vmax if vmax is not None else 1)
    if not hi > lo:
        raise ValueError("vmax must be greater than vmin")
    rgba = np.zeros((*values.shape, 4), dtype=np.uint8)
    if finite.any():
        rgba[finite, :3] = map_values(values[finite], selected_palette, vmin=lo, vmax=hi)
    rgba[..., 3] = np.where(finite, 255, 0).astype(np.uint8)
    if field.quality is not None:
        # Missing and unknown pixels remain visible as transparent pixels.
        quality = np.asarray(field.quality.values, dtype=np.uint16)
        if y_axis is not None and y_axis[0] < y_axis[-1]:
            quality = quality[::-1, :]
        if x_axis is not None and x_axis[0] > x_axis[-1]:
            quality = quality[:, ::-1]
        rgba[(quality & (1 | 2 | 4)) != 0, 3] = 0
    return RenderedImage(rgba, selected_palette, lo, hi, _title(field), legend_labels(selected_palette, vmin=lo, vmax=hi), field.grid.kind, field.grid.crs, dict(field.provenance))
