from __future__ import annotations

import numpy as np
import pytest
import xarray as xr
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import ResourceLimitError
from radiust.field import RadarField
from radiust.grids import GeographicGrid


def test_context_enforces_pixel_and_temporary_byte_limits(tmp_path):
    config = load_config(
        {
            "runtime": {"max_pixels": 3, "max_temp_bytes": 4},
            "storage": {"output": str(tmp_path / "output")},
            "cache": {"dir": str(tmp_path / "cache")},
        },
        environ={},
    )
    context = SourceContext(config, "test")
    try:
        with pytest.raises(ResourceLimitError, match="pixels"):
            context.check_pixels(4)
        with pytest.raises(ResourceLimitError, match="temporary"):
            context.check_temp_bytes(5)
    finally:
        context.close()


def test_field_shape_can_be_checked_before_expensive_encoding(tmp_path):
    config = load_config(
        {"runtime": {"max_pixels": 3}, "storage": {"output": str(tmp_path / "output")}, "cache": {"dir": str(tmp_path / "cache")}},
        environ={},
    )
    context = SourceContext(config, "test")
    try:
        field = RadarField(
            xr.DataArray(np.zeros((2, 2), dtype="float32"), dims=("latitude", "longitude")),
            GeographicGrid([0, 1], [1, 0]),
        )
        with pytest.raises(ResourceLimitError):
            context.check_value(field)
    finally:
        context.close()
