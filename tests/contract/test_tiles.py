from __future__ import annotations

from io import BytesIO

import pytest
from PIL import Image
from radiust.sources.tiles import TilePlan, assemble_tiles


def _png(color: tuple[int, int, int, int]) -> bytes:
    image = Image.new("RGBA", (1, 1), color)
    buffer = BytesIO()
    image.save(buffer, format="PNG")
    return buffer.getvalue()


def test_tile_plan_requires_complete_coverage_and_preserves_rgba():
    plan = TilePlan(columns=2, rows=1, tile_width=1, tile_height=1)
    result = assemble_tiles(plan, {(0, 0): _png((1, 2, 3, 4)), (1, 0): _png((5, 6, 7, 8))})

    assert result.image.size == (2, 1)
    assert result.image.getpixel((0, 0)) == (1, 2, 3, 4)
    assert result.image.getpixel((1, 0)) == (5, 6, 7, 8)

    with pytest.raises(ValueError, match="missing"):
        assemble_tiles(plan, {(0, 0): _png((1, 2, 3, 4))})


def test_raw_only_bypasses_mosaic_and_returns_no_derived_image():
    plan = TilePlan(columns=1, rows=1, tile_width=1, tile_height=1)
    result = assemble_tiles(plan, {(0, 0): b"not decoded"}, raw_only=True)
    assert result.image is None
    assert result.raw_payloads == {(0, 0): b"not decoded"}
