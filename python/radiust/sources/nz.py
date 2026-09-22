"""New Zealand MetService rural radar acquisition."""

from __future__ import annotations

import json

from ..context import SourceContext
from .legacy import LegacyImageSource, parse_provider_time


class NzSource(LegacyImageSource):
    BASE_URL = "https://mobile-apps.metservice.com"
    STATIONS = {
        "NZAU2": "Kumeu",
        "NZBA2": "Rotorua",
        "NZCA2": "Darfield",
        "NZNO2": "Whangarei",
        "NZSO2": "Gore",
        "NZMA2": "Gisborne",
        "NZOT2": "Oamaru",
        "NZTA2": "New-Plymouth",
        "NZWE2": "Ohariu-Valley",
        "NZWS2": "Hokitika",
    }

    @staticmethod
    def _headers() -> dict[str, str]:
        return {
            "Accept": "*/*",
            "User-Agent": "rural/52 CFNetwork/3860.300.31 Darwin/25.2.0",
            "Accept-Language": "en-US,en;q=0.9",
        }

    async def discover_entries(self, context: SourceContext):
        entries = []
        for station, rural_id in self.STATIONS.items():
            url = f"{self.BASE_URL}/publicData/mobileRainRadar_rural_{rural_id}"
            payload = await self._get(context, url, headers=self._headers())
            try:
                document = json.loads(payload)
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            images = document.get("imageList", []) if isinstance(document, dict) else []
            for item in images:
                if not isinstance(item, dict):
                    continue
                rel_url, raw_time = item.get("url"), item.get("dateTimeISO")
                if not isinstance(rel_url, str) or not isinstance(raw_time, str):
                    continue
                try:
                    valid_time = parse_provider_time(raw_time)
                except ValueError:
                    continue
                entries.append(
                    {
                        "url": self.BASE_URL + rel_url,
                        "station": station,
                        "valid_time": valid_time,
                        "headers": self._headers(),
                        "revision": f"{station}-{int(valid_time.timestamp())}",
                        "metadata": {"time_semantics": "provider_frame_time", "geometry_status": "unverified"},
                    }
                )
        return entries


__all__ = ["NzSource"]
