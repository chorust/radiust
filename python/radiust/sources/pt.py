"""Portugal IPMA Madeira radar adapter."""

from __future__ import annotations

import io
import json
from datetime import datetime, timezone
from importlib.resources import files
from pathlib import Path

import numpy as np
import xarray as xr
from PIL import Image

from ..context import SourceContext
from ..decoders.exact import QUALITY_BELOW_DETECTION, QUALITY_MISSING, QUALITY_NO_RAIN
from ..errors import DecodeError, UnknownColorError
from ..field import RadarField
from ..grids import GeographicGrid
from ..models import format_time
from ..raw import RawFrame
from .legacy import LegacyImageSource

_PALETTE_DOCUMENT = json.loads(
    files("radiust.resources")
    .joinpath("palettes", "pt_rain_intensity.json")
    .read_text(encoding="utf-8")
)
_RAMP_RGB = np.asarray(
    [tuple(bytes.fromhex(color.removeprefix("#"))) for color in _PALETTE_DOCUMENT["legend_rgb_ramp"]],
    dtype=np.int16,
)
_RAMP_Y = int(_PALETTE_DOCUMENT["legend_ramp_origin_y"]) + np.arange(len(_RAMP_RGB))
_CLASS_VALUES = np.asarray(
    [item["value"] for item in _PALETTE_DOCUMENT["legend_class_centers_high_to_low"]],
    dtype=np.uint8,
)
_CLASS_CENTERS_Y = np.asarray(
    [item["row"] for item in _PALETTE_DOCUMENT["legend_class_centers_high_to_low"]],
    dtype=np.float64,
)


class PtSource(LegacyImageSource):
    INDEX_URL = "https://www.ipma.pt/resources.www/transf/radar/imgs-radar-md.json"
    IMAGE_BASE = "https://www.ipma.pt/resources.www/transf/radar/mad/"
    REFERER = "https://www.ipma.pt/pt/otempo/obs.remote/index-md.jsp"
    MAP_SCRIPT_URL = "https://www.ipma.pt/pt/otempo/obs.remote/mapbuilder-md.js"
    LEGEND_URL = "https://www.ipma.pt/resources.www/transf/radar/png-geo/legenda-radar.png"
    # These are the bounds passed to Leaflet ImageOverlay for every Madeira
    # timeline entry in mapbuilder-md.js.
    IMAGE_BOUNDS = (-19.87034, 30.01467, -12.89697, 35.96053)

    async def discover_entries(self, context: SourceContext):
        payload = await self._get(context, self.INDEX_URL, headers={"Referer": self.REFERER})
        try:
            document = json.loads(payload)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DecodeError(f"IPMA returned invalid timeline JSON: {exc}") from exc
        madeira = document.get("Madeira", []) if isinstance(document, dict) else []
        entries = []
        for item in madeira:
            if not isinstance(item, dict):
                continue
            path, raw_time = item.get("path"), item.get("date")
            if not isinstance(path, str) or not isinstance(raw_time, str):
                continue
            try:
                valid_time = datetime.strptime(raw_time, "%Y-%m-%d %H:%M").replace(tzinfo=timezone.utc)
            except ValueError:
                continue
            entries.append(
                {
                    "url": self.IMAGE_BASE + path,
                    "station": "PTST2",
                    "valid_time": valid_time,
                    "headers": {"Referer": self.REFERER},
                    "revision": path.removesuffix(".png"),
                    "metadata": {
                        "time_semantics": "timeline_time_utc",
                        "geometry_status": "verified_ipma_image_overlay",
                        "image_bounds_wsen": list(self.IMAGE_BOUNDS),
                        "value_semantics": "ordinal_ipma_rainfall_intensity_category",
                        "palette": _PALETTE_DOCUMENT["id"],
                    },
                }
            )
        return entries

    def decode(self, raw: RawFrame, context: SourceContext) -> RadarField:
        """Decode IPMA's displayed rain-rate bands without inventing point rates."""

        raw._ensure_open()
        artifact = next((item for item in raw.artifacts if item.role == "data"), raw.artifacts[0])
        payload = artifact.payload if isinstance(artifact.payload, bytes) else Path(artifact.payload).read_bytes()
        try:
            with Image.open(io.BytesIO(payload)) as image:
                rgba = np.asarray(image.convert("RGBA"), dtype=np.uint8).copy()
        except Exception as exc:
            raise DecodeError(f"unable to decode IPMA radar image {artifact.name}: {exc}") from exc

        height, width = rgba.shape[:2]
        if height < 1 or width < 1:
            raise DecodeError("IPMA radar image must have non-empty dimensions")
        context.check_pixels(int(height * width) * 2)

        pixels = rgba.reshape(-1, 4)
        rgb = pixels[:, :3]
        alpha = pixels[:, 3]
        values = np.full(len(pixels), np.nan, dtype=np.float32)
        quality = np.full(len(pixels), QUALITY_MISSING, dtype=np.uint16)

        transparent = alpha == 0
        values[transparent] = 0
        quality[transparent] = QUALITY_BELOW_DETECTION

        opaque = alpha != 0
        opaque_black = opaque & np.all(rgb == 0, axis=1)
        quality[opaque_black] = QUALITY_MISSING
        colored = opaque & ~opaque_black
        if np.any(colored):
            colors, inverse = np.unique(rgb[colored], axis=0, return_inverse=True)
            differences = colors[:, None, :].astype(np.int16) - _RAMP_RGB[None, :, :]
            squared_distances = np.sum(differences.astype(np.int32) ** 2, axis=2)
            nearest = np.argmin(squared_distances, axis=1)
            distances = np.sqrt(squared_distances[np.arange(len(colors)), nearest])
            limit = float(_PALETTE_DOCUMENT["maximum_rgb_distance"])
            unknown = distances > limit
            if np.any(unknown):
                examples = ", ".join(str(tuple(int(channel) for channel in color)) for color in colors[unknown][:5])
                raise UnknownColorError(
                    f"IPMA rain-intensity image contains colors outside its official legend: {examples}"
                )
            nearest_y = _RAMP_Y[nearest]
            class_index = np.argmin(np.abs(nearest_y[:, None] - _CLASS_CENTERS_Y[None, :]), axis=1)
            values[colored] = _CLASS_VALUES[class_index][inverse].astype(np.float32)
            quality[colored] = QUALITY_NO_RAIN

        west, south, east, north = self.IMAGE_BOUNDS
        longitude = west + (np.arange(width, dtype=np.float64) + 0.5) * (east - west) / width
        latitude = north - (np.arange(height, dtype=np.float64) + 0.5) * (north - south) / height
        variable = "rain_intensity"
        categories = _PALETTE_DOCUMENT["categories"]
        data = xr.DataArray(
            values.reshape(height, width),
            dims=("latitude", "longitude"),
            coords={"latitude": latitude, "longitude": longitude},
            name=variable,
            attrs={
                "units": "1",
                "kind": "categorical",
                "long_name": "IPMA rainfall-intensity display category",
                "flag_values": [item["value"] for item in categories],
                "flag_meanings": " ".join(item["meaning"] for item in categories),
                "comment": (
                    "Ordinal bins read from the official IPMA rainfall-intensity legend; they are not recovered "
                    "point rainfall rates. Transparent pixels mean no displayed colored return and are marked "
                    "below-detection; opaque colors absent from the legend are rejected except opaque black, "
                    "which is retained as missing."
                ),
            },
        )
        quality_data = xr.DataArray(
            quality.reshape(height, width),
            dims=data.dims,
            coords=data.coords,
            name="quality",
        )
        receipt = next((item for item in raw.receipts if item.name == artifact.name), None)
        return RadarField(
            data,
            GeographicGrid(longitude, latitude),
            quality_data,
            {
                "source": self.info.id,
                "product": raw.ref.product,
                "station": raw.ref.station,
                "valid_time": format_time(raw.ref.valid_time),
                "value_semantics": "ordinal_ipma_rainfall_intensity_category",
                "palette": _PALETTE_DOCUMENT["id"],
                "palette_version": _PALETTE_DOCUMENT["version"],
                "legend_url": self.LEGEND_URL,
                "geometry": "official_leaflet_image_overlay_bounds",
                "image_bounds_wsen": list(self.IMAGE_BOUNDS),
                "raw": artifact.name,
                "raw_sha256": receipt.sha256 if receipt is not None else artifact.sha256,
            },
        )


__all__ = ["PtSource"]
