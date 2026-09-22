"""Source-independent lossless tile planning and RGBA assembly."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path

from PIL import Image


@dataclass(frozen=True, slots=True)
class TilePlan:
    columns: int
    rows: int
    tile_width: int
    tile_height: int

    def __post_init__(self) -> None:
        if min(self.columns, self.rows, self.tile_width, self.tile_height) < 1:
            raise ValueError("tile plan dimensions must be positive")

    @property
    def positions(self) -> frozenset[tuple[int, int]]:
        return frozenset((column, row) for row in range(self.rows) for column in range(self.columns))


@dataclass(frozen=True, slots=True)
class MosaicResult:
    image: Image.Image | None
    raw_payloads: dict[tuple[int, int], bytes]
    missing: tuple[tuple[int, int], ...] = ()


def _payload_bytes(payload: bytes | bytearray | memoryview | str | Path | Image.Image) -> bytes:
    if isinstance(payload, (bytes, bytearray, memoryview)):
        return bytes(payload)
    if isinstance(payload, Image.Image):
        return b""
    return Path(payload).read_bytes()


def assemble_tiles(
    plan: TilePlan,
    tiles: Mapping[tuple[int, int], bytes | bytearray | memoryview | str | Path | Image.Image],
    *,
    raw_only: bool = False,
    allow_partial: bool = False,
) -> MosaicResult:
    positions = set(tiles)
    unexpected = positions - plan.positions
    if unexpected:
        raise ValueError(f"tile positions are outside plan: {sorted(unexpected)!r}")
    missing = tuple(sorted(plan.positions - positions))
    raw_payloads = {position: _payload_bytes(payload) for position, payload in tiles.items()}
    if raw_only:
        return MosaicResult(None, raw_payloads, missing)
    if missing and not allow_partial:
        raise ValueError(f"missing tile(s): {missing!r}")
    image = Image.new("RGBA", (plan.columns * plan.tile_width, plan.rows * plan.tile_height), (0, 0, 0, 0))
    for (column, row), payload in tiles.items():
        current = payload if isinstance(payload, Image.Image) else Image.open(BytesIO(_payload_bytes(payload)))
        try:
            rgba = current.convert("RGBA")
            if rgba.size != (plan.tile_width, plan.tile_height):
                raise ValueError(f"tile {(column, row)!r} has size {rgba.size}, expected {(plan.tile_width, plan.tile_height)}")
            image.paste(rgba, (column * plan.tile_width, row * plan.tile_height))
        finally:
            if not isinstance(payload, Image.Image):
                current.close()
    return MosaicResult(image, raw_payloads, missing)

__all__ = ["MosaicResult", "TilePlan", "assemble_tiles"]
