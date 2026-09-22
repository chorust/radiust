"""Encoder capability registry and dependency checks."""

from __future__ import annotations

import importlib.util
from dataclasses import dataclass
from typing import Any

from ..errors import MissingDependencyError
from ..field import RadarDataset, RadarField
from .base import Encoder
from .netcdf import write_netcdf
from .png import write_png


@dataclass(frozen=True, slots=True)
class EncoderSpec:
    format: str
    writer: Encoder
    required_extra: str | None = None


def _write_netcdf(value: RadarField | RadarDataset, path: Any, *, options: dict[str, Any] | None = None) -> list[Any]:
    return write_netcdf(value, path, options=options)


def _write_png(value: RadarField | RadarDataset, path: Any, *, options: dict[str, Any] | None = None) -> list[Any]:
    if isinstance(value, RadarDataset):
        variable = (options or {}).get("variable")
        if not variable:
            raise ValueError("PNG output for a multi-variable dataset requires variable")
        value = value.select(variable)
    return write_png(value, path, options=options)


def _load_optional(format: str) -> Encoder:
    if format == "geotiff":
        from .geotiff import write_geotiff

        return write_geotiff
    if format == "zarr":
        from .zarr import write_zarr

        return write_zarr
    raise ValueError(f"unsupported output format: {format}")


def encoder_for(format: str) -> EncoderSpec:
    if format == "netcdf":
        return EncoderSpec(format, _write_netcdf)
    if format == "png":
        return EncoderSpec(format, _write_png)
    if format == "geotiff":
        return EncoderSpec(format, _load_optional(format), "geotiff")
    if format == "zarr":
        return EncoderSpec(format, _load_optional(format), "zarr")
    raise ValueError(f"unsupported output format: {format}")


def check_encoder_dependencies(format: str) -> None:
    if format == "geotiff" and importlib.util.find_spec("rasterio") is None:
        raise MissingDependencyError("GeoTIFF output requires the 'geotiff' extra; install radiust[geotiff]")
    if format == "zarr" and importlib.util.find_spec("zarr") is None:
        raise MissingDependencyError("Zarr output requires the 'zarr' extra; install radiust[zarr]")

