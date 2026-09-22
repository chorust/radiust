"""RainViewer public radar tile adapter.

RainViewer publishes a small JSON timeline and global Web Mercator radar
tiles.  The adapter keeps every fetched tile as a raw artifact and only
decodes the tile palette after acquisition, so raw-only downloads remain
replayable.
"""

from __future__ import annotations

import hashlib
import io
import json
from datetime import datetime, timezone
from importlib import resources
from typing import Any
from urllib.parse import urlparse

import numpy as np
import xarray as xr
from PIL import Image

from ..context import SourceContext
from ..errors import DecodeError, IntegrityError, NoDataError, TransportError, UnsupportedQueryError
from ..field import RadarField
from ..grids import GeographicGrid
from ..identity import artifact_bytes
from ..models import Artifact, FrameRef, Query, SourceInfo, format_time
from ..raw import RawFrame
from .base import Source

API_URL = "https://api.rainviewer.com/public/weather-maps.json"
TILE_HOST = "tilecache.rainviewer.com"
TILE_SIZE = 512
ZOOM = 1
COLOR_SCHEME = 2
TILE_OPTIONS = "0_0"

# The official Universal Blue table is indexed by dBZ.  Transparent entries
# below -10 dBZ are handled as missing coverage; only non-transparent entries
# are used as scientific values.  Duplicate display colors use the lowest
# dBZ value, which is deterministic and preserves the table's lower bound.
_UNIVERSAL_BLUE_HEX = """
63615914 66635a19 69665c1e 6c685d24 6f6b5f29 726e612e 75706234
78736439 7c75653e 7f786744 827b6949 857d6a4e 88806c54 8b826d59
8e856f5e 92887164 9e93756e aa9e7978 b6a97e82 c2b4828c cec08796
d2c48ba0 d6c88faa dacc93b4 ded097be 88ddeeff 6cd1ebff 51c5e8ff
36bae5ff 1baee2ff 00a3e0ff 009ad5ff 0091caff 0088bfff 007fb4ff
0077aaff 0070a3ff 00699cff 006295ff 005b8eff 005588ff 005180ff
004e78ff 004a70ff 004768ff ffee00ff ffe000ff ffd200ff ffc500ff
ffb700ff ffaa00ff ff9f00ff ff9500ff ff8b00ff ff8100ff ff4400ff
f23600ff e62800ff d91b00ff cd0d00ff c10000ff a80000ff 8f0000ff
760000ff 5d0000ff ffaaffff ff9fffff ff95ffff ff8bffff ff81ffff
ff77ffff ff6cffff ff62ffff ff58ffff ff4effff ffffffff ffffffff
ffffffff ffffffff ffffffff ffffffff ffffffff ffffffff ffffffff ffffffff
00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff
00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff
00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff 00ff00ff
"""

_COLOR_TO_DBZ: dict[int, float] = {}
_COLOR_BYTES = tuple(bytes.fromhex(value) for value in _UNIVERSAL_BLUE_HEX.split())
for _dbz, _rgba in zip(range(-10, 96), _COLOR_BYTES, strict=True):
    _key = int.from_bytes(_rgba, "big")
    _COLOR_TO_DBZ.setdefault(_key, float(_dbz))


def _resource_config() -> dict[str, Any]:
    path = resources.files("radiust.resources").joinpath("sources/rainviewer.json")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise DecodeError(f"invalid RainViewer resource: {exc}") from exc
    if not isinstance(value, dict):
        raise DecodeError("RainViewer resource must be an object")
    return value


def _epoch(value: Any) -> datetime:
    try:
        return datetime.fromtimestamp(int(value), timezone.utc)
    except (TypeError, ValueError, OverflowError, OSError) as exc:
        raise DecodeError(f"invalid RainViewer frame time: {value!r}") from exc


def _safe_host(host: str) -> str:
    parsed = urlparse(host)
    if parsed.scheme != "https" or parsed.hostname != TILE_HOST or parsed.path or parsed.query or parsed.fragment:
        raise TransportError("RainViewer returned an unsafe tile host")
    return host.rstrip("/")


def _frame_path(value: Any) -> str:
    if not isinstance(value, str) or not value.startswith("/v2/radar/") or ".." in value.split("/"):
        raise DecodeError("RainViewer returned an invalid radar frame path")
    return value.rstrip("/")


def _tile_url(host: str, path: str, x: int, y: int) -> str:
    return f"{host}{path}/{TILE_SIZE}/{ZOOM}/{x}/{y}/{COLOR_SCHEME}/{TILE_OPTIONS}.png"


def _global_coordinates(width: int, height: int) -> tuple[np.ndarray, np.ndarray]:
    world_width = (1 << ZOOM) * TILE_SIZE
    x = (np.arange(width, dtype=np.float64) + 0.5) / world_width
    y = (np.arange(height, dtype=np.float64) + 0.5) / world_width
    longitude = x * 360.0 - 180.0
    latitude = np.degrees(np.arctan(np.sinh(np.pi * (1.0 - 2.0 * y))))
    return longitude, latitude


def _decode_rgba(rgba: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    pixels = np.asarray(rgba, dtype=np.uint8)
    packed = (
        (pixels[..., 0].astype(np.uint32) << 24)
        | (pixels[..., 1].astype(np.uint32) << 16)
        | (pixels[..., 2].astype(np.uint32) << 8)
        | pixels[..., 3].astype(np.uint32)
    )
    unique, inverse = np.unique(packed, return_inverse=True)
    values = np.asarray([_COLOR_TO_DBZ.get(int(value), np.nan) for value in unique], dtype=np.float32)[inverse]
    known = np.isfinite(values)
    alpha = pixels[..., 3]
    quality = np.where(alpha == 0, 1, np.where(known, 0, 4)).astype("uint16")
    values = values.reshape(pixels.shape[:2])
    values[quality != 0] = np.nan
    return values, quality


class RainViewerSource(Source):
    """Source adapter for the public RainViewer past-radar timeline."""

    API_URL = API_URL

    def __init__(self, info: SourceInfo) -> None:
        self.info = info
        self.resource = _resource_config()

    def _product(self, query: Query) -> str:
        product = self.info.default_product
        if product is None:
            raise UnsupportedQueryError("RainViewer has no default product")
        if query.product is not None and query.product != product.id:
            raise UnsupportedQueryError(f"unsupported RainViewer product: {query.product}")
        return product.id

    async def _manifest(self, context: SourceContext) -> dict[str, Any]:
        context.cancellation.check()
        try:
            payload = await context.transport.get(self.API_URL)
            value = json.loads(payload)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DecodeError(f"invalid RainViewer API response: {exc}") from exc
        if not isinstance(value, dict):
            raise DecodeError("RainViewer API response must be an object")
        return value

    async def discover(self, query: Query, context: SourceContext) -> list[FrameRef]:
        if query.source != self.info.id:
            raise UnsupportedQueryError(f"source mismatch: {query.source}")
        self._product(query)
        if query.stations:
            raise UnsupportedQueryError("RainViewer does not expose station selection")
        if query.base_time is not None:
            raise UnsupportedQueryError("RainViewer does not expose base times")
        document = await self._manifest(context)
        radar = document.get("radar")
        if not isinstance(radar, dict):
            raise NoDataError("RainViewer response has no radar timeline")
        host = _safe_host(str(document.get("host", "")))
        frames = radar.get("past", [])
        if not isinstance(frames, list):
            raise DecodeError("RainViewer radar.past must be an array")
        refs: list[FrameRef] = []
        for item in frames:
            if not isinstance(item, dict):
                continue
            timestamp = _epoch(item.get("time"))
            path = _frame_path(item.get("path"))
            revision = path.rsplit("/", 1)[-1]
            refs.append(
                FrameRef(
                    source=self.info.id,
                    product=self.info.default_product.id,  # type: ignore[union-attr]
                    valid_time=timestamp,
                    uri=f"{host}{path}",
                    locator={
                        "api_url": self.API_URL,
                        "host": host,
                        "path": path,
                        "tile_size": TILE_SIZE,
                        "zoom": ZOOM,
                        "color": COLOR_SCHEME,
                        "options": TILE_OPTIONS,
                    },
                    locator_version="rainviewer-v2",
                    metadata={
                        "time_semantics": "frame_generation_time",
                        "api_generated": document.get("generated"),
                    },
                    revision=revision,
                )
            )
        refs.sort(key=lambda ref: ref.valid_time)
        if query.at is not None:
            refs = [ref for ref in refs if ref.valid_time == query.at]
        elif query.start is not None:
            refs = [ref for ref in refs if query.start <= ref.valid_time < query.end]  # type: ignore[operator]
        elif query.latest:
            refs = refs[-1:] if refs else []
            if refs and query.max_age is not None:
                age = datetime.now(timezone.utc) - refs[0].valid_time
                if age > query.max_age and age.total_seconds() >= 0:
                    refs = []
        if not refs:
            raise NoDataError("RainViewer has no matching radar frame")
        return refs

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        context.cancellation.check()
        locator = dict(ref.locator)
        host = _safe_host(str(locator.get("host", "")))
        path = _frame_path(locator.get("path"))
        if int(locator.get("tile_size", TILE_SIZE)) != TILE_SIZE or int(locator.get("zoom", ZOOM)) != ZOOM:
            raise IntegrityError("unsupported RainViewer tile plan")
        root = context.temp_root
        if root is None:
            raise RuntimeError("source context has no temporary root")
        artifacts: list[Artifact] = []
        for y in range(1 << ZOOM):
            for x in range(1 << ZOOM):
                context.cancellation.check()
                payload = await context.transport.get(_tile_url(host, path, x, y))
                context.cancellation.check()
                context.check_bytes(len(payload))
                try:
                    with Image.open(io.BytesIO(payload)) as image:
                        if image.size != (TILE_SIZE, TILE_SIZE):
                            raise IntegrityError(f"RainViewer tile {(x, y)} has size {image.size}")
                        image.verify()
                except IntegrityError:
                    raise
                except Exception as exc:
                    raise DecodeError(f"invalid RainViewer tile {(x, y)}: {exc}") from exc
                name = f"tile-z{ZOOM}-x{x}-y{y}.png"
                target = root / name
                target.write_bytes(payload)
                artifacts.append(
                    Artifact(
                        name=name,
                        role="tile",
                        media_type="image/png",
                        payload=target,
                        source_revision=ref.revision,
                        size_bytes=len(payload),
                        sha256=hashlib.sha256(payload).hexdigest(),
                    )
                )
        return RawFrame(
            ref,
            tuple(artifacts),
            metadata={
                "host": host,
                "path": path,
                "tile_size": TILE_SIZE,
                "zoom": ZOOM,
                "color": COLOR_SCHEME,
                "options": TILE_OPTIONS,
                "time_semantics": ref.metadata.get("time_semantics"),
            },
        )

    def decode(self, raw: RawFrame, context: SourceContext) -> RadarField:
        raw._ensure_open()
        context.cancellation.check()
        tile_size = int(raw.metadata.get("tile_size", TILE_SIZE))
        zoom = int(raw.metadata.get("zoom", ZOOM))
        expected = (1 << zoom) ** 2
        if len(raw.artifacts) != expected:
            raise DecodeError(f"RainViewer raw frame has {len(raw.artifacts)} tiles, expected {expected}")
        mosaic_shape = ((1 << zoom) * tile_size, (1 << zoom) * tile_size)
        mosaic_rgba = np.zeros((*mosaic_shape, 4), dtype=np.uint8)
        for artifact in raw.artifacts:
            name = artifact.name
            try:
                parts = name.removesuffix(".png").split("-")
                x = int(parts[-2][1:])
                y = int(parts[-1][1:])
            except (IndexError, ValueError) as exc:
                raise DecodeError(f"invalid RainViewer tile name: {name}") from exc
            with Image.open(io.BytesIO(artifact_bytes(artifact))) as image:
                tile = np.asarray(image.convert("RGBA"), dtype=np.uint8)
            if tile.shape[:2] != (tile_size, tile_size):
                raise DecodeError(f"RainViewer tile {name} has unexpected shape {tile.shape[:2]}")
            mosaic_rgba[y * tile_size : (y + 1) * tile_size, x * tile_size : (x + 1) * tile_size] = tile
        values, quality = _decode_rgba(mosaic_rgba)
        context.check_pixels(values.size + quality.size)
        longitude, latitude = _global_coordinates(values.shape[1], values.shape[0])
        grid = GeographicGrid(longitude, latitude)
        product = self.info.default_product
        if product is None:
            raise DecodeError("RainViewer has no default product")
        variable = product.variables[0]
        data = xr.DataArray(
            values,
            dims=("latitude", "longitude"),
            coords={"latitude": latitude, "longitude": longitude},
            name=variable,
            attrs={"units": product.units[variable], "standard_name": "equivalent_reflectivity_factor"},
        )
        quality_array = xr.DataArray(quality, dims=data.dims, coords=data.coords, name="quality")
        return RadarField(
            data,
            grid,
            quality_array,
            provenance={
                "source": self.info.id,
                "product": raw.ref.product,
                "valid_time": format_time(raw.ref.valid_time),
                "time_semantics": raw.metadata.get("time_semantics"),
                "upstream_uri": raw.ref.uri,
                "tile_plan": {"zoom": zoom, "tile_size": tile_size, "color": COLOR_SCHEME, "options": TILE_OPTIONS},
                "decoder": "rainviewer-universal-blue-v1",
            },
        )


__all__ = ["RainViewerSource"]
