"""BMKG public TMS radar tile acquisition."""

from __future__ import annotations

from datetime import datetime

from ..context import SourceContext
from ._legacy_tiles import FourTileSource


class BmkgSource(FourTileSource):
    cadence_seconds = 600

    def tile_url(self, valid_time: datetime, x: int, y: int, context: SourceContext) -> str:
        stamp = valid_time.strftime("%Y%m%d%H%M")
        tms_y = (2**self.zoom - 1) - y
        return (
            "https://inasiam.bmkg.go.id/api23/mpl_req/radar/radar/0/"
            f"{stamp}/{stamp}/{self.zoom}/{x}/{tms_y}.png?overlays=contourf"
        )


__all__ = ["BmkgSource"]
