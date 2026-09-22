from __future__ import annotations

import numpy as np
import pytest
import xarray as xr
from radiust.field import RadarField
from radiust.grids import CartesianGrid, GeographicGrid
from radiust.rendering.core import RenderedImage, render_field
from radiust.rendering.palettes import DEFAULT_PALETTE, Palette
from radiust.terminal.ansi import render_ansi
from radiust.terminal.iterm2 import iterm2_sequence
from radiust.terminal.kitty import kitty_chunks


def _image() -> RenderedImage:
    rgba = np.zeros((2, 2, 4), dtype="uint8")
    rgba[0, 0] = [1, 2, 3, 255]
    rgba[1, 1] = [4, 5, 6, 255]
    return RenderedImage(rgba, DEFAULT_PALETTE, 0, 1, "title", ("0", "1"), "geographic", "EPSG:4326", {})


def test_ansi_uses_half_blocks_and_preserves_title_and_legend():
    rendered = render_ansi(_image(), width=2, height=1)
    assert "title" in rendered
    assert "legend=0 1" in rendered
    assert "▀" in rendered
    assert "\x1b[38;2;" in rendered


@pytest.mark.parametrize(
    "grid",
    [
        pytest.param(GeographicGrid(longitude=[20.0, 10.0], latitude=[-20.0, 20.0]), id="geographic"),
        pytest.param(CartesianGrid(x=[20.0, 10.0], y=[-20.0, 20.0]), id="cartesian"),
    ],
)
def test_regular_grid_render_places_north_west_at_top_left(grid):
    field = RadarField(
        xr.DataArray(
            np.array([[1.0, 0.0], [3.0, 2.0]], dtype="float32"),
            dims=("latitude", "longitude"),
            name="reflectivity",
            attrs={"units": "dBZ"},
        ),
        grid,
    )
    palette = Palette("gray", "1", ((0, 0, 0), (255, 255, 255)))

    rendered = render_field(field, palette=palette, vmin=0.0, vmax=3.0)

    # Coordinates run east-to-west and south-to-north; a map display must put
    # north-west at its top-left corner regardless of array storage order.
    np.testing.assert_array_equal(
        rendered.rgba[:, :, 0],
        np.array([[170, 255], [0, 85]], dtype="uint8"),
    )


def test_geographic_render_keeps_quality_mask_on_its_pixel_after_orientation():
    field = RadarField(
        xr.DataArray(
            np.array([[1.0, 0.0], [3.0, 2.0]], dtype="float32"),
            dims=("latitude", "longitude"),
            name="reflectivity",
            attrs={"units": "dBZ"},
        ),
        GeographicGrid(longitude=[20.0, 10.0], latitude=[-20.0, 20.0]),
        xr.DataArray(
            np.array([[1, 0], [0, 0]], dtype="uint16"),
            dims=("latitude", "longitude"),
            name="quality",
        ),
    )

    rendered = render_field(field, palette=Palette("gray", "1", ((0, 0, 0), (255, 255, 255))), vmin=0.0, vmax=3.0)

    # The masked source pixel is south-east; its display alpha must move with
    # the data when both coordinate axes are reversed.
    np.testing.assert_array_equal(
        rendered.rgba[:, :, 3],
        np.array([[255, 255], [255, 0]], dtype="uint8"),
    )


def test_image_protocols_are_chunked_and_bounded():
    chunks = kitty_chunks(b"0123456789", chunk_size=4)
    assert len(chunks) == 4
    assert all(chunk.startswith("\x1b_G") and chunk.endswith("\x1b\\") for chunk in chunks)
    assert "m=1" in chunks[0] and "m=0" in chunks[-1]
    sequence = iterm2_sequence(b"png", width=20, height=10)
    assert sequence.startswith("\x1b]1337;File=inline=1;width=20;height=10:")
    assert sequence.endswith("\x07")
