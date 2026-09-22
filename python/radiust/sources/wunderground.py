"""Weather Underground radar mosaic tile acquisition."""

from __future__ import annotations

from datetime import datetime
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

from ..context import SourceContext
from ..errors import AuthenticationError, ConfigError, ResourceLimitError, TransportError
from ._legacy_tiles import FourTileSource


class WundergroundSource(FourTileSource):
    cadence_seconds = 300

    async def discover_entries(self, context: SourceContext):
        # Fail closed at discovery while keeping the credential out of the
        # URLs retained by the resulting FrameRef.
        self._api_key(context)
        return await super().discover_entries(context)

    def tile_url(self, valid_time: datetime, x: int, y: int, context: SourceContext) -> str:
        params = urlencode(
            {
                "product": "wuRadarMosaic",
                "ts": int(valid_time.timestamp()),
                "xyz": f"{x}:{y}:1",
            }
        )
        return f"https://api0.weather.com/v3/TileServer/tile?{params}"

    def _api_key(self, context: SourceContext) -> str:
        values = context.config.values.get("sources", {})
        configured = values.get("wunderground", {}) if isinstance(values, dict) else {}
        api_key = configured.get("api_key") if isinstance(configured, dict) else None
        if not api_key:
            raise ConfigError(
                "source wunderground requires sources.wunderground.api_key; legacy embedded credentials are not migrated"
            )
        return str(api_key)

    async def _get(self, context: SourceContext, url: str, *, headers=None) -> bytes:
        """Add the credential only at the transport boundary, never to FrameRef."""
        parsed = urlsplit(url)
        query = [(key, value) for key, value in parse_qsl(parsed.query, keep_blank_values=True) if key != "apiKey"]
        query.append(("apiKey", self._api_key(context)))
        request_url = urlunsplit((parsed.scheme, parsed.netloc, parsed.path, urlencode(query), parsed.fragment))
        try:
            return await super()._get(context, request_url, headers=headers)
        except AuthenticationError:
            raise AuthenticationError("Weather Underground rejected the configured credentials") from None
        except ResourceLimitError:
            raise ResourceLimitError("Weather Underground tile request exceeded configured resource limits") from None
        except Exception:
            # Custom transports and HTTP libraries may include the full
            # request URL, including apiKey, in their exception text.
            raise TransportError("Weather Underground tile request failed") from None


__all__ = ["WundergroundSource"]
