"""Spain AEMET national radar adapter."""

from __future__ import annotations

import json

from ..context import SourceContext
from ..errors import DecodeError
from .legacy import LegacyImageSource, parse_provider_time


class EsSource(LegacyImageSource):
    TIMELINE_URL = "https://www.aemet.es/es/api-eltiempo/radar/timeline/compo/PB"
    IMAGE_BASE = "https://www.aemet.es/es/api-eltiempo/radar/imagen-radar/compo/"
    REFERER = "https://www.aemet.es/es/eltiempo/observacion/radar"

    async def discover_entries(self, context: SourceContext):
        payload = await self._get(context, self.TIMELINE_URL, headers={"Referer": self.REFERER})
        try:
            document = json.loads(payload)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DecodeError(f"AEMET returned invalid timeline JSON: {exc}") from exc
        if not isinstance(document, list) or not document or not isinstance(document[0], dict):
            return []
        entries = []
        for item in document[0].get("Elementos", []):
            if not isinstance(item, dict):
                continue
            filename, raw_time = item.get("Nombre fichero"), item.get("Fecha")
            if not isinstance(filename, str) or not isinstance(raw_time, str):
                continue
            try:
                valid_time = parse_provider_time(raw_time)
            except ValueError:
                continue
            entries.append(
                {
                    "url": self.IMAGE_BASE + filename,
                    "station": "ESCOMP",
                    "valid_time": valid_time,
                    "headers": {"Referer": self.REFERER},
                    "revision": filename.removesuffix(".png"),
                    "metadata": {
                        "time_semantics": "provider_frame_time",
                        "source_projection_claim": "EPSG:3857",
                        "geometry_status": "control_points_pending",
                    },
                }
            )
        return entries


__all__ = ["EsSource"]
