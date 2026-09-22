"""GeoTIFF group encoder with a paired uint16 quality raster."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import numpy as np

from ..errors import GridError, MissingDependencyError
from ..field import RadarField
from ..grids import CartesianGrid, GeographicGrid


def _transform(field: RadarField) -> tuple[Any, bool]:
    try:
        import rasterio.transform
    except ImportError as exc:  # pragma: no cover - exercised through capability preflight
        raise MissingDependencyError("GeoTIFF output requires the 'geotiff' extra; install radiust[geotiff]") from exc
    if isinstance(field.grid, GeographicGrid):
        x, y = np.asarray(field.grid.longitude), np.asarray(field.grid.latitude)
    elif isinstance(field.grid, CartesianGrid):
        x, y = np.asarray(field.grid.x), np.asarray(field.grid.y)
    else:
        raise GridError("GeoTIFF requires a regular geographic or cartesian grid")
    if len(x) < 2 or len(y) < 2:
        raise GridError("GeoTIFF requires at least two centers on each axis")
    dx = float(np.median(np.diff(x)))
    dy = float(np.median(np.diff(y)))
    if not np.allclose(np.diff(x), dx) or not np.allclose(np.diff(y), dy):
        raise GridError("GeoTIFF requires regularly spaced center coordinates")
    if dx <= 0:
        raise GridError("GeoTIFF x coordinates must increase")
    flip_y = dy > 0
    west = float(x[0] - dx / 2)
    north = float((y[-1] if flip_y else y[0]) + abs(dy) / 2)
    return rasterio.transform.from_origin(west, north, dx, abs(dy)), flip_y


def write_geotiff(field: RadarField, path: str | Path, *, options: dict[str, Any] | None = None) -> list[Path]:
    try:
        import rasterio
    except ImportError as exc:  # pragma: no cover - capability check normally catches it
        raise MissingDependencyError("GeoTIFF output requires the 'geotiff' extra; install radiust[geotiff]") from exc
    if not isinstance(field, RadarField):
        raise ValueError("GeoTIFF output requires one selected RadarField")
    transform, flip_y = _transform(field)
    output = Path(path)
    output.parent.mkdir(parents=True, exist_ok=True)
    data = np.asarray(field.data.values, dtype=np.float32)
    quality = np.zeros(data.shape, dtype=np.uint16) if field.quality is None else np.asarray(field.quality.values, dtype=np.uint16)
    if flip_y:
        data = data[::-1, :]
        quality = quality[::-1, :]
    crs = field.grid.crs
    profile = {"driver": "GTiff", "height": data.shape[0], "width": data.shape[1], "count": 1, "dtype": "float32", "crs": crs, "transform": transform, "compress": "deflate"}
    quality_path = output.with_name(output.stem + "_quality.tif")
    provenance_path = output.with_name(output.stem + "_provenance.json")
    package_dir = Path(rasterio.__file__).parent
    env_options = {
        name: str(package_dir / relative)
        for name, relative in (("PROJ_DATA", "proj_data"), ("GDAL_DATA", "gdal_data"))
        if (package_dir / relative).is_dir()
    }
    if "PROJ_DATA" in env_options:
        from rasterio.env import set_proj_data_search_path

        set_proj_data_search_path(env_options["PROJ_DATA"])
    if "GDAL_DATA" in env_options:
        from rasterio.env import set_gdal_config

        set_gdal_config("GDAL_DATA", env_options["GDAL_DATA"])
    if crs is not None:
        profile["crs"] = rasterio.crs.CRS.from_user_input(crs)
    with rasterio.open(output, "w", **profile, nodata=np.nan) as writer:
        writer.write(data, 1)
        writer.set_band_description(1, field.variable)
    with rasterio.open(quality_path, "w", **{**profile, "dtype": "uint16", "nodata": None}) as writer:
        writer.write(quality, 1)
        writer.set_band_description(1, "quality")
    provenance_path.write_text(json.dumps({"schema_version": 1, "variable": field.variable, "units": field.data.attrs.get("units"), "grid": field.grid.kind, "crs": crs, "transform": list(transform), "processing_history": list(field.processing_history), "provenance": field.provenance}, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False), encoding="utf-8")
    return [output, quality_path, provenance_path]
