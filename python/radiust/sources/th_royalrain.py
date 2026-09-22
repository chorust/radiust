"""Thailand Royal Rainmaking Department CAPPI acquisition."""

from __future__ import annotations

import re
from datetime import datetime, timezone
from urllib.parse import urljoin

from ..context import SourceContext
from ..models import Query
from .legacy import LegacyImageSource

STATIONS = (
    "omkoi", "rongkwang", "takhli", "rasisalai", "singha", "phimai",
    "banphue", "sattahip", "pathio", "phanom", "pluakdaeng",
)


def _timestamp(filename: str) -> datetime | None:
    patterns = (
        (r"(\d{12})0[24]00dBZ\.cappi\.png", "%Y%m%d%H%M"),
        (r"\d+THA-(\d{8})-(\d{4})_", "%Y%m%d%H%M"),
        (r"[A-Za-z0-9]+-(\d{8})-(\d{4})_", "%Y%m%d%H%M"),
    )
    for pattern, fmt in patterns:
        match = re.match(pattern, filename)
        if not match:
            continue
        try:
            return datetime.strptime("".join(match.groups()), fmt).replace(tzinfo=timezone.utc)
        except ValueError:
            continue
    return None


class ThRoyalRainSource(LegacyImageSource):
    BASE_URL = "https://file.royalrain.go.th/opendata/radar_data/cappi"
    _IMG = re.compile(r'<img[^>]+src=["\']([^"\']*cappi[^"\']*\.png)["\']', re.IGNORECASE)

    async def discover_entries(self, context: SourceContext):
        return await self._discover_entries(context, stations=STATIONS)

    async def discover_entries_for_query(self, context: SourceContext, query: Query):
        stations = tuple(station for station in STATIONS if not query.stations or station in query.stations)
        return await self._discover_entries(context, stations=stations)

    async def _discover_entries(self, context: SourceContext, *, stations: tuple[str, ...]):
        entries = []
        for station in stations:
            page_url = f"{self.BASE_URL}/?station={station}"
            payload = await self._get(
                context,
                page_url,
                headers={"User-Agent": "Mozilla/5.0 (compatible; radiust/1)"},
            )
            for src in dict.fromkeys(self._IMG.findall(payload.decode("utf-8", errors="replace"))):
                filename = src.split("?", 1)[0].rsplit("/", 1)[-1]
                valid_time = _timestamp(filename)
                if valid_time is None:
                    continue
                entries.append(
                    {
                        "url": urljoin(page_url, src),
                        "station": station,
                        "valid_time": valid_time,
                        "headers": {"Referer": page_url},
                        "revision": filename.removesuffix(".png"),
                        "metadata": {"time_semantics": "filename_utc", "geometry_status": "unverified"},
                    }
                )
        return entries


__all__ = ["ThRoyalRainSource"]
