"""Singapore MSS 240 km rain-area radar adapter."""

from __future__ import annotations

import io
import json
import re
from datetime import datetime
from importlib.resources import files
from pathlib import Path
from zoneinfo import ZoneInfo

import numpy as np
import xarray as xr
from PIL import Image

from ..context import SourceContext
from ..decoders.exact import QUALITY_BELOW_DETECTION, QUALITY_OUTSIDE, ExactPaletteDecoder
from ..errors import DecodeError, GeoreferencingError
from ..field import RadarField
from ..grids import CartesianGrid
from ..models import format_time
from ..raw import RawFrame
from .legacy import LegacyImageSource

_PALETTE_DOCUMENT = json.loads(
    files("radiust.resources").joinpath("palettes", "sg_rain_intensity.json").read_text(encoding="utf-8")
)
_CATEGORY_MEANINGS = tuple(category["meaning"] for category in _PALETTE_DOCUMENT["categories"])
_PALETTE = {
    (*bytes.fromhex(color.removeprefix("#")), 255): (category["value"], 0)
    for category in _PALETTE_DOCUMENT["categories"]
    for color in category["colors"]
}
_PALETTE[(0, 0, 0, 0)] = (0, int(QUALITY_BELOW_DETECTION))


class SgSource(LegacyImageSource):
    PAGE_URL = "https://www.weather.gov.sg/weather-rain-area-240km"
    RANGE_METERS = 240_000.0
    CENTER_LONGITUDE = 103.972583
    CENTER_LATITUDE = 1.34911
    EARTH_RADIUS_METERS = 6_371_000.0
    AEQD_CRS = (
        "+proj=aeqd +lat_0=1.34911 +lon_0=103.972583 +R=6371000 +units=m +no_defs"
    )
    provider_timezone = ZoneInfo("Asia/Singapore")
    _IMAGE_RE = re.compile(r'https?://[^"\'\s,]+/dpsri_240km_(\d{16})dBR\.dpsri\.png')

    @staticmethod
    def _headers() -> dict[str, str]:
        return {
            "User-Agent": "Mozilla/5.0 (compatible; radiust/1)",
            "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,image/*,*/*;q=0.8",
            "Accept-Language": "en-US,en;q=0.9",
        }

    async def discover_entries(self, context: SourceContext):
        payload = await self._get(context, self.PAGE_URL, headers=self._headers())
        text = payload.decode("utf-8", errors="replace")
        entries = []
        seen: set[str] = set()
        for match in self._IMAGE_RE.finditer(text):
            url = match.group(0)
            if url in seen:
                continue
            seen.add(url)
            try:
                valid_time = datetime.strptime(match.group(1)[:12], "%Y%m%d%H%M").replace(tzinfo=self.provider_timezone)
            except ValueError:
                continue
            entries.append(
                {
                    "url": url,
                    "station": "SGCOMP",
                    "valid_time": valid_time,
                    "headers": self._headers(),
                    "revision": match.group(1),
                    "metadata": {
                        "time_semantics": "provider_frame_time",
                        "geometry_status": "verified_aeqd_240km",
                        "value_semantics": "provider_rainfall_intensity_categories",
                    },
                }
            )
        return entries

    def decode(self, raw: RawFrame, context: SourceContext) -> RadarField:
        """Decode the provider's categorical rain overlay on its native AEQD grid."""

        raw._ensure_open()
        artifact = next((item for item in raw.artifacts if item.role == "data"), raw.artifacts[0])
        payload = artifact.payload if isinstance(artifact.payload, bytes) else Path(artifact.payload).read_bytes()
        try:
            with Image.open(io.BytesIO(payload)) as image:
                rgba = np.asarray(image.convert("RGBA"), dtype=np.uint8).copy()
        except Exception as exc:
            raise DecodeError(f"unable to decode Singapore radar image {artifact.name}: {exc}") from exc

        height, width = rgba.shape[:2]
        if height != width or width < 1:
            raise GeoreferencingError(
                f"Singapore 240 km radar image must be square; received {width}x{height}"
            )
        context.check_pixels(int(height * width) * 2)

        # RGB under alpha=0 differs between the legacy slideshow and the
        # corresponding official API PNG; it carries no visible information.
        rgba[rgba[:, :, 3] == 0] = (0, 0, 0, 0)
        values, quality = ExactPaletteDecoder(_PALETTE, strict=True).decode(rgba)
        values = np.asarray(values, dtype=np.float32)
        quality = np.asarray(quality, dtype=np.uint16)

        pixel_size = 2 * self.RANGE_METERS / width
        x = -self.RANGE_METERS + (np.arange(width, dtype=np.float64) + 0.5) * pixel_size
        y = self.RANGE_METERS - (np.arange(height, dtype=np.float64) + 0.5) * pixel_size
        mesh_x, mesh_y = np.meshgrid(x, y)
        outside = np.hypot(mesh_x, mesh_y) > self.RANGE_METERS
        values[outside] = np.nan
        quality[outside] = QUALITY_OUTSIDE

        product = self.info.default_product
        if product is None or not product.variables:
            raise DecodeError("Singapore radar catalog has no rainfall-intensity variable")
        variable = product.variables[0]
        meanings = "no_displayed_rain " + " ".join(_CATEGORY_MEANINGS)
        data = xr.DataArray(
            values,
            dims=("y", "x"),
            coords={"y": y, "x": x},
            name=variable,
            attrs={
                "units": "1",
                "kind": "categorical",
                "long_name": "MSS radar rainfall-intensity display category",
                "flag_values": [0, *[item["value"] for item in _PALETTE_DOCUMENT["categories"]]],
                "flag_meanings": meanings,
                "comment": (
                    "Ordinal display categories, not quantitative rainfall rates. Class 0 means no visible "
                    "colored rain return; the provider does not distinguish true no-rain from below-threshold "
                    "or unobserved pixels within range. Pixels outside the nominal 240 km radius are masked."
                ),
            },
        )
        quality_data = xr.DataArray(quality, dims=data.dims, coords=data.coords, name="quality")
        return RadarField(
            data,
            CartesianGrid(x, y, crs=self.AEQD_CRS),
            quality_data,
            {
                "source": self.info.id,
                "product": raw.ref.product,
                "station": raw.ref.station,
                "valid_time": format_time(raw.ref.valid_time),
                "value_semantics": "ordinal_provider_rainfall_intensity_category",
                "palette": _PALETTE_DOCUMENT["id"],
                "palette_version": _PALETTE_DOCUMENT["version"],
                "projection": "Azimuthal Equidistant",
                "projection_center": [self.CENTER_LONGITUDE, self.CENTER_LATITUDE],
                "projection_earth_radius_m": self.EARTH_RADIUS_METERS,
                "range_m": self.RANGE_METERS,
                "raw": artifact.name,
            },
        )


__all__ = ["SgSource"]
