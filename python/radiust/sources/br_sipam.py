"""Brazil SIPAM radar acquisition adapter.

SIPAM publishes a JSON radar index and timestamped PNG products.  The adapter
keeps the provider image as raw data and carries the station bbox into the
frame locator.  The old repository's luminance conversion is deliberately not
used as a physical dBZ decoder.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from urllib.parse import urljoin

from ..context import SourceContext
from ..errors import DecodeError
from .legacy import LegacyImageSource, parse_provider_time


class BrSipamSource(LegacyImageSource):
    RADAR_LIST_URL = "https://apihidro.sipam.gov.br/radares/"
    IMAGE_BASE_URL = "https://siger.sipam.gov.br/radar/"
    DEFAULT_PRODUCT = "dbz"
    provider_timezone = timezone.utc

    @staticmethod
    def _station_id(code: str) -> str:
        normalized = code.strip().lower()
        if normalized.startswith("sb") and len(normalized) == 4:
            return f"BR{normalized[2:].upper()}"
        return normalized.upper()

    @staticmethod
    def _bbox(entry: dict[str, object]) -> tuple[float, float, float, float] | None:
        values = (
            entry.get("longitudeMin"),
            entry.get("latitudeMin"),
            entry.get("longitudeMax"),
            entry.get("latitudeMax"),
        )
        if not all(isinstance(value, (int, float)) for value in values):
            return None
        return tuple(float(value) for value in values)  # type: ignore[return-value]

    @staticmethod
    def _url_stamp(value: datetime) -> str:
        return value.astimezone(timezone.utc).strftime("%Y_%m_%d_%H_%M_%S")

    async def discover_entries(self, context: SourceContext):
        payload = await self._get(context, self.RADAR_LIST_URL, headers={"Accept": "application/json"})
        try:
            document = json.loads(payload)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DecodeError(f"SIPAM radar index is not valid JSON: {exc}") from exc
        if not isinstance(document, list):
            raise DecodeError("SIPAM radar index must be an array")

        entries: list[dict[str, object]] = []
        for item in document:
            if not isinstance(item, dict):
                continue
            code = item.get("nomeRadar")
            if not isinstance(code, str) or not code.strip():
                continue
            products = item.get("produtos", [])
            if not isinstance(products, list) or self.DEFAULT_PRODUCT not in products:
                continue
            scans = item.get("varreduras", [])
            if not isinstance(scans, list):
                continue
            bbox = self._bbox(item)
            station = self._station_id(code)
            for scan in scans:
                if not isinstance(scan, str) or not scan.strip():
                    continue
                try:
                    valid_time = parse_provider_time(scan, timezone_name=self.provider_timezone)
                except (TypeError, ValueError, OverflowError):
                    continue
                stamp = self._url_stamp(valid_time)
                entries.append(
                    {
                        "url": urljoin(self.IMAGE_BASE_URL, f"{code.strip().lower()}/{self.DEFAULT_PRODUCT}/{stamp}.png"),
                        "station": station,
                        "valid_time": valid_time,
                        "revision": f"{station}-{stamp}",
                        "bbox": bbox,
                        "metadata": {
                            "time_semantics": "varreduras_iso_utc",
                            "provider_radar": code.strip().lower(),
                            "provider_product": self.DEFAULT_PRODUCT,
                            "municipality": str(item.get("nomeMunicipio") or ""),
                            "geometry_status": "declared_bbox_pending_pixel_control_points",
                            "bbox_order": "west,south,east,north",
                        },
                    }
                )
        return entries


__all__ = ["BrSipamSource"]
