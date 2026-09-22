"""Taiwan CWA HTTP radar observation adapter."""

from __future__ import annotations

import re
from datetime import datetime
from zoneinfo import ZoneInfo

from ..context import SourceContext
from .legacy import LegacyImageSource


class TwHttpSource(LegacyImageSource):
    JS_URL = "https://www.cwa.gov.tw/Data/js/obs_img/Observe_radar.js"
    IMAGE_BASE = "https://www.cwa.gov.tw/Data/radar/"
    REFERER = "https://www.cwa.gov.tw/V8/E/W/OBS_Radar.html"
    provider_timezone = ZoneInfo("Asia/Taipei")

    async def discover_entries(self, context: SourceContext):
        payload = await self._get(context, self.JS_URL, headers={"Accept": "*/*", "Referer": self.REFERER})
        text = payload.decode("utf-8", errors="replace")
        matches = re.findall(r'\{"img":\s*\'(.*?)\'\s*,\s*\'text\':\s*\'(.*?)\'\}', text)
        entries = []
        for filename, label in matches:
            if not filename.startswith("CV1_3600_") or not filename.endswith(".png"):
                continue
            try:
                valid_time = datetime.strptime(label, "%Y/%m/%d %H:%M").replace(tzinfo=self.provider_timezone)
            except ValueError:
                continue
            entries.append(
                {
                    "url": self.IMAGE_BASE + filename,
                    "station": "CV1_3600",
                    "valid_time": valid_time,
                    "headers": {"Referer": self.REFERER},
                    "revision": filename.removesuffix(".png"),
                    "metadata": {
                        "time_semantics": "provider_frame_time",
                        "geometry_status": "unverified",
                    },
                }
            )
        return entries


__all__ = ["TwHttpSource"]
