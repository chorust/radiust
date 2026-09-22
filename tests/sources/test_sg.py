from __future__ import annotations

import io
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pytest
from PIL import Image
from pyproj import Transformer
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.decoders.exact import QUALITY_BELOW_DETECTION, QUALITY_OUTSIDE
from radiust.errors import NoDataError
from radiust.grids import CartesianGrid
from radiust.models import Artifact, Query
from radiust.raw import RawFrame
from radiust.registry import registry
from radiust.sources.sg import SgSource

PAGE = "https://www.weather.gov.sg/weather-rain-area-240km"
IMAGE = "https://www.weather.gov.sg/files/rainarea/240km/dpsri_240km_2026091810450000dBR.dpsri.png"


class ReplayTransport:
    def __init__(self, responses): self.responses = responses
    async def get(self, url, *, headers=None): return self.responses[url]


@pytest.mark.asyncio
async def test_sg_discovers_slideshow_and_preserves_raw(tmp_path):
    html = f'<script>slideshowimages("{IMAGE}");</script>'.encode()
    image = (Path(__file__).parents[1] / "fixtures/sources/sg/raw/SGCOMP.png").read_bytes()
    source = SgSource(registry.get_info("sg"))
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "sg", transport=ReplayTransport({PAGE: html, IMAGE: image}), temp_root=tmp_path)
    try:
        ref = (await source.discover(Query("sg", latest=True), context))[0]
        assert ref.station == "SGCOMP"
        assert ref.valid_time == datetime(2026, 9, 18, 2, 45, tzinfo=timezone.utc)
        assert ref.metadata["geometry_status"] == "verified_aeqd_240km"
        assert ref.metadata["value_semantics"] == "provider_rainfall_intensity_categories"
        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].sha256 == "154722ed6221d6d634de24006eaab0e5a48fcee75ef6147569186a56bf731b2c"
            field = source.decode(raw, context)
            assert field.variable == "rain_intensity"
            assert isinstance(field.grid, CartesianGrid)
            assert field.grid.crs is not None and "+proj=aeqd" in field.grid.crs
            assert field.data.attrs["kind"] == "categorical"
            assert field.data.attrs["flag_values"] == [0, 1, 2, 3, 4, 5]
            assert field.data.attrs["flag_meanings"] == (
                "no_displayed_rain light light_to_moderate moderate moderate_to_heavy heavy"
            )
            assert field.data.attrs["units"] == "1"

            pixels = np.asarray(Image.open(Path(__file__).parents[1] / "fixtures/sources/sg/raw/SGCOMP.png").convert("RGBA"))
            values = field.data.values
            quality = field.quality.values
            representatives = (
                ((0, 255, 255), 1),
                ((0, 218, 13), 2),
                ((255, 255, 59), 3),
                ((255, 198, 0), 4),
                ((255, 73, 0), 5),
            )
            for rgb, category in representatives:
                mask = (
                    np.all(pixels[:, :, :3] == rgb, axis=2)
                    & (pixels[:, :, 3] != 0)
                    & (quality == 0)
                )
                assert mask.any()
                assert np.all(values[mask] == category)
                assert np.all(quality[mask] == 0)

            x, y = np.meshgrid(field.grid.x, field.grid.y)
            radius = np.hypot(x, y)
            transparent_in_range = np.argwhere((pixels[:, :, 3] == 0) & (radius < 239_000))
            assert len(transparent_in_range) > 0
            row, column = transparent_in_range[0]
            assert values[row, column] == 0
            assert quality[row, column] == QUALITY_BELOW_DETECTION
            assert np.isnan(values[0, 0])
            assert quality[0, 0] == QUALITY_OUTSIDE

            assert field.grid.x[0] == pytest.approx(-239_500)
            assert field.grid.x[-1] == pytest.approx(239_500)
            assert field.grid.y[0] == pytest.approx(239_500)
            assert field.grid.y[-1] == pytest.approx(-239_500)
            transformer = Transformer.from_crs(field.grid.crs, "EPSG:4326", always_xy=True)
            upper_left = transformer.transform(field.grid.x[0] - 500, field.grid.y[0] + 500)
            lower_right = transformer.transform(field.grid.x[-1] + 500, field.grid.y[-1] - 500)
            assert upper_left == pytest.approx((101.810507, 3.506012), abs=0.00025)
            assert lower_right == pytest.approx((106.130495, -0.809711), abs=0.00025)

            # The API rendition uses white RGB under transparent pixels while
            # the legacy slideshow uses black; both must decode identically.
            api_pixels = pixels.copy()
            api_pixels[api_pixels[:, :, 3] == 0] = (255, 255, 255, 0)
            encoded = io.BytesIO()
            Image.fromarray(api_pixels).save(encoded, format="PNG")
            api_raw = RawFrame(
                ref,
                (Artifact(name="api.png", role="data", media_type="image/png", payload=encoded.getvalue()),),
            )
            try:
                api_field = source.decode(api_raw, context)
                assert np.array_equal(field.data.values, api_field.data.values, equal_nan=True)
                assert np.array_equal(field.quality.values, api_field.quality.values)
            finally:
                api_raw.close()
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_sg_missing_frame_is_not_decoded_as_no_rain(tmp_path):
    source = SgSource(registry.get_info("sg"))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "sg",
        transport=ReplayTransport({PAGE: b"<html>no slideshow frames</html>"}),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(NoDataError, match="no matching frame"):
            await source.discover(Query("sg", latest=True), context)
    finally:
        context.close()
