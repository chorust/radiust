"""Thailand TMD live GIF radar adapter."""

from __future__ import annotations

import asyncio
import hashlib
from typing import Any

from ..context import SourceContext
from ..errors import IntegrityError, UnsupportedQueryError
from ..models import FrameRef, Query
from ..raw import RawFrame
from ._th_time import read_tmd_footer_times
from .legacy import LegacyImageSource


class ThSource(LegacyImageSource):
    historical = False
    RADARS = {
        "cmp1": ("https://weather.tmd.go.th/cmp/cmp1.gif", "https://weather.tmd.go.th/cmpLoop.php"),
        "kkn240Loop": (
            "https://weather.tmd.go.th/kkn/kkn240Loop.gif",
            "https://weather.tmd.go.th/kknLoop.php",
        ),
    }

    async def discover(self, query: Query, context: SourceContext):
        if not self.historical and (query.at is not None or query.start is not None):
            raise UnsupportedQueryError(f"source {self.info.id} only supports latest frames")
        return await super().discover(query, context)

    async def discover_entries(self, context: SourceContext) -> list[dict[str, Any]]:
        return await self._discover_stations(context, tuple(self.RADARS))

    async def discover_entries_for_query(
        self, context: SourceContext, query: Query
    ) -> list[dict[str, Any]]:
        stations = query.stations or tuple(self.RADARS)
        return await self._discover_stations(context, stations)

    async def _discover_stations(
        self, context: SourceContext, stations: tuple[str, ...]
    ) -> list[dict[str, Any]]:
        prefetched = context.metadata.setdefault("th_prefetched_gifs", {})
        entries: list[dict[str, Any]] = []
        for station in stations:
            endpoint = self.RADARS.get(station)
            if endpoint is None:
                continue
            url, referer = endpoint
            headers = {"Referer": referer}
            payload = await super()._request(context, url, headers=headers)
            context.check_bytes(len(payload))
            times = await asyncio.to_thread(read_tmd_footer_times, payload, station=station)
            valid_time = times[-1]
            prefetched[url] = payload
            entries.append(
                {
                    "url": url,
                    "station": station,
                    "valid_time": valid_time,
                    "headers": headers,
                    "revision": hashlib.sha256(payload).hexdigest(),
                    "payload_sha256": hashlib.sha256(payload).hexdigest(),
                    "metadata": {
                        "time_semantics": "rendered_footer_ocr_utc",
                        "footer_observation_times": [
                            item.isoformat(timespec="seconds").replace("+00:00", "Z")
                            for item in times
                        ],
                        "geometry_status": "unverified",
                    },
                }
            )
        return entries

    async def _request(self, context: SourceContext, url: str, **kwargs: Any) -> bytes:
        context.cancellation.check()
        prefetched = context.metadata.get("th_prefetched_gifs", {})
        if kwargs.get("method", "GET").upper() == "GET" and url in prefetched:
            return prefetched.pop(url)
        return await super()._request(context, url, **kwargs)

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        raw = await super().download(ref, context)
        expected_hash = ref.locator.get("payload_sha256")
        if expected_hash is None:
            return raw
        data_artifact = next(
            (item for item in raw.artifacts if item.role == "data"), raw.artifacts[0]
        )
        if data_artifact.sha256 != expected_hash:
            await raw.aclose()
            raise IntegrityError(
                "TMD GIF changed since timestamp discovery; rediscover before acquiring it"
            )
        return raw


__all__ = ["ThSource"]
