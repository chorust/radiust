"""Small immutable-ish public value objects used by the Python facade."""

from __future__ import annotations

import math
from collections.abc import Mapping
from dataclasses import dataclass
from dataclasses import field as dc_field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import MappingProxyType
from typing import Any

from .errors import UnsupportedQueryError

UTC = timezone.utc
ALLOWED_AVAILABILITY = {
    "available",
    "missing_dependency",
    "needs_configuration",
    "upstream_unavailable",
    "retired",
}


def _freeze_mapping(value: Any) -> Any:
    """Recursively freeze user supplied mappings and sequences."""
    if isinstance(value, Mapping):
        return MappingProxyType({key: _freeze_mapping(item) for key, item in value.items()})
    if isinstance(value, list):
        return tuple(_freeze_mapping(item) for item in value)
    if isinstance(value, tuple):
        return tuple(_freeze_mapping(item) for item in value)
    return value


def utc_datetime(value: datetime) -> datetime:
    if not isinstance(value, datetime):
        raise TypeError("time must be a datetime")
    if value.tzinfo is None or value.utcoffset() is None:
        raise ValueError("naive datetime is not accepted; provide a timezone")
    return value.astimezone(UTC).replace(tzinfo=UTC)


def format_time(value: datetime) -> str:
    value = utc_datetime(value)
    return value.strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def validate_identifier(value: str, label: str = "identifier") -> str:
    if not isinstance(value, str) or not value or value in {".", ".."}:
        raise ValueError(f"{label} must be a non-empty string")
    if any(part in value for part in ("/", "\\", "\x00")):
        raise ValueError(f"{label} cannot contain a path separator")
    return value


@dataclass(frozen=True, slots=True)
class ProductInfo:
    id: str
    variables: tuple[str, ...] = ()
    units: Mapping[str, str] = dc_field(default_factory=dict)
    native_grid_kind: str = "geographic"
    cadence: timedelta | None = None
    publication_delay: timedelta | None = None
    suggested_max_age: timedelta | None = None
    historical: bool = False
    forecast: bool = False
    default: bool = False
    time_binding_policy: str = "valid_time"
    mutable: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "variables", tuple(self.variables))
        object.__setattr__(self, "units", _freeze_mapping(self.units))
        validate_identifier(self.id, "product id")
        if not self.variables:
            raise ValueError("product must declare at least one variable")
        if self.native_grid_kind not in {"geographic", "cartesian", "polar", "curvilinear"}:
            raise ValueError(f"unsupported native grid kind: {self.native_grid_kind}")
        if self.cadence is not None and self.cadence <= timedelta(0):
            raise ValueError("cadence must be positive")
        if self.suggested_max_age is not None and self.suggested_max_age <= timedelta(0):
            raise ValueError("suggested_max_age must be positive")


@dataclass(frozen=True, slots=True)
class StationInfo:
    id: str
    name: str
    longitude: float
    latitude: float
    altitude: float | None = None
    product_ids: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "product_ids", tuple(self.product_ids))
        validate_identifier(self.id, "station id")
        if not -180 <= self.longitude <= 180:
            raise ValueError("longitude must be between -180 and 180")
        if not -90 <= self.latitude <= 90:
            raise ValueError("latitude must be between -90 and 90")
        if not all(math.isfinite(v) for v in (self.longitude, self.latitude)):
            raise ValueError("station coordinates must be finite")


@dataclass(frozen=True, slots=True)
class SourceInfo:
    id: str
    description: str
    adapter_version: str
    products: tuple[ProductInfo, ...]
    stations: tuple[StationInfo, ...] = ()
    required_extras: tuple[str, ...] = ()
    availability: str = "available"
    availability_evidence: str | None = None

    def __post_init__(self) -> None:
        validate_identifier(self.id, "source id")
        if self.availability not in ALLOWED_AVAILABILITY:
            raise ValueError(f"invalid availability: {self.availability}")
        if sum(p.default for p in self.products) > 1:
            raise ValueError("a source can have at most one default product")
        product_ids = [p.id for p in self.products]
        if len(product_ids) != len(set(product_ids)):
            raise ValueError("duplicate product id")
        station_ids = [s.id for s in self.stations]
        if len(station_ids) != len(set(station_ids)):
            raise ValueError("duplicate station id")

    @property
    def default_product(self) -> ProductInfo | None:
        defaults = [p for p in self.products if p.default]
        return defaults[0] if defaults else (self.products[0] if len(self.products) == 1 else None)


@dataclass(frozen=True, slots=True)
class Query:
    source: str
    product: str | None = None
    stations: tuple[str, ...] = ()
    latest: bool = False
    at: datetime | None = None
    start: datetime | None = None
    end: datetime | None = None
    base_time: datetime | None = None
    max_age: timedelta | None = None

    def __post_init__(self) -> None:
        validate_identifier(self.source, "source")
        stations = tuple(self.stations)
        if len(stations) != len(set(stations)):
            raise UnsupportedQueryError("stations must not contain duplicates")
        object.__setattr__(self, "stations", stations)
        choices = int(self.latest) + int(self.at is not None) + int(self.start is not None or self.end is not None)
        if choices != 1:
            raise UnsupportedQueryError("choose exactly one of latest, at, or start/end")
        if (self.start is None) != (self.end is None):
            raise UnsupportedQueryError("start and end must be supplied together")
        if self.at is not None:
            object.__setattr__(self, "at", utc_datetime(self.at))
        if self.start is not None:
            start = utc_datetime(self.start)
            end = utc_datetime(self.end)  # type: ignore[arg-type]
            if start >= end:
                raise UnsupportedQueryError("query range must satisfy start < end")
            object.__setattr__(self, "start", start)
            object.__setattr__(self, "end", end)
        if self.base_time is not None:
            object.__setattr__(self, "base_time", utc_datetime(self.base_time))
        if self.max_age is not None and (self.max_age <= timedelta(0) or not self.latest):
            raise UnsupportedQueryError("max_age is positive and only valid with latest")


@dataclass(frozen=True, slots=True)
class FrameRef:
    source: str
    product: str
    valid_time: datetime
    station: str | None = None
    base_time: datetime | None = None
    uri: str | None = None
    locator: Mapping[str, Any] = dc_field(default_factory=dict)
    locator_version: str = "1"
    metadata: Mapping[str, Any] = dc_field(default_factory=dict)
    revision: str | None = None

    def __post_init__(self) -> None:
        validate_identifier(self.source, "source")
        validate_identifier(self.product, "product")
        if self.station is not None:
            validate_identifier(self.station, "station")
        object.__setattr__(self, "valid_time", utc_datetime(self.valid_time))
        if self.base_time is not None:
            object.__setattr__(self, "base_time", utc_datetime(self.base_time))
        validate_identifier(self.locator_version, "locator version")
        object.__setattr__(self, "locator", _freeze_mapping(self.locator))
        object.__setattr__(self, "metadata", _freeze_mapping(self.metadata))

    @property
    def logical_id(self) -> str:
        from .identity import logical_id

        return logical_id(self)


@dataclass(frozen=True, slots=True)
class Artifact:
    name: str
    role: str
    media_type: str
    payload: bytes | Path | str
    source_revision: str | None = None
    size_bytes: int | None = None
    sha256: str | None = None

    def __post_init__(self) -> None:
        validate_identifier(self.name, "artifact name")
        if self.role not in {"data", "metadata", "tile", "derived"}:
            raise ValueError(f"invalid artifact role: {self.role}")
        if isinstance(self.payload, str):
            object.__setattr__(self, "payload", Path(self.payload))
        if self.size_bytes is not None and self.size_bytes < 0:
            raise ValueError("size_bytes cannot be negative")


@dataclass(frozen=True, slots=True)
class ProcessingSpec:
    output_kind: str = "decoded"
    format: str = "netcdf"
    variable: str | None = None
    grid: str = "native"
    bbox: tuple[float, float, float, float] | None = None
    resolution: float | None = None
    resampling: str = "nearest"
    decoder_version: str = "1"
    resource_version: str = "1"
    encoder_version: str = "1"
    options: Mapping[str, Any] = dc_field(default_factory=dict)

    def __post_init__(self) -> None:
        if self.bbox is not None:
            object.__setattr__(self, "bbox", tuple(self.bbox))
        object.__setattr__(self, "options", _freeze_mapping(self.options))
        if self.output_kind not in {"decoded", "raw-only"}:
            raise ValueError("output_kind must be decoded or raw-only")
        if self.format not in {"netcdf", "geotiff", "png", "zarr"}:
            raise ValueError(f"unsupported output format: {self.format}")
        if self.grid not in {"native", "geographic"}:
            raise ValueError("grid must be native or geographic")
        if self.grid == "native" and (self.bbox is not None or self.resolution is not None):
            raise UnsupportedQueryError("bbox and resolution require grid=geographic")
        if self.grid == "geographic" and (self.bbox is None or self.resolution is None):
            raise UnsupportedQueryError("grid=geographic requires bbox and resolution")
        if self.resampling not in {"nearest", "bilinear"}:
            raise ValueError("unsupported resampling")
        if self.bbox is not None:
            if len(self.bbox) != 4 or not all(math.isfinite(v) for v in self.bbox):
                raise ValueError("bbox must be four finite values")
            west, south, east, north = self.bbox
            if not (-180 <= west <= 180 and -180 <= east <= 180 and -90 <= south <= 90 and -90 <= north <= 90):
                raise ValueError("bbox is outside geographic bounds")
            if west > east or south >= north:
                raise ValueError("bbox must not cross the date line and must have positive area")
        if self.resolution is not None and self.resolution <= 0:
            raise ValueError("resolution must be positive")
        if self.output_kind == "raw-only" and any(
            value is not None for value in (self.variable, self.bbox, self.resolution)
        ):
            raise UnsupportedQueryError("raw-only cannot request decoded processing options")


@dataclass(frozen=True, slots=True)
class Receipt:
    name: str
    size_bytes: int
    sha256: str
    retrieved_at: datetime
    source_revision: str | None = None

    def __post_init__(self) -> None:
        if self.size_bytes < 0:
            raise ValueError("receipt size cannot be negative")
        object.__setattr__(self, "retrieved_at", utc_datetime(self.retrieved_at))


@dataclass(frozen=True, slots=True)
class OutputRequest:
    ref: FrameRef
    processing: ProcessingSpec = dc_field(default_factory=ProcessingSpec)
    output_root: Path = Path("data")
    overwrite: bool = False
    raw: bool = False
    output_template: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "output_root", Path(self.output_root))
        if self.raw and self.processing.output_kind == "raw-only":
            raise ValueError("raw and raw-only are distinct output modes")


@dataclass(frozen=True, slots=True)
class FrameResult:
    ref: FrameRef
    status: str
    output_uri: str | None = None
    error: Mapping[str, Any] | None = None
    data: Any = dc_field(default=None, repr=False, compare=False)

    def __post_init__(self) -> None:
        if self.status not in {"success", "written", "skipped", "failed", "cancelled", "not_started", "planned"}:
            raise ValueError(f"invalid frame status: {self.status}")


@dataclass(frozen=True, slots=True)
class BatchResult:
    items: tuple[FrameResult, ...]

    @property
    def succeeded(self) -> tuple[FrameResult, ...]:
        return tuple(item for item in self.items if item.status in {"success", "written", "skipped"})

    @property
    def failed(self) -> tuple[FrameResult, ...]:
        return tuple(item for item in self.items if item.status == "failed")

    @property
    def counts(self) -> dict[str, int]:
        names = ("success", "written", "skipped", "failed", "cancelled", "not_started", "planned")
        return {name: sum(item.status == name for item in self.items) for name in names}

    def as_dict(self) -> dict[str, Any]:
        return {
            "schema_version": 1,
            "counts": self.counts,
            "items": [
                {
                    "source": item.ref.source,
                    "product": item.ref.product,
                    "station": item.ref.station,
                    "valid_time": format_time(item.ref.valid_time),
                    "logical_id": item.ref.logical_id,
                    "status": item.status,
                    "error": dict(item.error) if item.error else None,
                }
                for item in self.items
            ],
        }


@dataclass(frozen=True, slots=True)
class DownloadReport:
    command: str
    run_id: str
    items: tuple[FrameResult, ...]
    query: Mapping[str, Any] | None = None
    interrupted: bool = False

    @property
    def counts(self) -> dict[str, int]:
        names = ("written", "skipped", "failed", "cancelled", "not_started", "planned")
        return {name: sum(item.status == name for item in self.items) for name in names}

    def as_dict(self) -> dict[str, Any]:
        return {
            "schema_version": 1,
            "command": self.command,
            "run_id": self.run_id,
            "query": dict(self.query) if self.query is not None else None,
            "counts": self.counts,
            "items": [
                {
                    "source": item.ref.source,
                    "product": item.ref.product,
                    "station": item.ref.station,
                    "valid_time": format_time(item.ref.valid_time),
                    "logical_id": item.ref.logical_id,
                    "status": item.status,
                    "output_uri": item.output_uri,
                    "error": dict(item.error) if item.error else None,
                }
                for item in self.items
            ],
            "error": None,
            "interrupted": self.interrupted,
        }
