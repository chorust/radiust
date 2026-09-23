"""Static source catalog with lazy adapter construction."""

from __future__ import annotations

import json
from collections.abc import Callable
from importlib import import_module, resources
from importlib.metadata import entry_points
from pathlib import Path
from typing import Any

from .errors import DuplicateSourceError, UnsupportedQueryError
from .models import ProductInfo, SourceInfo, StationInfo
from .sources.base import Source


def _duration(value: Any) -> Any:
    if value is None:
        return None
    from datetime import timedelta

    return timedelta(seconds=float(value))


def _info_from_dict(item: dict[str, Any]) -> SourceInfo:
    products = tuple(
        ProductInfo(
            id=p["id"],
            variables=tuple(p.get("variables", ["reflectivity"])),
            units=p.get("units", {}),
            native_grid_kind=p.get("native_grid_kind", "geographic"),
            cadence=_duration(p.get("cadence_seconds")),
            publication_delay=_duration(p.get("publication_delay_seconds")),
            suggested_max_age=_duration(p.get("suggested_max_age_seconds")),
            historical=bool(p.get("historical", False)),
            forecast=bool(p.get("forecast", False)),
            default=bool(p.get("default", False)),
            time_binding_policy=p.get("time_binding_policy", "valid_time"),
            mutable=bool(p.get("mutable", False)),
        )
        for p in item.get("products", [])
    )
    stations = tuple(
        StationInfo(
            id=s["id"],
            name=s.get("name", s["id"]),
            longitude=float(s.get("longitude", 0)),
            latitude=float(s.get("latitude", 0)),
            altitude=s.get("altitude"),
            product_ids=tuple(s.get("product_ids", [])),
        )
        for s in item.get("stations", [])
    )
    return SourceInfo(
        id=item["id"],
        description=item.get("description", item["id"]),
        adapter_version=item.get("adapter_version", "1"),
        products=products,
        stations=stations,
        required_extras=tuple(item.get("required_extras", [])),
        availability=item.get("availability", "available"),
        availability_evidence=item.get("availability_evidence"),
    )


class SourceRegistry:
    def __init__(self, catalog_path: Path | None = None) -> None:
        if catalog_path is None:
            catalog_path = Path(resources.files("radiust.resources").joinpath("catalog.json"))
        data = json.loads(catalog_path.read_text(encoding="utf-8"))
        entries = data.get("sources", data)
        self._info: dict[str, SourceInfo] = {}
        for item in entries:
            info = _info_from_dict(item)
            if info.id in self._info:
                raise DuplicateSourceError(f"duplicate source id: {info.id}")
            self._info[info.id] = info
        self._factories: dict[str, Callable[[SourceInfo], Source]] = {}
        self._instances: dict[str, Source] = {}
        # Entry-point metadata and factory imports can execute arbitrary
        # third-party code. In particular, discover-all must spawn its bounded
        # catalog worker before touching any plugin, not while importing the
        # CLI or constructing the global registry in the unbounded parent.
        self._plugins_loaded = False
        self._plugins_loading = False
        self._plugin_error: Exception | None = None
        if "my" in self._info:
            self._factories["my"] = lambda info: import_module("radiust.sources.my").MySource(
                info
            )
        if "rainviewer" in self._info:
            self._factories["rainviewer"] = lambda info: import_module("radiust.sources.rainviewer").RainViewerSource(info)
        if "id_sidarma" in self._info:
            self._factories["id_sidarma"] = lambda info: import_module("radiust.sources.id_sidarma").IdSidarmaSource(info)
        if "fr" in self._info:
            self._factories["fr"] = lambda info: import_module("radiust.sources.fr").FrSource(info)
        for source_id, module_name, class_name in (
            ("au", "au", "AuSource"),
            ("kr", "kr", "KrSource"),
            ("tw", "tw", "TwSource"),
            ("tw-http", "tw_http", "TwHttpSource"),
            ("ph", "ph", "PhSource"),
            ("vn", "vn", "VnSource"),
            ("es", "es", "EsSource"),
            ("ca", "ca", "CaSource"),
            ("th_royalrain", "th_royalrain", "ThRoyalRainSource"),
            ("pt", "pt", "PtSource"),
            ("sg", "sg", "SgSource"),
            ("nz", "nz", "NzSource"),
            ("id", "id", "IdSource"),
            ("cam", "cam", "CamSource"),
            ("uk", "uk", "UkSource"),
            ("th", "th", "ThSource"),
            ("windy", "windy", "WindySource"),
            ("wunderground", "wunderground", "WundergroundSource"),
            ("opensnow", "opensnow", "OpenSnowSource"),
            ("bmkg", "bmkg", "BmkgSource"),
        ):
            if source_id in self._info:
                self._factories[source_id] = lambda info, module_name=module_name, class_name=class_name: getattr(
                    import_module(f"radiust.sources.{module_name}"), class_name
                )(info)

    def _ensure_plugins(self) -> None:
        if self._plugin_error is not None:
            raise self._plugin_error
        if self._plugins_loaded:
            return
        if self._plugins_loading:
            raise RuntimeError("recursive source plugin loading")
        self._plugins_loading = True
        try:
            self._load_entry_points()
        except Exception as exc:
            # Preserve the failure instead of treating the partially loaded
            # registry as a complete catalog on the next attempt.
            self._plugin_error = exc
            raise
        else:
            self._plugins_loaded = True
        finally:
            self._plugins_loading = False

    @staticmethod
    def _fixture_path(source_id: str) -> Path:
        source_tree = Path(__file__).parent.parent.parent / "tests" / "fixtures" / "sources" / source_id / "fixture.json"
        if source_tree.exists():
            return source_tree
        package_fixture = Path(__file__).parent / "tests" / "fixtures" / "sources" / source_id / "fixture.json"
        if package_fixture.exists():
            return package_fixture
        installed_fixture = Path(__file__).parents[1] / "tests" / "fixtures" / "sources" / source_id / "fixture.json"
        return installed_fixture

    def _load_entry_points(self) -> None:
        """Load optional third-party sources without importing them during listing."""
        try:
            candidates = entry_points(group="radiust.sources")
        except TypeError:  # pragma: no cover - Python 3.10 compatibility
            candidates = entry_points().get("radiust.sources", ())
        for entry_point in candidates:
            loaded = entry_point.load()
            if isinstance(loaded, SourceInfo):
                info, factory = loaded, lambda _info, value=loaded: value  # type: ignore[assignment]
            elif isinstance(loaded, tuple) and len(loaded) == 2 and isinstance(loaded[0], SourceInfo):
                info, factory = loaded
            elif callable(loaded):
                candidate = loaded()
                info = candidate.info
                def factory(_info: SourceInfo, value: Source = candidate) -> Source:
                    return value
            else:
                raise TypeError(f"entry point {entry_point.name!r} must expose a SourceInfo or factory")
            self.register(info, factory)

    def infos(self) -> tuple[SourceInfo, ...]:
        self._ensure_plugins()
        return tuple(self._info[key] for key in sorted(self._info))

    def get_info(self, source_id: str) -> SourceInfo:
        self._ensure_plugins()
        return self._info[source_id]

    def register(self, info: SourceInfo, factory: Callable[[SourceInfo], Source]) -> None:
        if info.id in self._info or info.id in self._factories:
            raise DuplicateSourceError(f"duplicate source id: {info.id}")
        self._info[info.id] = info
        self._factories[info.id] = factory

    def get(self, source_id: str) -> Source:
        self._ensure_plugins()
        if source_id not in self._info:
            raise KeyError(source_id)
        if source_id not in self._instances:
            factory = self._factories.get(source_id)
            if factory is not None:
                self._instances[source_id] = factory(self._info[source_id])
            else:
                raise UnsupportedQueryError(f"source {source_id} has no registered adapter")
        return self._instances[source_id]


registry = SourceRegistry()


def sources() -> tuple[SourceInfo, ...]:
    return registry.infos()


def get_source(source_id: str) -> Source:
    return registry.get(source_id)
