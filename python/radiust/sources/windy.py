"""Windy composite radar tile acquisition."""

from __future__ import annotations

import asyncio
from collections.abc import Mapping
from datetime import datetime

from ..context import SourceContext
from ..errors import DecodeError, TransportError
from ..identity import artifact_bytes
from ..models import Artifact, FrameRef, SourceInfo
from ..raw import RawFrame
from ._legacy_tiles import FourTileSource
from .browser import BrowserAcquirer
from .legacy import filename_from_url


class WindySource(FourTileSource):
    cadence_seconds = 300

    def __init__(self, info: SourceInfo, *, browser_acquirer: BrowserAcquirer | None = None) -> None:
        super().__init__(info)
        self._browser_acquirer = browser_acquirer or BrowserAcquirer()

    def tile_url(self, valid_time: datetime, x: int, y: int, context: SourceContext) -> str:
        path_time = valid_time.strftime("%Y/%m/%d/%H%M")
        max_time = valid_time.strftime("%Y%m%d%H%M%S")
        return (
            f"https://rdr.windy.com/radar2/composite/{path_time}/1/{x}/{y}/reflectivity.png"
            f"?multichannel=true&maxt={max_time}"
        )

    def _uses_playwright(self, context: SourceContext) -> bool:
        values = context.config.values.get("sources", {})
        configured = values.get("windy", {}) if isinstance(values, dict) else {}
        selected = configured.get("use_playwright", False) if isinstance(configured, dict) else False
        if not isinstance(selected, bool):
            raise DecodeError("sources.windy.use_playwright must be a boolean")
        return selected

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        if not self._uses_playwright(context):
            return await super().download(ref, context)
        context.cancellation.check()
        if not context.limits["allow_network"]:
            raise TransportError("public network access is disabled; Playwright tile acquisition requires opt-in")

        locator = dict(ref.locator)
        primary_url = str(locator["url"])
        requested: list[dict[str, object]] = [
            {
                "url": primary_url,
                "name": str(locator.get("name") or filename_from_url(primary_url)),
                "role": "data",
                "media_type": "image/png",
            }
        ]
        additional = locator.get("artifacts", ())
        if isinstance(additional, Mapping):
            additional = (additional,)
        requested.extend(dict(item) for item in additional if isinstance(item, Mapping))
        if not requested:
            raise DecodeError("Windy frame has no tile URLs")

        semaphore = asyncio.Semaphore(int(context.limits["request_concurrency"]))

        async def acquire_tile(item: dict[str, object]) -> Artifact:
            url = str(item["url"])
            name = filename_from_url(str(item.get("name") or url))
            async with semaphore:
                context.cancellation.check()
                raw = await self._browser_acquirer.acquire(
                    ref,
                    context,
                    page_url=url,
                    artifact_name=name,
                    headers=locator.get("headers") if isinstance(locator.get("headers"), Mapping) else None,
                )
                try:
                    context.cancellation.check()
                    context.check_artifacts(raw.artifacts)
                    if len(raw.artifacts) != 1:
                        raise DecodeError("Windy browser acquisition must return one artifact per tile")
                    payload = artifact_bytes(raw.artifacts[0])
                    self._validate_image(payload, name)
                    return Artifact(
                        name=name,
                        role=str(item.get("role", "tile")),
                        media_type=str(item.get("media_type") or raw.artifacts[0].media_type),
                        payload=payload,
                        source_revision=ref.revision,
                        size_bytes=len(payload),
                        sha256=raw.artifacts[0].sha256,
                    )
                finally:
                    await raw.aclose()

        results = await asyncio.gather(
            *(acquire_tile(item) for item in requested), return_exceptions=True
        )
        artifacts: list[Artifact] = []
        for result in results:
            if isinstance(result, BaseException):
                raise result
            artifacts.append(result)
        context.cancellation.check()
        context.check_artifacts(artifacts)
        context.check_temp_bytes(sum(artifact.size_bytes or 0 for artifact in artifacts))
        raw = RawFrame(
            ref,
            tuple(artifacts),
            metadata={
                "bbox": locator.get("bbox", self.default_bbox),
                "value_mode": self.value_mode,
                "acquisition": "playwright",
                "tile_count": len(artifacts),
            },
        )
        try:
            return self._validate_tile_frame(raw)
        except BaseException:
            await raw.aclose()
            raise


__all__ = ["WindySource"]
