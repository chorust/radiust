"""Per-frame Zarr v2 encoder."""

from __future__ import annotations

import importlib.util
from pathlib import Path
from typing import Any

from ..errors import MissingDependencyError
from ..field import RadarDataset, RadarField


def write_zarr(value: RadarField | RadarDataset, path: str | Path, *, options: dict[str, Any] | None = None) -> list[Path]:
    if importlib.util.find_spec("zarr") is None:
        raise MissingDependencyError("Zarr output requires the 'zarr' extra; install radiust[zarr]")
    output = Path(path)
    output.parent.mkdir(parents=True, exist_ok=True)
    dataset = value.to_dataset()
    encoding = {
        name: {
            "chunks": tuple(min(512, int(size)) for size in variable.shape),
        }
        for name, variable in dataset.data_vars.items()
        if variable.ndim
    }
    try:
        dataset.to_zarr(output, mode="w", consolidated=True, zarr_format=2, encoding=encoding)
    except TypeError:  # xarray releases using the older spelling
        dataset.to_zarr(output, mode="w", consolidated=True, zarr_version=2, encoding=encoding)
    return [output]
