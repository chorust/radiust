"""Philippines PAGASA radar timeline acquisition."""

from __future__ import annotations

import json
import re
from datetime import datetime
from pathlib import Path
from urllib.parse import urlencode
from zoneinfo import ZoneInfo

from ..context import SourceContext
from ..errors import (
    AuthenticationError,
    ConfigError,
    DecodeError,
    MissingDependencyError,
    ResourceLimitError,
    TransportError,
)
from ..identity import artifact_bytes
from ..models import FrameRef, SourceInfo
from ..raw import RawFrame
from .browser import BrowserAcquirer, acquire_with_http_fallback
from .legacy import LegacyImageSource


def _safe_browser_error(exc: Exception, operation: str) -> Exception:
    """Map browser failures to URL-free public errors.

    Playwright exceptions often include the navigated URL, which can contain
    PAGASA's configured timeline token or a signed image locator.
    """
    if isinstance(exc, AuthenticationError):
        return AuthenticationError(f"PAGASA browser {operation} was rejected")
    if isinstance(exc, MissingDependencyError):
        return MissingDependencyError("PAGASA browser fallback requires the 'playwright' extra")
    if isinstance(exc, ResourceLimitError):
        return ResourceLimitError(f"PAGASA browser {operation} exceeded configured resource limits")
    return TransportError(f"PAGASA browser {operation} failed")


class PhSource(LegacyImageSource):
    BASE_URL = "https://www.panahon.gov.ph"
    TIMELINE_URL = BASE_URL + "/api/v1/radar/timeline"
    provider_timezone = ZoneInfo("Asia/Manila")

    def __init__(
        self,
        info: SourceInfo,
        fixture_path: Path | None = None,
        *,
        browser_acquirer: BrowserAcquirer | None = None,
    ) -> None:
        super().__init__(info, fixture_path)
        self._browser_acquirer = browser_acquirer or BrowserAcquirer()

    def _source_config(self, context: SourceContext) -> dict:
        values = context.config.values.get("sources", {})
        configured = values.get("ph", {}) if isinstance(values, dict) else {}
        return configured if isinstance(configured, dict) else {}

    async def discover_entries(self, context: SourceContext):
        configured = self._source_config(context)
        token = configured.get("timeline_token")
        if not token:
            raise ConfigError("source ph requires sources.ph.timeline_token; legacy embedded tokens are not migrated")

        timeline_url = f"{self.TIMELINE_URL}?{urlencode({'token': token})}"
        try:
            page = await self._get(context, self.BASE_URL)
            text = page.decode("utf-8", errors="replace")
            csrf = re.search(
                r'<meta[^>]+name=["\']csrf-token["\'][^>]+content=["\']([^"\']+)["\']',
                text,
                re.IGNORECASE,
            )
            headers = {"Referer": self.BASE_URL}
            if csrf:
                headers["X-CSRF-TOKEN"] = csrf.group(1)
            payload = await self._get(context, timeline_url, headers=headers)
        except TransportError:
            context.cancellation.check()
            if not context.limits["allow_network"]:
                raise
            try:
                payload = await self._browser_acquirer.fetch_page(
                    timeline_url, context, warmup_url=self.BASE_URL,
                    csrf_selector='meta[name="csrf-token"]',
                    headers={"Referer": self.BASE_URL},
                )
            except Exception as exc:
                raise _safe_browser_error(exc, "timeline discovery") from None
        try:
            document = json.loads(payload)
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise DecodeError(f"PAGASA returned invalid timeline JSON: {exc}") from exc
        if not isinstance(document, dict):
            raise DecodeError("PAGASA timeline response must be an object")
        data = document.get("data")
        if not isinstance(data, dict):
            raise DecodeError("PAGASA timeline response has no data object")
        timeline = data.get("timeline")
        if not isinstance(timeline, list):
            raise DecodeError("PAGASA timeline response has no timeline list")
        entries = []
        for item in timeline:
            if not isinstance(item, dict):
                continue
            raw_time, url = item.get("observed_at"), item.get("image_url")
            if not isinstance(raw_time, str) or not isinstance(url, str):
                continue
            try:
                valid_time = datetime.strptime(raw_time, "%Y-%m-%d %H:%M:%S").replace(
                    tzinfo=self.provider_timezone
                )
            except ValueError:
                continue
            entries.append(
                {
                    "url": url,
                    "station": "PHCOMP4",
                    "valid_time": valid_time,
                    "headers": {"Referer": self.BASE_URL},
                    "revision": f"PHCOMP4-{valid_time.strftime('%Y%m%d%H%M%S')}",
                    "metadata": {"time_semantics": "provider_frame_time", "geometry_status": "unverified"},
                }
            )
        return entries

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        async def http() -> RawFrame:
            return await super(PhSource, self).download(ref, context)

        async def browser() -> RawFrame:
            # A disabled network policy or cancelled task cannot be rescued by
            # launching an independent browser process.
            context.cancellation.check()
            if not context.limits["allow_network"]:
                raise TransportError("public network access is disabled; browser fallback requires opt-in")
            try:
                raw = await self._browser_acquirer.acquire(
                    ref, context, warmup_url=self.BASE_URL,
                    csrf_selector='meta[name="csrf-token"]',
                    headers={"Referer": self.BASE_URL},
                )
            except Exception as exc:
                raise _safe_browser_error(exc, "artifact acquisition") from None
            try:
                context.cancellation.check()
                context.check_artifacts(raw.artifacts)
                for artifact in raw.artifacts:
                    self._validate_image(artifact_bytes(artifact), artifact.name)
                return raw
            except BaseException:
                await raw.aclose()
                raise

        # HTTP authentication and malformed image failures must remain fatal;
        # a browser cannot correct invalid credentials or provider frame data.
        return await acquire_with_http_fallback(http, browser)


__all__ = ["PhSource"]
