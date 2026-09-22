"""Météo-France radar WMS adapter."""

from __future__ import annotations

import codecs
from datetime import datetime, timedelta, timezone
from http.cookies import SimpleCookie
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

from ..context import SourceContext
from ..errors import ConfigError
from .legacy import LegacyImageSource


def _rot13(value: str) -> str:
    return codecs.decode(value, "rot_13")


class FrSource(LegacyImageSource):
    PAGE_URL = "https://meteofrance.com/images-radar"
    WMS_BASE_URL = "https://rwg.meteofrance.com/geoservices/Radar-mapcache-WMS"
    WMS_LAYER = "BASE_REFLECTIVITY"
    WMS_STYLE = "synopsis_reflectivity_oppidum_transparence"
    default_bbox = (-5.5, 41.0, 10.0, 51.5)
    historical = False

    @classmethod
    def _headers(cls) -> dict[str, str]:
        return {
            "Accept": "*/*",
            "Referer": cls.PAGE_URL,
            "User-Agent": "Mozilla/5.0 (compatible; radiust/1)",
        }

    async def _get(self, context: SourceContext, url: str, *, headers=None) -> bytes:
        """Attach the ephemeral session token only to the WMS transport request."""
        if url.startswith(self.WMS_BASE_URL + "?"):
            parsed = urlsplit(url)
            query = [(key, value) for key, value in parse_qsl(parsed.query, keep_blank_values=True) if key != "token"]
            query.append(("token", await self._token(context)))
            url = urlunsplit((parsed.scheme, parsed.netloc, parsed.path, urlencode(query), parsed.fragment))
        return await super()._get(context, url, headers=headers)

    async def _token(self, context: SourceContext) -> str:
        cached = context.metadata.get("fr_wms_session_token")
        if isinstance(cached, str) and cached:
            return cached
        config = context.config.values.get("sources", {}).get(self.info.id, {})
        configured = str(config.get("token", "")) if isinstance(config, dict) else ""
        if configured:
            return configured
        get_response = getattr(context.transport, "get_response", None)
        if get_response is None:
            raise ConfigError(
                "source fr requires sources.fr.token when the configured transport cannot expose response headers"
            )
        response = await get_response(self.PAGE_URL, headers=self._headers())
        headers = {str(key).lower(): str(value) for key, value in dict(response.headers).items()}
        cookie_header = headers.get("set-cookie", "")
        cookie = SimpleCookie()
        cookie.load(cookie_header)
        morsel = cookie.get("mfsession")
        if morsel is None or not morsel.value:
            raise ConfigError("Météo-France page did not provide an mfsession cookie")
        token = _rot13(morsel.value)
        context.metadata["fr_wms_session_token"] = token
        return token

    @staticmethod
    def _current_frame_time(now: datetime | None = None) -> datetime:
        current = (now or datetime.now(timezone.utc)).astimezone(timezone.utc) - timedelta(minutes=5)
        minute = current.minute - current.minute % 15
        return current.replace(minute=minute, second=0, microsecond=0)

    async def discover_entries(self, context: SourceContext):
        # The WMS exposes no timeline listing. Build an unsigned current-time
        # reference here; fetch the ephemeral session token only if the caller
        # actually acquires the WMS image.
        frame_time = self._current_frame_time()
        bbox = self.default_bbox
        # EPSG:3857 bounds for the WMS request.  Keep the conversion local so
        # the decoder can continue to expose the declared geographic extent.
        import math

        origin_shift = 20037508.342789244
        west, south, east, north = bbox
        def mercator(lon: float, lat: float) -> tuple[float, float]:
            x = lon * origin_shift / 180.0
            clipped = max(min(lat, 89.9), -89.9)
            y = math.log(math.tan((90.0 + clipped) * math.pi / 360.0)) * origin_shift / math.pi
            return x, y

        x0, y0 = mercator(west, south)
        x1, y1 = mercator(east, north)
        # Store the exact requested pixel grid, distinct from the provider's
        # underlying radar-native grid (which this WMS portrayal does not expose).
        request_bounds = tuple(float(f"{value:.6f}") for value in (x0, y0, x1, y1))
        width, height = 700, 600
        params = {
            "service": "WMS", "request": "GetMap", "version": "1.3.0",
            "layers": self.WMS_LAYER, "styles": self.WMS_STYLE, "format": "image/png",
            "transparent": "true", "crs": "EPSG:3857",
            "bbox": ",".join(f"{value:.6f}" for value in request_bounds),
            "width": str(width), "height": str(height),
            "time": frame_time.strftime("%Y-%m-%dT%H:%M:%SZ"),
        }
        return [{
            "url": f"{self.WMS_BASE_URL}?{urlencode(params)}",
            # The WMS endpoint has no file suffix. Explicitly name the
            # requested image so shared acquisition validates its PNG bytes
            # before publishing a RawFrame (HTTP 200 may contain WMS XML).
            "name": f"FRCOMP_{frame_time:%Y%m%dT%H%M%SZ}.png",
            "station": "FRCOMP",
            "valid_time": frame_time,
            "bbox": bbox,
            "headers": self._headers(),
            "revision": frame_time.strftime("%Y%m%dT%H%M%SZ"),
            "metadata": {
                "time_semantics": "wms_time_dimension",
                "wms_crs": "EPSG:3857",
                "wms_request_bbox_m": list(request_bounds),
                "wms_request_size": [width, height],
                "wms_request_center_m": [
                    request_bounds[0] + 350.5 * (request_bounds[2] - request_bounds[0]) / width,
                    request_bounds[3] - 300.5 * (request_bounds[3] - request_bounds[1]) / height,
                ],
                "wms_request_grid_only": True,
                "layer": self.WMS_LAYER,
                "period_minutes": 15,
            },
        }]


__all__ = ["FrSource"]
