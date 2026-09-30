"""Thin Python view of the source catalog embedded in the Rust core.

Source discovery and acquisition are implemented by Rust adapters. This
module keeps the Python SDK's metadata DTOs available without constructing or
loading Python provider implementations.
"""

from __future__ import annotations

from datetime import timedelta
from typing import Any

from ._bridge import native_source_catalog
from .models import ProductInfo, SourceInfo, StationInfo


def _duration(seconds: float | None) -> timedelta | None:
    return None if seconds is None else timedelta(seconds=seconds)


def _info_from_dict(item: dict[str, Any]) -> SourceInfo:
    products = tuple(
        ProductInfo(
            id=product["id"],
            variables=tuple(product.get("variables", ("reflectivity",))),
            units=product.get("units", {}),
            native_grid_kind=product.get("native_grid_kind", "geographic"),
            cadence=_duration(product.get("cadence_seconds")),
            publication_delay=_duration(product.get("publication_delay_seconds")),
            suggested_max_age=_duration(product.get("suggested_max_age_seconds")),
            historical=bool(product.get("historical", False)),
            forecast=bool(product.get("forecast", False)),
            default=bool(product.get("default", False)),
            time_binding_policy=product.get("time_binding_policy", "valid_time"),
            mutable=bool(product.get("mutable", False)),
        )
        for product in item.get("products", ())
    )
    stations = tuple(
        StationInfo(
            id=station["id"],
            name=station.get("name") or station["id"],
            longitude=float(station.get("longitude") or 0.0),
            latitude=float(station.get("latitude") or 0.0),
            altitude=station.get("altitude"),
            product_ids=tuple(station.get("product_ids", ())),
        )
        for station in item.get("stations", ())
    )
    return SourceInfo(
        id=item["id"],
        description=item.get("description") or item["id"],
        adapter_version=item.get("adapter_version") or "1",
        products=products,
        stations=stations,
        required_extras=tuple(item.get("required_extras", ())),
        availability=item.get("availability", "available"),
        availability_evidence=item.get("availability_evidence"),
    )


class SourceRegistry:
    """Expose Rust catalog metadata as stable Python SDK value objects."""

    def __init__(self) -> None:
        payload = native_source_catalog()
        self._info = {
            info.id: info for info in (_info_from_dict(item) for item in payload["sources"])
        }

    def infos(self) -> tuple[SourceInfo, ...]:
        return tuple(self._info[source_id] for source_id in sorted(self._info))

    def get_info(self, source_id: str) -> SourceInfo:
        return self._info[source_id]


registry = SourceRegistry()


def sources() -> tuple[SourceInfo, ...]:
    return registry.infos()


def get_source_info(source_id: str) -> SourceInfo:
    return registry.get_info(source_id)


__all__ = ["SourceRegistry", "get_source_info", "registry", "sources"]
