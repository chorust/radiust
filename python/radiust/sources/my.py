"""Malaysia Met Department composite image acquisition."""

from __future__ import annotations

from datetime import timedelta
from email.utils import parsedate_to_datetime
from io import BytesIO
from pathlib import Path

from PIL import Image

from ..context import SourceContext
from ..errors import IntegrityError, TransportError
from ..models import FrameRef, SourceInfo, utc_datetime
from ..raw import RawFrame
from .legacy import LegacyImageSource


class MySource(LegacyImageSource):
    """Acquire the two live composite images while preserving legacy time semantics.

    The former scraper associated each image with ``Last-Modified - 9 minutes``.
    A retained sample's filename timestamp matches its embedded Malaysian-time
    label, but the provider's timestamp contract has not been independently
    verified. That uncertainty is carried on every frame reference.
    """

    STATIONS = {
        "peninsular": "https://www.met.gov.my/data/radar_peninsular.gif",
        "east": "https://www.met.gov.my/data/radar_east.gif",
    }
    LEGACY_PUBLICATION_DELAY = timedelta(minutes=9)
    historical = False
    PAYLOAD_MEDIA_TYPE = "image/png"
    OBSERVED_PROVIDER_MEDIA_TYPES = frozenset({"image/gif", "image/png"})

    def __init__(self, info: SourceInfo, fixture_path: Path | None = None) -> None:
        super().__init__(info, fixture_path)

    async def _head(self, context: SourceContext, url: str):
        context.cancellation.check()
        request = getattr(context.transport, "head_response", None)
        if request is not None:
            return await request(url)
        # Small custom transports may expose only GET-with-headers. Keep their
        # behavior useful while the production HTTPTransport uses HEAD.
        request = getattr(context.transport, "get_response", None)
        if request is not None:
            return await request(url)
        raise TransportError("source my requires a response-header transport")

    async def discover_entries(self, context: SourceContext) -> list[dict]:
        entries: list[dict] = []
        for station, url in self.STATIONS.items():
            response = await self._head(context, url)
            if not 200 <= int(response.status) < 300:
                raise TransportError(f"source my metadata request failed with status {response.status}")
            headers = {str(key).lower(): str(value) for key, value in response.headers.items()}
            raw_modified = headers.get("last-modified")
            if not raw_modified:
                raise TransportError("source my response omitted Last-Modified; refusing to infer valid time")
            try:
                modified = parsedate_to_datetime(raw_modified)
            except (TypeError, ValueError, OverflowError) as exc:
                raise TransportError("source my returned an invalid Last-Modified timestamp") from exc
            if modified.tzinfo is None:
                raise TransportError("source my Last-Modified timestamp has no timezone")
            content_type = headers.get("content-type", "").split(";", 1)[0].strip().lower()
            if content_type not in self.OBSERVED_PROVIDER_MEDIA_TYPES:
                raise TransportError("source my metadata response has an unsupported image Content-Type")

            valid_time = utc_datetime(modified) - self.LEGACY_PUBLICATION_DELAY
            # The live endpoints are named .gif and advertise image/gif, but a
            # bounded response inspection found PNG signatures. Keep the
            # observed provider header as metadata and name the acquired bytes
            # according to the format that download() will verify.
            entries.append(
                {
                    "url": url,
                    "station": station,
                    "valid_time": valid_time,
                    "revision": f"{station}-{int(valid_time.timestamp())}",
                    "name": f"my_{station}.png",
                    "media_type": self.PAYLOAD_MEDIA_TYPE,
                    "metadata": {
                        "time_semantics": "legacy_last_modified_minus_9_minutes",
                        "time_binding_status": "legacy_rule_unverified",
                        "geometry_status": "unverified",
                        "provider_content_type": content_type,
                        "provider_last_modified": raw_modified,
                        "payload_media_type": self.PAYLOAD_MEDIA_TYPE,
                    },
                }
            )
        return entries

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        raw = await super().download(ref, context)
        expected_type = ref.metadata.get("payload_media_type")
        if not expected_type:
            return raw
        try:
            artifact = next(item for item in raw.artifacts if item.role == "data")
            payload = artifact.payload if isinstance(artifact.payload, bytes) else Path(artifact.payload).read_bytes()
            with Image.open(BytesIO(payload)) as image:
                actual_type = Image.MIME.get(image.format or "")
                image.verify()
            if actual_type != expected_type:
                raise IntegrityError("source my payload format does not match the verified PNG representation")
            provider_type = ref.metadata.get("provider_content_type")
            raw.metadata.update(
                {
                    "provider_content_type": provider_type,
                    "provider_last_modified": ref.metadata.get("provider_last_modified"),
                    "payload_media_type": actual_type,
                    "content_type_mismatch": provider_type != actual_type,
                }
            )
            return raw
        except BaseException:
            await raw.aclose()
            raise


__all__ = ["MySource"]
