"""Independent Pillow and JSON readback of Rust-generated PNG output."""

from __future__ import annotations

import json
from pathlib import Path

from PIL import Image

FIXTURE = Path(__file__).parents[1] / "fixtures/rust-migration/output/png"


def test_python_can_read_rust_png_and_render_sidecar() -> None:
    with Image.open(FIXTURE / "decoded.png") as image:
        rgba = image.convert("RGBA")
        assert rgba.size == (2, 2)
        assert rgba.getpixel((0, 0)) == (215, 25, 28, 0)
        assert rgba.getpixel((1, 0)) == (0, 0, 0, 0)
        assert rgba.getpixel((0, 1)) == (0, 0, 0, 255)
        assert rgba.getpixel((1, 1)) == (213, 238, 177, 255)

    sidecar = json.loads((FIXTURE / "decoded.render.json").read_text(encoding="utf-8"))
    assert sidecar == {
        "crs": "EPSG:4326",
        "grid": "geographic",
        "legend": ["0", "0.5", "1", "1.5", "2"],
        "palette": {"id": "default", "version": "1"},
        "provenance": {"product": "composite", "source": "fixture", "station": "east"},
        "schema_version": 1,
        "shape": [2, 2],
        "title": "fixture / composite / east / 2026-09-26T00:00:00.000000Z / reflectivity [dBZ]",
        "variable": "reflectivity",
        "vmax": 2.0,
        "vmin": 0.0,
    }
