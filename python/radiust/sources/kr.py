"""Korea Meteorological Administration station-radar acquisition."""

from __future__ import annotations

import json
from datetime import datetime, timedelta
from urllib.parse import urlencode
from zoneinfo import ZoneInfo

from ..context import SourceContext
from ..errors import DecodeError
from .legacy import LegacyImageSource


class KrSource(LegacyImageSource):
    BASE_URL = "https://radar.kma.go.kr"
    DISCOVERY_URL = BASE_URL + "/radar/fileChkAjax.do"
    IMAGE_URL = BASE_URL + "/cgi-bin/center/nph-rdr_stn1_img"
    REFERER = BASE_URL + "/eng/radar/individual.do"
    STATIONS = ("KWK", "BRI", "GDK", "GNG", "KSN", "JNI", "MYN", "PSN", "GSN", "SSP")
    provider_timezone = ZoneInfo("Asia/Seoul")

    @classmethod
    def _headers(cls) -> dict[str, str]:
        return {
            "User-Agent": "Mozilla/5.0 (compatible; radiust/1)",
            "Accept": "application/json, text/javascript, */*; q=0.01",
            "X-Requested-With": "XMLHttpRequest",
            "Referer": cls.REFERER,
        }

    async def discover_entries(self, context: SourceContext):
        now = datetime.now(self.provider_timezone)
        entries = []
        for offset in (3, 8, 13, 18, 23):
            tm = (now - timedelta(minutes=offset)).strftime("%Y%m%d%H%M")
            for station in self.STATIONS:
                payload = await self._request(
                    context,
                    self.DISCOVERY_URL,
                    method="POST",
                    body={"tm": tm, "cgiId": "STN", "siteCd": station, "prId": "gif"},
                    headers=self._headers(),
                )
                try:
                    document = json.loads(payload)
                except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                    if not payload.strip():
                        continue
                    raise DecodeError("KMA returned invalid station discovery JSON") from exc
                if not isinstance(document, list) or not document or not isinstance(document[0], dict):
                    continue
                item = document[0]
                rec_date = item.get("recDate")
                if item.get("result") != 1 or not isinstance(rec_date, str):
                    continue
                try:
                    valid_time = datetime.strptime(rec_date, "%Y%m%d%H%M").replace(tzinfo=self.provider_timezone)
                except ValueError:
                    continue
                params = {
                    "rdr": "HSR",
                    "vol": "RN",
                    "cpi": "CPP",
                    "cdf": "1",
                    "sms": "5",
                    "swpn": "0",
                    "ht": "1.5i",
                    "aws": "0",
                    "map": "",
                    "color": "",
                    "area": "1",
                    "ang": "",
                    "size": "720",
                    "stn": station,
                    "tm": rec_date,
                    "zoom_level": "0",
                    "zoom_x": "0000000",
                    "zoom_y": "0000000",
                    "xp": "undefined",
                    "yp": "undefined",
                    "zoom": "1",
                }
                entries.append(
                    {
                        "url": self.IMAGE_URL + "?" + urlencode(params),
                        "station": station,
                        "valid_time": valid_time,
                        "headers": {"User-Agent": self._headers()["User-Agent"], "Referer": self.REFERER},
                        "revision": f"{station}-{rec_date}",
                        "metadata": {"time_semantics": "provider_frame_time", "geometry_status": "unverified"},
                    }
                )
        return entries


__all__ = ["KrSource"]
