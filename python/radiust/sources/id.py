"""Legacy BMKG SIDARMA image endpoint with explicit credential configuration."""

from __future__ import annotations

import json
from importlib import resources
from urllib.parse import urlencode

from ..context import SourceContext
from ..errors import ConfigError, DecodeError
from .legacy import LegacyImageSource, parse_provider_time


class IdSource(LegacyImageSource):
    API_URL = "https://radar.bmkg.go.id:8090/sidarmaimage"

    def __init__(self, info):
        super().__init__(info)
        path = resources.files("radiust.resources").joinpath("sources/id.json")
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise DecodeError(f"invalid BMKG radar resource: {exc}") from exc
        radar_ids = document.get("radar_ids") if isinstance(document, dict) else None
        if not isinstance(radar_ids, list) or not radar_ids:
            raise DecodeError("BMKG ID resource has no radar IDs")
        self._radars = tuple(str(radar_id).strip().upper() for radar_id in radar_ids)
        if any(not radar_id for radar_id in self._radars) or len(set(self._radars)) != len(self._radars):
            raise DecodeError("BMKG ID resource contains empty or duplicate radar IDs")

    def _config(self, context: SourceContext) -> dict:
        values = context.config.values.get("sources", {})
        configured = values.get("id", {}) if isinstance(values, dict) else {}
        return configured if isinstance(configured, dict) else {}

    async def discover_entries(self, context: SourceContext):
        config = self._config(context)
        token = config.get("token")
        if not token:
            raise ConfigError("source id requires sources.id.token; legacy embedded credentials are not migrated")
        radar_ids = config.get("radar_ids", self._radars)
        if not isinstance(radar_ids, (list, tuple)):
            raise DecodeError("sources.id.radar_ids must be a list")
        entries = []
        headers = {
            "Referer": "https://kalteng.bmkg.go.id/",
            "User-Agent": "Mozilla/5.0 (compatible; radiust/1)",
        }
        for radar_id in radar_ids:
            radar_id = str(radar_id).strip().upper()
            if radar_id not in self._radars:
                raise DecodeError(f"unknown BMKG radar id: {radar_id}")
            query = urlencode({"token": str(token), "radar": radar_id})
            payload = await self._get(context, f"{self.API_URL}?{query}", headers=headers)
            try:
                document = json.loads(payload)
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                raise DecodeError(f"BMKG returned invalid discovery JSON for {radar_id}") from exc
            bucket = document.get("LastOneHour") if isinstance(document, dict) else None
            if not isinstance(bucket, dict):
                continue
            files = bucket.get("file", [])
            times = bucket.get("timeUTC", [])
            if not isinstance(files, list) or not isinstance(times, list):
                continue
            for url, raw_time in zip(files, times, strict=False):
                if not isinstance(url, str) or not isinstance(raw_time, str):
                    continue
                try:
                    valid_time = parse_provider_time(raw_time.replace(" UTC", "+00:00"))
                except ValueError:
                    continue
                entries.append(
                    {
                        "url": url,
                        "station": radar_id,
                        "valid_time": valid_time,
                        "revision": f"{radar_id}-{int(valid_time.timestamp())}",
                        "metadata": {
                            "time_semantics": "provider_frame_time",
                            "geometry_status": "unverified",
                        },
                    }
                )
        return entries


__all__ = ["IdSource"]
