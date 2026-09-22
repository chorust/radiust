"""Scientific containers with explicit grid and quality semantics."""

from __future__ import annotations

from dataclasses import dataclass
from dataclasses import field as dc_field
from datetime import datetime, timezone
from typing import Any

import numpy as np
import xarray as xr

from .errors import GridError
from .grids import CartesianGrid, CurvilinearGrid, GeographicGrid, Grid, PolarGrid


def _grid_coords(grid: Grid, dims: tuple[str, str] | None = None) -> dict[str, Any]:
    if isinstance(grid, GeographicGrid):
        return {"latitude": np.asarray(grid.latitude), "longitude": np.asarray(grid.longitude)}
    if isinstance(grid, CartesianGrid):
        return {"y": np.asarray(grid.y), "x": np.asarray(grid.x)}
    if isinstance(grid, PolarGrid):
        return {"azimuth": np.asarray(grid.azimuth), "range": np.asarray(grid.range_m)}
    if isinstance(grid, CurvilinearGrid):
        coordinate_dims = dims or ("y", "x")
        return {
            "latitude": (coordinate_dims, np.asarray(grid.latitude)),
            "longitude": (coordinate_dims, np.asarray(grid.longitude)),
        }
    return {}


def _is_categorical(field: RadarField) -> bool:
    attrs = field.data.attrs
    return (
        str(attrs.get("kind", "")).lower() in {"categorical", "category", "class"}
        or "flag_values" in attrs
        or "flag_meanings" in attrs
        or field.variable.lower() in {"quality", "category", "class"}
    )


def _bracket(axis: np.ndarray, value: float) -> tuple[int, int, float] | None:
    if value < axis[0] or value > axis[-1]:
        return None
    if value == axis[0]:
        return 0, 0, 0.0
    if value == axis[-1]:
        last = len(axis) - 1
        return last, last, 0.0
    upper = int(np.searchsorted(axis, value, side="right"))
    lower = upper - 1
    weight = float((value - axis[lower]) / (axis[upper] - axis[lower]))
    return lower, upper, weight


def _target_mesh_in_source_crs(source: Grid, target: Grid) -> tuple[np.ndarray, np.ndarray]:
    target_x = np.asarray(target.longitude if isinstance(target, GeographicGrid) else target.x, dtype=np.float64)
    target_y = np.asarray(target.latitude if isinstance(target, GeographicGrid) else target.y, dtype=np.float64)
    mesh_x, mesh_y = np.meshgrid(target_x, target_y)
    if source.crs == target.crs:
        return mesh_x, mesh_y
    if source.crs is None or target.crs is None:
        raise GridError("regridding across grid types requires both source and target CRS")
    try:
        from pyproj import CRS, Transformer
        from pyproj.exceptions import ProjError

        source_crs = CRS.from_user_input(source.crs)
        target_crs = CRS.from_user_input(target.crs)
        if source_crs == target_crs:
            return mesh_x, mesh_y
        transformer = Transformer.from_crs(target_crs, source_crs, always_xy=True)
        transformed_x, transformed_y = transformer.transform(mesh_x, mesh_y)
    except (ProjError, ValueError, TypeError) as exc:
        raise GridError(f"unable to transform target coordinates into source CRS: {exc}") from exc
    return np.asarray(transformed_x, dtype=np.float64), np.asarray(transformed_y, dtype=np.float64)


def _nearest_indices(axis: np.ndarray, values: np.ndarray) -> np.ndarray:
    if len(axis) == 1:
        return np.zeros(values.shape, dtype=np.intp)
    safe = np.where(np.isfinite(values), values, axis[0])
    upper = np.searchsorted(axis, safe, side="left")
    upper = np.clip(upper, 1, len(axis) - 1)
    lower = upper - 1
    choose_upper = np.abs(axis[upper] - safe) < np.abs(safe - axis[lower])
    return np.where(choose_upper, upper, lower).astype(np.intp)


def _bilinear(
    values: np.ndarray,
    source_x: np.ndarray,
    source_y: np.ndarray,
    target_x: np.ndarray,
    target_y: np.ndarray,
    units: Any,
    source_quality: np.ndarray | None,
) -> tuple[np.ndarray, np.ndarray]:
    result = np.full(target_x.shape, np.nan, dtype=np.float32)
    quality = np.full(target_x.shape, 2, dtype="uint16")
    dbz = str(units).lower() == "dbz"
    working = values.astype(np.float64)
    if dbz:
        with np.errstate(over="ignore", invalid="ignore"):
            working = np.power(10.0, working / 10.0)
    invalid_flags = np.uint16(1 | 2 | 4 | 32)
    for target_row in range(target_x.shape[0]):
        for target_column in range(target_x.shape[1]):
            x_value = float(target_x[target_row, target_column])
            y_value = float(target_y[target_row, target_column])
            if not np.isfinite(x_value) or not np.isfinite(y_value):
                continue
            x_bracket = _bracket(source_x, x_value)
            y_bracket = _bracket(source_y, y_value)
            if x_bracket is None or y_bracket is None:
                continue
            x0, x1, wx = x_bracket
            y0, y1, wy = y_bracket
            indices = ((y0, x0), (y0, x1), (y1, x0), (y1, x1))
            weights = np.asarray(
                [(1 - wy) * (1 - wx), (1 - wy) * wx, wy * (1 - wx), wy * wx],
                dtype=np.float64,
            )
            # Source pixels with zero weight must not turn an exact grid-line
            # sample into missing data or propagate unrelated quality flags.
            active = weights > 0
            contributions = np.asarray([working[row, column] for row, column in indices], dtype=np.float64)[active]
            flags = np.asarray(
                [source_quality[row, column] for row, column in indices]
                if source_quality is not None
                else [0, 0, 0, 0],
                dtype="uint16",
            )[active]
            combined = np.bitwise_or.reduce(flags)
            if not np.isfinite(contributions).all() or np.any(flags & invalid_flags):
                quality[target_row, target_column] = np.uint16(combined | (1 if not np.isfinite(contributions).all() else 0))
                continue
            physical = float(np.dot(contributions, weights[active]))
            if dbz:
                with np.errstate(divide="ignore", invalid="ignore"):
                    physical = float(10.0 * np.log10(physical))
            result[target_row, target_column] = np.float32(physical)
            quality[target_row, target_column] = np.uint16(combined | (16 if np.count_nonzero(active) > 1 else 0))
    return result, quality


@dataclass(frozen=True, slots=True)
class RadarField:
    data: xr.DataArray
    grid: Grid
    quality: xr.DataArray | None = None
    provenance: dict[str, Any] = dc_field(default_factory=dict)
    processing_history: tuple[dict[str, Any], ...] = ()

    def __post_init__(self) -> None:
        self.validate()

    @property
    def variable(self) -> str:
        return self.data.name or "value"

    def validate(self) -> None:
        self.grid.validate()
        if self.data.ndim != 2:
            raise ValueError("RadarField data must be two-dimensional")
        if tuple(self.data.shape) != self.grid.shape:
            raise GridError(f"data shape {self.data.shape} does not match grid {self.grid.shape}")
        if not np.issubdtype(self.data.dtype, np.number):
            raise TypeError("RadarField data must be numeric")
        if self.quality is not None:
            if self.quality.dims != self.data.dims or self.quality.shape != self.data.shape:
                raise ValueError("quality must have the same dimensions as data")
            if self.quality.dtype != np.dtype("uint16"):
                raise TypeError("quality must preserve uint16 flags")
        if not isinstance(self.provenance, dict):
            raise TypeError("provenance must be a dictionary")

    def to_dataset(self) -> xr.Dataset:
        dataset = self.data.to_dataset(name=self.variable)
        if self.quality is not None:
            dataset["quality"] = self.quality
            dataset["quality"].attrs.update(
                {
                    "flag_masks": np.array([1, 2, 4, 8, 16, 32], dtype="uint16"),
                    "flag_meanings": "missing outside_coverage unknown_color recovered interpolated below_detection",
                }
            )
            dataset[self.variable].attrs.setdefault("ancillary_variables", "quality")
        for name, values in _grid_coords(self.grid, tuple(self.data.dims)).items():
            if name not in dataset.coords:
                dataset = dataset.assign_coords({name: values})
        dataset = _add_common_coordinates(dataset, self.grid, self.provenance)
        dataset.attrs["radiust_provenance"] = self.provenance
        dataset.attrs["radiust_grid_kind"] = self.grid.kind
        return dataset

    def select(self, variable: str) -> RadarField:
        if variable != self.variable:
            raise KeyError(variable)
        return self

    def regrid(self, target: Grid, *, method: str = "nearest") -> RadarField:
        if method not in {"nearest", "bilinear"}:
            raise ValueError("method must be nearest or bilinear")
        if method == "bilinear" and _is_categorical(self):
            raise GridError("categorical fields cannot use bilinear interpolation")
        if not isinstance(self.grid, (GeographicGrid, CartesianGrid)) or not isinstance(target, (GeographicGrid, CartesianGrid)):
            raise GridError("explicit regrid currently requires regular geographic/cartesian grids")
        source_x = np.asarray(self.grid.longitude if isinstance(self.grid, GeographicGrid) else self.grid.x)
        source_y = np.asarray(self.grid.latitude if isinstance(self.grid, GeographicGrid) else self.grid.y)
        target_x, target_y = _target_mesh_in_source_crs(self.grid, target)
        values = np.asarray(self.data.values, dtype=np.float32)
        source_quality = None if self.quality is None else np.asarray(self.quality.values, dtype="uint16")
        if source_x[0] > source_x[-1]:
            source_x, values = source_x[::-1], values[:, ::-1]
            if source_quality is not None:
                source_quality = source_quality[:, ::-1]
        if source_y[0] > source_y[-1]:
            source_y, values = source_y[::-1], values[::-1, :]
            if source_quality is not None:
                source_quality = source_quality[::-1, :]
        xi = _nearest_indices(source_x, target_x)
        yi = _nearest_indices(source_y, target_y)
        inside = (
            np.isfinite(target_x)
            & np.isfinite(target_y)
            & (target_x >= source_x[0])
            & (target_x <= source_x[-1])
            & (target_y >= source_y[0])
            & (target_y <= source_y[-1])
        )
        if method == "nearest":
            result = values[yi, xi].astype(np.float32)
            result[~inside] = np.nan
        else:
            result, bilinear_quality = _bilinear(values, source_x, source_y, target_x, target_y, self.data.attrs.get("units", ""), source_quality)
        if method == "nearest":
            quality = None
            if source_quality is not None:
                quality = source_quality[yi, xi].astype("uint16")
                quality[~inside] |= np.uint16(2)
        else:
            quality = bilinear_quality if source_quality is not None else None
        dims = ("latitude", "longitude") if isinstance(target, GeographicGrid) else ("y", "x")
        coords = _grid_coords(target)
        array = xr.DataArray(result.astype(np.float32), dims=dims, coords=coords, name=self.data.name, attrs=dict(self.data.attrs))
        if quality is not None:
            quality = xr.DataArray(quality.astype("uint16"), dims=dims, coords=coords, name="quality")
        return RadarField(array, target, quality, dict(self.provenance), self.processing_history + ({"op": "regrid", "method": method, "target": target.kind},))

    def to_geographic(self, *, bbox: tuple[float, float, float, float], resolution: float, method: str = "nearest") -> RadarField:
        west, south, east, north = bbox
        if not all(np.isfinite(value) for value in bbox) or resolution <= 0:
            raise GridError("bbox must be finite and resolution must be positive")
        if west < -180 or east > 180 or south < -90 or north > 90 or west >= east or south >= north:
            raise GridError("bbox must have positive area and cannot cross the date line")
        target = GeographicGrid(np.arange(west, east + resolution / 2, resolution), np.arange(south, north + resolution / 2, resolution))
        return self.regrid(target, method=method)


@dataclass(frozen=True, slots=True)
class RadarDataset:
    data: xr.Dataset
    grid: Grid
    quality: xr.DataArray | None = None
    provenance: dict[str, Any] = dc_field(default_factory=dict)

    def __post_init__(self) -> None:
        self.validate()

    def validate(self) -> None:
        self.grid.validate()
        scientific = [name for name in self.data.data_vars if name != "quality"]
        if not scientific:
            raise ValueError("RadarDataset must contain at least one variable")
        for name, value in self.data.data_vars.items():
            if name == "quality":
                continue
            if value.ndim != 2 or tuple(value.shape) != self.grid.shape:
                raise ValueError(f"variable {name} does not match the grid")
        if self.quality is not None:
            if self.quality.dtype != np.dtype("uint16"):
                raise TypeError("quality must preserve uint16 flags")
            if self.quality.ndim != 2 or tuple(self.quality.shape) != self.grid.shape:
                raise ValueError("quality must match the grid shape")

    def select(self, variable: str) -> RadarField:
        if variable == "quality" or variable not in self.data.data_vars:
            raise KeyError(variable)
        return RadarField(self.data[variable], self.grid, self.quality, dict(self.provenance))

    def regrid(self, target: Grid, *, method: str = "nearest") -> RadarDataset:
        transformed = []
        transformed_quality = []
        for name, value in self.data.data_vars.items():
            if name == "quality":
                continue
            transformed_field = RadarField(value, self.grid, self.quality, dict(self.provenance)).regrid(target, method=method)
            transformed.append(transformed_field.data.to_dataset(name=name))
            if transformed_field.quality is not None:
                transformed_quality.append(transformed_field.quality)
        if not transformed:
            raise ValueError("RadarDataset has no scientific variables to regrid")
        dataset = xr.merge(transformed, join="exact", compat="equals")
        quality = None
        if transformed_quality:
            # A dataset has one shared quality plane. Preserve interpolation
            # and missing flags produced by each actual scientific variable.
            combined = np.array(transformed_quality[0].values, dtype="uint16", copy=True)
            for variable_quality in transformed_quality[1:]:
                np.bitwise_or(combined, variable_quality.values, out=combined)
            quality = xr.DataArray(combined, dims=transformed_quality[0].dims, coords=transformed_quality[0].coords, name="quality")
        return RadarDataset(dataset, target, quality, dict(self.provenance))

    def to_geographic(self, *, bbox: tuple[float, float, float, float], resolution: float, method: str = "nearest") -> RadarDataset:
        if not all(np.isfinite(value) for value in bbox) or resolution <= 0:
            raise GridError("bbox must be finite and resolution must be positive")
        if bbox[0] < -180 or bbox[2] > 180 or bbox[1] < -90 or bbox[3] > 90 or bbox[0] >= bbox[2] or bbox[1] >= bbox[3]:
            raise GridError("bbox must have positive area and cannot cross the date line")
        target = GeographicGrid(
            np.arange(bbox[0], bbox[2] + resolution / 2, resolution),
            np.arange(bbox[1], bbox[3] + resolution / 2, resolution),
        )
        return self.regrid(target, method=method)

    def to_dataset(self) -> xr.Dataset:
        dataset = self.data.copy()
        if self.quality is not None and "quality" not in dataset.data_vars:
            dataset["quality"] = self.quality
            dataset["quality"].attrs.update(
                {
                    "flag_masks": np.array([1, 2, 4, 8, 16, 32], dtype="uint16"),
                    "flag_meanings": "missing outside_coverage unknown_color recovered interpolated below_detection",
                }
            )
            for name in dataset.data_vars:
                if name != "quality":
                    dataset[name].attrs.setdefault("ancillary_variables", "quality")
        first = next(iter(self.data.data_vars.values()))
        for name, values in _grid_coords(self.grid, tuple(first.dims)).items():
            if name not in dataset.coords:
                dataset = dataset.assign_coords({name: values})
        dataset = _add_common_coordinates(dataset, self.grid, self.provenance)
        dataset.attrs.setdefault("radiust_provenance", self.provenance)
        dataset.attrs.setdefault("radiust_grid_kind", self.grid.kind)
        return dataset


def _add_common_coordinates(dataset: xr.Dataset, grid: Grid, provenance: dict[str, Any]) -> xr.Dataset:
    """Attach scalar time and CF grid mapping metadata without changing data variables."""
    if "valid_time" in provenance and "time" not in dataset.coords:
        value = provenance["valid_time"]
        if isinstance(value, datetime):
            value = value.astimezone(timezone.utc).replace(tzinfo=None)
        elif isinstance(value, str):
            value = datetime.fromisoformat(value.replace("Z", "+00:00")).astimezone(timezone.utc).replace(tzinfo=None)
        dataset = dataset.assign_coords(time=np.datetime64(value))
    if "crs" not in dataset.coords:
        crs_value = 0
        crs_attrs: dict[str, Any] = {
            "long_name": "coordinate reference system",
            "spatial_ref": grid.crs or "unknown",
            "units": "1",
        }
        if grid.kind == "geographic":
            crs_attrs["grid_mapping_name"] = "latitude_longitude"
            if grid.crs == "EPSG:3821":
                # TWD67 uses the GRS 1967 Modified ellipsoid, not a sphere.
                crs_attrs["geographic_crs_name"] = "TWD67"
                crs_attrs["semi_major_axis"] = 6378160.0
                crs_attrs["inverse_flattening"] = 298.25
            elif grid.crs == "EPSG:4326":
                from pyproj import CRS

                ellipsoid = CRS.from_user_input(grid.crs).ellipsoid
                crs_attrs["semi_major_axis"] = ellipsoid.semi_major_metre
                crs_attrs["inverse_flattening"] = ellipsoid.inverse_flattening
        dataset = dataset.assign_coords(crs=xr.DataArray(crs_value, attrs=crs_attrs))
    dataset.attrs.setdefault("Conventions", "CF-1.8")
    if "time" in dataset.coords:
        dataset["time"].attrs.setdefault("standard_name", "time")
    if isinstance(grid, (GeographicGrid, CurvilinearGrid)):
        if "latitude" in dataset.coords:
            dataset["latitude"].attrs.setdefault("standard_name", "latitude")
            dataset["latitude"].attrs.setdefault("units", "degrees_north")
        if "longitude" in dataset.coords:
            dataset["longitude"].attrs.setdefault("standard_name", "longitude")
            dataset["longitude"].attrs.setdefault("units", "degrees_east")
    for name in dataset.data_vars:
        if name == "quality":
            dataset[name].attrs.setdefault("long_name", "quality flags")
            continue
        dataset[name].attrs.setdefault("long_name", name.replace("_", " "))
        dataset[name].attrs.setdefault("grid_mapping", "crs")
    return dataset
