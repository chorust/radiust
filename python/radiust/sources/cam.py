"""Cambodia meteorological slideshow radar acquisition."""

from __future__ import annotations

import re
from datetime import datetime, timezone
from urllib.parse import urljoin

from ..context import SourceContext
from .legacy import LegacyImageSource, extract_urls


class CamSource(LegacyImageSource):
    PAGE_URL = "http://www.cambodiameteo.com/slideshow?menu=117&lang=en&domain=CAMBODIA"
    BASE_URL = "http://www.cambodiameteo.com"
    PAGE_HEADERS = {
        "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        "Accept-Language": "zh,en;q=0.9",
        "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
    }

    async def discover_entries(self, context: SourceContext):
        payload = await self._get(context, self.PAGE_URL, headers=self.PAGE_HEADERS)
        text = payload.decode("utf-8", errors="replace")
        urls = extract_urls(text)
        urls.extend(re.findall(r'["\'](/[^"\']+\.(?:png|gif|jpg|jpeg))["\']', text, re.IGNORECASE))
        entries = []
        for path in dict.fromkeys(urls):
            match = re.search(r"(\d{14})_cambodia_", path, re.IGNORECASE)
            if not match:
                continue
            try:
                valid_time = datetime.strptime(match.group(1), "%Y%m%d%H%M%S").replace(tzinfo=timezone.utc)
            except ValueError:
                continue
            filename = path.split("?", 1)[0].rsplit("/", 1)[-1]
            parts = filename.split("_")
            station = parts[1] if len(parts) > 2 else "CAMCOMP"
            entries.append(
                {
                    "url": urljoin(self.BASE_URL, path),
                    "station": station,
                    "valid_time": valid_time,
                    "revision": filename.rsplit(".", 1)[0],
                    "metadata": {"time_semantics": "filename_utc", "geometry_status": "unverified"},
                }
            )
        return entries


__all__ = ["CamSource"]
