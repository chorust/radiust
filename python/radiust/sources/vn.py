"""Vietnam Hymetnet CMAX radar acquisition."""

from __future__ import annotations

import re
from datetime import datetime, timedelta, timezone

from ..context import SourceContext
from .legacy import LegacyImageSource


class VnSource(LegacyImageSource):
    BASE_URL = "http://hymetnet.gov.vn"
    STATIONS = ("PLI", "VTR", "PHA", "VIN", "DHA", "TKY", "QNH", "PLE", "NHT", "NHB", "HUE")

    async def discover_entries(self, context: SourceContext):
        entries = []
        for station in self.STATIONS:
            page = await self._get(context, f"{self.BASE_URL}/radar/{station}")
            text = page.decode("utf-8", errors="replace")
            for timestamp in re.findall(r'tentimesett\[.*?\]\s*=\s*"([0-9]{12})";', text):
                try:
                    valid_time = datetime.strptime(timestamp, "%Y%m%d%H%M").replace(tzinfo=timezone.utc)
                except ValueError:
                    continue
                date = timestamp[:8]
                entries.append(
                    {
                        "url": f"{self.BASE_URL}/dataout_web/{station}/{date}/{station}_{timestamp}_CMAX00.png",
                        "station": station,
                        "valid_time": valid_time,
                        "revision": f"{station}-{timestamp}",
                        "metadata": {"time_semantics": "provider_frame_time", "geometry_status": "unverified"},
                    }
                )
        # Hymetnet occasionally leaves retired/stale station timestamps in the
        # inline slideshow JavaScript even after the corresponding image has
        # disappeared (HUE exposed a 2023 frame during the 2026-09-18 live
        # migration check).  This endpoint is a rolling-current surface, not a
        # historical archive, so discard station frames more than one day
        # behind the freshest frame advertised by the provider.
        if entries:
            freshest = max(item["valid_time"] for item in entries)
            cutoff = freshest - timedelta(days=1)
            entries = [item for item in entries if item["valid_time"] >= cutoff]
        return entries


__all__ = ["VnSource"]
