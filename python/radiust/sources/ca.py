"""Canada ECCC CAPPI rainfall radar directory adapter."""

from __future__ import annotations

import re
from datetime import datetime, timezone
from urllib.parse import urljoin

from ..context import SourceContext
from ..models import Query
from .legacy import LegacyImageSource


class CaSource(LegacyImageSource):
    BASE_URL = "https://dd.meteo.gc.ca/{date}/WXO-DD/radar/CAPPI/GIF/"
    _HREF = re.compile(r'href=["\']([^"\']+)["\']', re.IGNORECASE)

    async def _links(self, context: SourceContext, url: str) -> list[str]:
        payload = await self._get(context, url, headers={"Accept": "text/html,*/*;q=0.8"})
        return self._HREF.findall(payload.decode("utf-8", errors="replace"))

    async def discover_entries(self, context: SourceContext):
        return await self._discover_entries(context, requested_stations=())

    async def discover_entries_for_query(self, context: SourceContext, query: Query):
        return await self._discover_entries(context, requested_stations=query.stations)

    async def _discover_entries(self, context: SourceContext, *, requested_stations: tuple[str, ...]):
        day = datetime.now(timezone.utc).strftime("%Y%m%d")
        root = self.BASE_URL.format(date=day)
        entries = []
        station_dirs = sorted(
            {
                href.strip("/")
                for href in await self._links(context, root)
                if href.endswith("/") and href.strip("/").startswith("CA") and "/" not in href.strip("/")
            }
        )
        if requested_stations:
            requested = set(requested_stations)
            station_dirs = [station for station in station_dirs if station in requested]
        for station in station_dirs:
            station_url = urljoin(root, station + "/")
            links = await self._links(context, station_url)
            subdirs = [
                href.strip("/")
                for href in links
                if href.endswith("/") and href.strip("/") not in {"", ".", ".."} and "/" not in href.strip("/")
            ]
            targets = [(station_url, links)]
            if subdirs:
                targets = []
                for subdir in subdirs:
                    url = urljoin(station_url, subdir + "/")
                    targets.append((url, await self._links(context, url)))
            for base, file_links in targets:
                for href in file_links:
                    name = href.split("?", 1)[0].split("#", 1)[0].rstrip("/").rsplit("/", 1)[-1]
                    if not name.upper().endswith("RAIN.GIF"):
                        continue
                    match = re.match(r"(\d{8,14})_", name)
                    if not match:
                        continue
                    digits = match.group(1)
                    fmt = {
                        8: "%Y%m%d",
                        10: "%Y%m%d%H",
                        12: "%Y%m%d%H%M",
                        14: "%Y%m%d%H%M%S",
                    }.get(len(digits))
                    if fmt is None:
                        continue
                    try:
                        valid_time = datetime.strptime(digits, fmt).replace(tzinfo=timezone.utc)
                    except ValueError:
                        continue
                    entries.append(
                        {
                            "url": urljoin(base, href),
                            "station": station,
                            "valid_time": valid_time,
                            "revision": name,
                            "metadata": {"time_semantics": "filename_utc", "geometry_status": "unverified"},
                        }
                    )
        return entries


__all__ = ["CaSource"]
