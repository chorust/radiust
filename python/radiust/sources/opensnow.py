"""OpenSnow RainViewer-like radar tile acquisition."""

from __future__ import annotations

from datetime import datetime

from ..context import SourceContext
from ._legacy_tiles import FourTileSource


class OpenSnowSource(FourTileSource):
    cadence_seconds = 600

    def tile_url(self, valid_time: datetime, x: int, y: int, context: SourceContext) -> str:
        timestamp = int(valid_time.timestamp())
        return f"https://opensnow.com/tiles/rvp/v2/radar/{timestamp}/256/1/{x}/{y}/6/1_0.png"


__all__ = ["OpenSnowSource"]
