"""Text summaries that contain no image or color control sequences."""

from __future__ import annotations

from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import numpy as np
from PIL import Image

from ..field import RadarDataset, RadarField
from ..outputs.netcdf import read_netcdf


def summarize(value: RadarField | RadarDataset | str | Path, *, at: datetime | None = None, variable: str | None = None) -> str:
    if isinstance(value, (str, Path)):
        path = Path(value)
        if path.suffix.lower() in {".nc", ".netcdf"}:
            return summarize(_dataset_from_netcdf(path, at=at), variable=variable)
        with Image.open(path) as image:
            sidecar = path.with_name(path.stem + ".render.json")
            suffix = f"; metadata={sidecar.name}" if sidecar.exists() else "; metadata=unknown"
            return f"image path={path.name} format={image.format or 'unknown'} size={image.width}x{image.height} mode={image.mode}{suffix}"
    field = value if isinstance(value, RadarField) else _select_dataset(value, variable)
    data = np.asarray(field.data.values, dtype=np.float32)
    finite = np.isfinite(data)
    valid_range = "unknown" if not finite.any() else f"{float(np.nanmin(data)):g}..{float(np.nanmax(data)):g}"
    missing = 1.0 if data.size == 0 else float((~finite).sum() / data.size)
    provenance = field.provenance
    return " ".join(
        [
            f"source={provenance.get('source', 'unknown')}",
            f"product={provenance.get('product', 'unknown')}",
            f"station={provenance.get('station', 'unknown')}",
            f"time={provenance.get('valid_time', 'unknown')}",
            f"variable={field.variable}",
            f"shape={field.data.shape}",
            f"units={field.data.attrs.get('units', 'unknown')}",
            f"valid_range={valid_range}",
            f"missing={missing:.2%}",
        ]
    )


def _dataset_from_netcdf(path: Path, *, at: datetime | None = None) -> RadarDataset:
    from ..grids import GeographicGrid

    dataset = read_netcdf(path)
    selected_time: datetime | None = None
    if "time" in dataset.coords:
        time_values = np.asarray(dataset["time"].values)
        if not np.issubdtype(time_values.dtype, np.datetime64):
            raise ValueError("NetCDF time coordinate must use a datetime type")
        if time_values.ndim == 0:
            if at is not None:
                target = np.datetime64(at.astimezone(timezone.utc).replace(tzinfo=None), "ns")
                if time_values.astype("datetime64[ns]") != target:
                    raise ValueError(f"NetCDF has no frame at {at.isoformat()}")
            selected_time = datetime.fromisoformat(str(np.datetime_as_string(time_values, unit="us"))).replace(tzinfo=timezone.utc)
        elif time_values.ndim == 1:
            if time_values.size == 0:
                raise ValueError("NetCDF time coordinate must be non-empty")
            if at is None and time_values.size > 1:
                raise ValueError("multi-time NetCDF requires --at")
            if at is not None:
                target = np.datetime64(at.astimezone(timezone.utc).replace(tzinfo=None), "ns")
                try:
                    dataset = dataset.sel(time=target)
                except (KeyError, ValueError) as exc:
                    raise ValueError(f"NetCDF has no frame at {at.isoformat()}") from exc
            elif "time" in dataset.dims:
                dataset = dataset.isel(time=0)
            value = np.asarray(dataset["time"].values)
            selected_time = datetime.fromisoformat(str(np.datetime_as_string(value, unit="us"))).replace(tzinfo=timezone.utc)
        else:
            raise ValueError("NetCDF time coordinate must be scalar or one-dimensional")
    if "latitude" not in dataset.coords or "longitude" not in dataset.coords:
        raise ValueError("NetCDF file has no supported geographic center coordinates")
    provenance: dict[str, Any] = {}
    raw = dataset.attrs.get("radiust_provenance")
    if isinstance(raw, str):
        import json

        try:
            provenance = json.loads(raw)
        except json.JSONDecodeError:
            provenance = {"raw": raw}
    grid = GeographicGrid(dataset.longitude.values, dataset.latitude.values, dataset["crs"].attrs.get("spatial_ref", "EPSG:4326") if "crs" in dataset.coords else "EPSG:4326")
    quality = dataset["quality"] if "quality" in dataset.data_vars else None
    if selected_time is not None:
        provenance.setdefault("valid_time", selected_time.isoformat().replace("+00:00", "Z"))
    return RadarDataset(dataset, grid, quality, provenance)


def _select_dataset(dataset: RadarDataset, variable: str | None = None) -> RadarField:
    if variable is not None:
        return dataset.select(variable)
    variables = [name for name in dataset.data.data_vars if name != "quality"]
    if len(variables) != 1:
        raise ValueError("multi-variable dataset requires variable")
    return dataset.select(variables[0])
