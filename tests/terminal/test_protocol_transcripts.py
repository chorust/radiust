from __future__ import annotations

import io

import numpy as np
import pytest
import xarray as xr
from radiust.field import RadarField
from radiust.grids import GeographicGrid
from radiust.outputs.netcdf import write_netcdf
from radiust.terminal.api import show


class _TtyBuffer(io.StringIO):
    def isatty(self) -> bool:
        return True


def test_text_renderer_never_emits_image_escape_sequences():
    image = RadarField(
        xr.DataArray(np.zeros((1, 1), dtype="float32"), dims=("latitude", "longitude"), name="reflectivity"),
        GeographicGrid([0], [0]),
    )
    stream = io.StringIO()

    show(image, renderer="text", stream=stream)

    assert "\x1b" not in stream.getvalue()


def test_image_renderer_rejects_non_tty_before_writing():
    image = RadarField(
        xr.DataArray(np.zeros((1, 1), dtype="float32"), dims=("latitude", "longitude"), name="reflectivity"),
        GeographicGrid([0], [0]),
    )
    stream = io.StringIO()
    with pytest.raises(ValueError, match="requires a TTY"):
        show(image, renderer="ansi", stream=stream)
    assert stream.getvalue() == ""


def test_ansi_renderer_loads_netcdf_paths(tmp_path):
    field = RadarField(
        xr.DataArray(
            np.array([[1.0, 2.0], [3.0, 4.0]], dtype="float32"),
            dims=("latitude", "longitude"),
            name="reflectivity",
            attrs={"units": "dBZ"},
        ),
        GeographicGrid([0.0, 1.0], [1.0, 0.0]),
        provenance={"source": "test", "product": "composite", "valid_time": "2025-01-01T00:00:00Z"},
    )
    path = tmp_path / "field.nc"
    write_netcdf(field, path)
    stream = _TtyBuffer()

    show(path, renderer="ansi", stream=stream, width=2, height=1)

    output = stream.getvalue()
    assert "test / composite" in output
    assert "legend=" in output
    assert "▀" in output
