"""Shared raw-preserving acquisition for legacy XYZ radar tile providers."""

from __future__ import annotations

import io
from datetime import datetime, timezone

from PIL import Image

from ..context import SourceContext
from ..errors import IntegrityError
from ..identity import artifact_bytes
from ..models import FrameRef
from ..raw import RawFrame
from .legacy import LegacyImageSource


class FourTileSource(LegacyImageSource):
    """Acquire a z=1 world tile set while leaving science evidence-gated."""

    cadence_seconds = 600
    zoom = 1
    tile_size = 256
    tile_extension = "png"
    # The migrated legacy tile endpoints only expose a cadence-derived current
    # locator.  Keep explicit historical queries fail-closed until a provider
    # archive and frame-time contract are independently verified.
    historical = False

    def frame_time(self) -> datetime:
        now = int(datetime.now(timezone.utc).timestamp())
        floored = now // self.cadence_seconds * self.cadence_seconds
        return datetime.fromtimestamp(floored, timezone.utc)

    def tile_url(self, valid_time: datetime, x: int, y: int, context: SourceContext) -> str:
        raise NotImplementedError

    async def discover_entries(self, context: SourceContext):
        valid_time = self.frame_time()
        artifacts = []
        for y in range(2):
            for x in range(2):
                artifacts.append(
                    {
                        "url": self.tile_url(valid_time, x, y, context),
                        "name": f"tile-z1-x{x}-y{y}.{self.tile_extension}",
                        "role": "tile",
                        "media_type": f"image/{self.tile_extension}",
                    }
                )
        primary, *rest = artifacts
        return [
            {
                "url": primary["url"],
                "name": primary["name"],
                "artifacts": rest,
                "station": "global",
                "valid_time": valid_time,
                "revision": f"{self.info.id}-{int(valid_time.timestamp())}",
                "metadata": {
                    "time_semantics": "cadence_floor_assumption",
                    "tile_zoom": self.zoom,
                    "tile_layout": "xyz-2x2-global",
                    "geometry_status": "tile_geometry_known_scientific_values_unverified",
                },
            }
        ]

    def _validate_tile_frame(self, raw: RawFrame) -> RawFrame:
        expected = (1 << self.zoom) ** 2
        if len(raw.artifacts) != expected:
            raise IntegrityError(
                f"source {self.info.id} returned {len(raw.artifacts)} tiles; expected {expected}"
            )
        names = [artifact.name for artifact in raw.artifacts]
        if len(set(names)) != len(names):
            raise IntegrityError(f"source {self.info.id} returned duplicate tile names")
        for artifact in raw.artifacts:
            payload = artifact_bytes(artifact)
            try:
                with Image.open(io.BytesIO(payload)) as image:
                    if image.size != (self.tile_size, self.tile_size):
                        raise IntegrityError(
                            f"source {self.info.id} tile {artifact.name} is {image.width}x{image.height}; "
                            f"expected {self.tile_size}x{self.tile_size}"
                        )
                    image.verify()
            except IntegrityError:
                raise
            except Exception as exc:
                raise IntegrityError(
                    f"source {self.info.id} tile {artifact.name} is not a valid image: {exc}"
                ) from exc
        raw.metadata.update(
            {
                "tile_count": expected,
                "tile_size": self.tile_size,
                "tile_zoom": self.zoom,
                "tile_layout": "xyz-2x2-global",
            }
        )
        return raw

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        raw = await super().download(ref, context)
        try:
            return self._validate_tile_frame(raw)
        except BaseException:
            await raw.aclose()
            raise


__all__ = ["FourTileSource"]
