"""Source protocol and a fixture-backed adapter used by offline validation."""

from __future__ import annotations

import json
import mimetypes
from abc import ABC, abstractmethod
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import numpy as np
import xarray as xr
from PIL import Image

from ..context import SourceContext
from ..errors import (
    ConfigError,
    DecodeError,
    NoDataError,
    UnsupportedQueryError,
)
from ..field import RadarField
from ..grids import GeographicGrid
from ..models import Artifact, FrameRef, Query, SourceInfo, format_time, utc_datetime
from ..raw import RawFrame


class Source(ABC):
    """The three explicit source stages; adapters do not write final output."""

    info: SourceInfo

    @abstractmethod
    async def discover(self, query: Query, context: SourceContext) -> list[FrameRef]:
        raise NotImplementedError

    @abstractmethod
    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        raise NotImplementedError

    @abstractmethod
    def decode(self, raw: RawFrame, context: SourceContext) -> RadarField | xr.Dataset:
        raise NotImplementedError


def _parse_fixture_time(value: str) -> datetime:
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    return utc_datetime(parsed)


class FixtureSource(Source):
    """Reads a versioned local fixture and can be injected with a replay transport."""

    def __init__(self, info: SourceInfo, fixture_path: Path | None = None) -> None:
        self.info = info
        self.fixture_path = fixture_path

    def _fixture(self) -> dict[str, Any]:
        if self.fixture_path is None or not self.fixture_path.exists():
            return {}
        try:
            value = json.loads(self.fixture_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise DecodeError(f"invalid fixture for source {self.info.id}: {exc}") from exc
        if not isinstance(value, dict):
            raise DecodeError("fixture root must be an object")
        return value

    def _frame_from_entry(self, entry: dict[str, Any]) -> FrameRef:
        product = str(entry.get("product") or self.info.default_product.id)  # type: ignore[union-attr]
        station = entry.get("station")
        return FrameRef(
            source=self.info.id,
            product=product,
            station=station,
            valid_time=_parse_fixture_time(str(entry["valid_time"])),
            base_time=_parse_fixture_time(str(entry["base_time"])) if entry.get("base_time") else None,
            uri=entry.get("uri"),
            locator=entry.get("locator", {"fixture": True}),
            locator_version=str(entry.get("locator_version", "1")),
            metadata=entry.get("metadata", {}),
            revision=entry.get("revision"),
        )

    async def discover(self, query: Query, context: SourceContext) -> list[FrameRef]:
        context.cancellation.check()
        if query.source != self.info.id:
            raise UnsupportedQueryError(f"source mismatch: {query.source}")
        fixture = self._fixture()
        if not fixture and self.info.availability != "available":
            evidence = self.info.availability_evidence or self.info.availability
            raise ConfigError(f"source {self.info.id} is not available: {evidence}")
        refs = [self._frame_from_entry(item) for item in fixture.get("frames", [])]
        if query.product:
            refs = [ref for ref in refs if ref.product == query.product]
        else:
            default = self.info.default_product
            if default is None:
                raise UnsupportedQueryError(f"source {self.info.id} requires an explicit product")
            refs = [ref for ref in refs if ref.product == default.id]
        if query.stations:
            refs = [ref for ref in refs if ref.station in query.stations]
        if query.base_time is not None:
            refs = [ref for ref in refs if ref.base_time == query.base_time]
        if query.at is not None:
            refs = [ref for ref in refs if ref.valid_time == query.at]
        elif query.start is not None:
            refs = [ref for ref in refs if query.start <= ref.valid_time < query.end]  # type: ignore[operator]
        elif query.latest:
            groups: dict[tuple[str, str | None], FrameRef] = {}
            for ref in refs:
                key = (ref.product, ref.station)
                if key not in groups or ref.valid_time > groups[key].valid_time:
                    groups[key] = ref
            refs = list(groups.values())
            if query.max_age:
                now = datetime.now(timezone.utc)
                refs = [ref for ref in refs if now - ref.valid_time <= query.max_age or ref.valid_time > now]
        refs.sort(key=lambda ref: (ref.valid_time, ref.source, ref.product, ref.station or "", ref.base_time or datetime.min.replace(tzinfo=timezone.utc), ref.logical_id))
        return refs

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        context.cancellation.check()
        fixture = self._fixture()
        entry = next((item for item in fixture.get("frames", []) if self._frame_from_entry(item).logical_id == ref.logical_id), None)
        if entry is None:
            raise NoDataError(f"no fixture frame for {ref.logical_id}")
        artifacts: list[Artifact] = []
        for item in entry.get("artifacts", []):
            path = Path(item["path"])
            if not path.is_absolute() and self.fixture_path is not None:
                path = self.fixture_path.parent / path
            if not path.exists():
                raise NoDataError(f"fixture artifact does not exist: {path}")
            if path.stat().st_size > int(context.limits["max_artifact_bytes"]):
                context.check_bytes(path.stat().st_size)
            artifacts.append(
                Artifact(
                    name=str(item.get("name", path.name)),
                    role=str(item.get("role", "data")),
                    media_type=str(item.get("media_type") or mimetypes.guess_type(path.name)[0] or "application/octet-stream"),
                    payload=path,
                    source_revision=item.get("source_revision"),
                    size_bytes=int(item["size_bytes"]) if item.get("size_bytes") is not None else None,
                    sha256=item.get("sha256"),
                )
            )
        if not artifacts:
            raise NoDataError(f"fixture has no artifacts for {ref.logical_id}")
        return RawFrame(ref, tuple(artifacts), metadata=dict(entry.get("metadata", {})))

    def decode(self, raw: RawFrame, context: SourceContext) -> RadarField | xr.Dataset:
        raw._ensure_open()
        artifact = raw.artifacts[0]
        path = Path(artifact.payload) if not isinstance(artifact.payload, bytes) else None
        try:
            if path and path.suffix.lower() in {".nc", ".netcdf"}:
                return xr.open_dataset(path).load()
            with Image.open(path if path else __import__("io").BytesIO(artifact.payload)) as image:
                rgba = np.asarray(image.convert("RGBA"), dtype=np.uint8)
        except Exception as exc:
            raise DecodeError(f"unable to decode {artifact.name}: {exc}") from exc
        # Generic source adapters preserve display information as a finite field;
        # source-specific palettes can replace this implementation.
        values = rgba[..., :3].mean(axis=2).astype(np.float32)
        alpha = rgba[..., 3]
        quality = np.where(alpha == 0, 1, 0).astype("uint16")
        height, width = values.shape
        grid = GeographicGrid(np.linspace(-180, 180, width), np.linspace(90, -90, height))
        product = next((item for item in self.info.products if item.id == raw.ref.product), self.info.products[0])
        variable = product.variables[0]
        data = xr.DataArray(
            values,
            dims=("latitude", "longitude"),
            coords={"latitude": np.asarray(grid.latitude), "longitude": np.asarray(grid.longitude)},
            name=variable,
            attrs={"units": product.units.get(variable, "1")},
        )
        q = xr.DataArray(quality, dims=data.dims, coords=data.coords, name="quality")
        return RadarField(
            data,
            grid,
            q,
            {
                "source": self.info.id,
                "product": raw.ref.product,
                "station": raw.ref.station,
                "valid_time": format_time(raw.ref.valid_time),
                "display_decoded": True,
                "raw": artifact.name,
            },
        )
