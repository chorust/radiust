"""Indonesia BMKG SIDARMA CMAX adapter."""

from __future__ import annotations

import asyncio
import json
from importlib import resources
from pathlib import Path
from typing import Any

from ..context import SourceContext
from ..errors import ConfigError, DecodeError, UnsupportedQueryError
from ..models import SourceInfo
from .legacy import LegacyImageSource, parse_provider_time


class IdSidarmaSource(LegacyImageSource):
    """Discover CMAX image URLs from the SIDARMA mobile API."""

    # The API advertises a rolling recent list only.  Do not expose an
    # apparently historical ProductInfo flag while discover() rejects `at`
    # and range queries before contacting the provider.
    historical = False

    API_URL_TEMPLATE = "https://api.bmkg.go.id/sidarma/sidarma-nowcast/android/ssx{radar_id}.json"
    provider_timezone = __import__("datetime").timezone.utc
    default_bbox = (94.0, -11.0, 141.0, 6.5)

    def __init__(self, info: SourceInfo) -> None:
        super().__init__(info)
        path = resources.files("radiust.resources").joinpath("sources/id_sidarma.json")
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise DecodeError(f"invalid SIDARMA resource: {exc}") from exc
        radars = value.get("radars") if isinstance(value, dict) else None
        if not isinstance(radars, list) or not radars:
            raise DecodeError("SIDARMA resource has no radar list")
        self._radars = {
            str(item["id"]).upper(): item
            for item in radars
            if isinstance(item, dict) and isinstance(item.get("id"), str)
        }

    def _radar_ids(self, context: SourceContext) -> tuple[str, ...]:
        values = context.config.values.get("sources", {}).get(self.info.id, {})
        configured = values.get("radar_ids") if isinstance(values, dict) else None
        if configured is None:
            return tuple(self._radars)
        if not isinstance(configured, (list, tuple)):
            raise DecodeError("sources.id_sidarma.radar_ids must be a list")
        radar_ids = tuple(str(value).strip().upper() for value in configured if str(value).strip())
        unknown = sorted(set(radar_ids) - set(self._radars))
        if unknown:
            raise DecodeError(f"unknown SIDARMA radar ids: {', '.join(unknown)}")
        return radar_ids

    def _bbox_for(self, radar_id: str) -> tuple[float, float, float, float]:
        bbox = self._radars[radar_id].get("bbox")
        if not isinstance(bbox, list) or len(bbox) != 4:
            raise DecodeError(f"SIDARMA radar {radar_id} has no verified bbox")
        return tuple(float(value) for value in bbox)  # type: ignore[return-value]

    async def _fetch_radar(self, context: SourceContext, radar_id: str) -> list[dict[str, Any]]:
        headers = {
            "Accept": "application/json, text/plain, */*",
            "User-Agent": "SidarmaMobile/2",
        }
        config = context.config.values.get("sources", {}).get(self.info.id, {})
        api_key = config.get("api_key") if isinstance(config, dict) else None
        if not api_key:
            raise ConfigError(
                "source id_sidarma requires sources.id_sidarma.api_key; legacy embedded credentials are not migrated"
            )
        headers["x-api-key"] = str(api_key)
        payload = await self._get(context, self.API_URL_TEMPLATE.format(radar_id=radar_id), headers=headers)
        try:
            document = json.loads(payload)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DecodeError(f"SIDARMA returned invalid JSON for {radar_id}: {exc}") from exc
        cmax = document.get("CMAX") if isinstance(document, dict) else None
        if not isinstance(cmax, dict):
            return []
        entries: list[dict[str, Any]] = []
        seen: dict[tuple[str, str], int] = {}
        for bucket_name in ("LastOneHour", "Latest"):
            bucket = cmax.get(bucket_name)
            if not isinstance(bucket, dict):
                continue
            files = bucket.get("file")
            times = bucket.get("timeUTC")
            file_list = files if isinstance(files, list) else [files]
            time_list = times if isinstance(times, list) else [times]
            for url, raw_time in zip(file_list, time_list, strict=False):
                if not isinstance(url, str) or not url.startswith(("http://", "https://")):
                    continue
                if not isinstance(raw_time, str) or not raw_time.strip() or raw_time.strip().lower() == "no data":
                    continue
                try:
                    normalized_time = raw_time.replace(" [UTC]", "+00:00").replace(" UTC", "+00:00")
                    valid_time = parse_provider_time(normalized_time)
                except ValueError:
                    try:
                        valid_time = parse_provider_time(raw_time, timezone_name=self.provider_timezone)
                    except ValueError:
                        continue
                key = (url, valid_time.isoformat())
                if key in seen:
                    if bucket_name == "Latest":
                        existing = entries[seen[key]]
                        metadata = dict(existing.get("metadata", {}))
                        metadata["bucket"] = "Latest"
                        metadata["buckets"] = ["LastOneHour", "Latest"]
                        existing["metadata"] = metadata
                    continue
                seen[key] = len(entries)
                entries.append(
                    {
                        "url": url,
                        "station": radar_id,
                        "valid_time": valid_time,
                        "bbox": self._bbox_for(radar_id),
                        "revision": Path(url.split("?", 1)[0]).name or None,
                        "metadata": {
                            "bucket": bucket_name,
                            "buckets": [bucket_name],
                            "time_semantics": "provider_frame_time",
                        },
                    }
                )
        return entries

    async def discover_entries(self, context: SourceContext) -> list[dict[str, Any]]:
        context.cancellation.check()
        responses = await asyncio.gather(
            *(self._fetch_radar(context, radar_id) for radar_id in self._radar_ids(context)),
            return_exceptions=True,
        )
        context.cancellation.check()
        entries: list[dict[str, Any]] = []
        failures: list[Exception] = []
        for response in responses:
            if isinstance(response, asyncio.CancelledError):
                raise response
            if isinstance(response, Exception):
                failures.append(response)
                continue
            entries.extend(response)
        if failures:
            context.metadata["discovery_failed_stations"] = len(failures)
            if len(failures) == len(responses):
                # Preserve a typed upstream error rather than misreporting a
                # provider outage or invalid credentials as "no radar data".
                raise failures[0]
        return entries

    async def discover(self, query, context):
        if query.at is not None or query.start is not None:
            # SIDARMA exposes a rolling recent list but does not guarantee historical retention.
            raise UnsupportedQueryError("id_sidarma only supports latest frames")
        return await super().discover(query, context)


__all__ = ["IdSidarmaSource"]
