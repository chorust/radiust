"""Explicit NetCDF4 encoder."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import numpy as np
import xarray as xr

from ..errors import MissingDependencyError
from ..field import RadarDataset, RadarField


def _safe_attrs(dataset: xr.Dataset) -> xr.Dataset:
    result = dataset.copy(deep=False)
    for name in result.data_vars:
        if result[name].dtype.kind == "f":
            result[name].encoding.setdefault("dtype", "float32")
        if name == "quality":
            result[name].encoding["dtype"] = "uint16"
            result[name].encoding["_FillValue"] = None
    for key, value in list(result.attrs.items()):
        if isinstance(value, (dict, list, tuple)):
            result.attrs[key] = json.dumps(value, ensure_ascii=False, sort_keys=True, allow_nan=False)
        elif not isinstance(value, (str, int, float, np.number, bool)) and value is not None:
            result.attrs[key] = str(value)
    return result


def write_netcdf(value: RadarField | RadarDataset, path: str | Path, *, options: dict[str, Any] | None = None) -> list[Path]:
    try:
        dataset = value.to_dataset()
        dataset = _safe_attrs(dataset)
        output = Path(path)
        output.parent.mkdir(parents=True, exist_ok=True)
        dataset.to_netcdf(output, engine="h5netcdf", format="NETCDF4", invalid_netcdf=False)
    except ImportError as exc:
        raise MissingDependencyError("NetCDF output requires h5netcdf and h5py") from exc
    return [output]


def read_netcdf(path: str | Path) -> xr.Dataset:
    return xr.open_dataset(path, engine="h5netcdf").load()
