"""Validated native grid descriptions."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import numpy as np

from ..errors import GridError


def _axis(values: Any, name: str) -> tuple[float, ...]:
    array = np.asarray(values, dtype=np.float64)
    if array.ndim != 1 or array.size < 1 or not np.isfinite(array).all():
        raise GridError(f"{name} must be a finite one-dimensional coordinate")
    delta = np.diff(array)
    if array.size > 1 and not (np.all(delta > 0) or np.all(delta < 0)):
        raise GridError(f"{name} must be strictly monotonic")
    return tuple(float(value) for value in array)


class Grid:
    kind: str
    shape: tuple[int, int]
    crs: str | None

    def validate(self) -> None:
        if len(self.shape) != 2 or any(value < 1 for value in self.shape):
            raise GridError("grid shape must contain two positive axes")


@dataclass(frozen=True, slots=True)
class GeographicGrid(Grid):
    longitude: tuple[float, ...]
    latitude: tuple[float, ...]

    def __init__(self, longitude: Any, latitude: Any, crs: str = "EPSG:4326") -> None:
        lon = _axis(longitude, "longitude")
        lat = _axis(latitude, "latitude")
        if any(value < -180 or value > 180 for value in lon):
            raise GridError("longitude is outside [-180, 180]")
        if any(value < -90 or value > 90 for value in lat):
            raise GridError("latitude is outside [-90, 90]")
        object.__setattr__(self, "kind", "geographic")
        object.__setattr__(self, "shape", (len(lat), len(lon)))
        object.__setattr__(self, "crs", crs)
        object.__setattr__(self, "longitude", lon)
        object.__setattr__(self, "latitude", lat)

    @property
    def extent(self) -> tuple[float, float, float, float]:
        return (self.longitude[0], self.latitude[0], self.longitude[-1], self.latitude[-1])


@dataclass(frozen=True, slots=True)
class CartesianGrid(Grid):
    x: tuple[float, ...]
    y: tuple[float, ...]

    def __init__(self, x: Any, y: Any, crs: str | None = None) -> None:
        xv, yv = _axis(x, "x"), _axis(y, "y")
        object.__setattr__(self, "kind", "cartesian")
        object.__setattr__(self, "shape", (len(yv), len(xv)))
        object.__setattr__(self, "crs", crs)
        object.__setattr__(self, "x", xv)
        object.__setattr__(self, "y", yv)


@dataclass(frozen=True, slots=True)
class PolarGrid(Grid):
    azimuth: tuple[float, ...]
    range_m: tuple[float, ...]
    site_longitude: float
    site_latitude: float
    site_altitude: float | None = None
    elevation: float | None = None
    beam_model: str | None = None

    def __init__(
        self,
        azimuth: Any,
        range_m: Any,
        *,
        site_longitude: float,
        site_latitude: float,
        site_altitude: float | None = None,
        elevation: float | None = None,
        beam_model: str | None = None,
    ) -> None:
        az, ran = _axis(azimuth, "azimuth"), _axis(range_m, "range")
        if not -180 <= site_longitude <= 180 or not -90 <= site_latitude <= 90:
            raise GridError("polar site coordinates are outside geographic bounds")
        object.__setattr__(self, "kind", "polar")
        object.__setattr__(self, "shape", (len(az), len(ran)))
        object.__setattr__(self, "crs", "EPSG:4326")
        object.__setattr__(self, "azimuth", az)
        object.__setattr__(self, "range_m", ran)
        object.__setattr__(self, "site_longitude", float(site_longitude))
        object.__setattr__(self, "site_latitude", float(site_latitude))
        object.__setattr__(self, "site_altitude", site_altitude)
        object.__setattr__(self, "elevation", elevation)
        object.__setattr__(self, "beam_model", beam_model)


@dataclass(frozen=True, slots=True)
class CurvilinearGrid(Grid):
    longitude: np.ndarray
    latitude: np.ndarray

    def __init__(self, longitude: Any, latitude: Any, crs: str = "EPSG:4326") -> None:
        lon, lat = np.asarray(longitude, dtype=np.float64), np.asarray(latitude, dtype=np.float64)
        if lon.ndim != 2 or lat.shape != lon.shape or not np.isfinite(lon).all() or not np.isfinite(lat).all():
            raise GridError("curvilinear longitude/latitude must be finite 2-D arrays of equal shape")
        if any(value < 1 for value in lon.shape):
            raise GridError("curvilinear grid axes must be non-empty")
        lon = lon.copy()
        lat = lat.copy()
        lon.setflags(write=False)
        lat.setflags(write=False)
        object.__setattr__(self, "kind", "curvilinear")
        object.__setattr__(self, "shape", tuple(int(v) for v in lon.shape))
        object.__setattr__(self, "crs", crs)
        object.__setattr__(self, "longitude", lon)
        object.__setattr__(self, "latitude", lat)
