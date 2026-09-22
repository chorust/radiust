from __future__ import annotations

import io
import json
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.decoders.exact import QUALITY_BELOW_DETECTION, QUALITY_MISSING
from radiust.errors import DecodeError, UnknownColorError
from radiust.models import Artifact, Query
from radiust.raw import RawFrame
from radiust.registry import registry
from radiust.sources.pt import PtSource

INDEX = "https://www.ipma.pt/resources.www/transf/radar/imgs-radar-md.json"
IMAGE = "https://www.ipma.pt/resources.www/transf/radar/mad/pcr_pst-2026-09-18T0250.png"


class ReplayTransport:
    def __init__(self, responses): self.responses = responses
    async def get(self, url, *, headers=None): return self.responses[url]


@pytest.mark.asyncio
async def test_pt_decodes_official_rainfall_classes_on_the_madeira_overlay_grid(tmp_path):
    payload = json.dumps({"Madeira": [{"date": "2026-09-18 02:50", "path": "pcr_pst-2026-09-18T0250.png"}]}).encode()
    image = (Path(__file__).parents[1] / "fixtures/sources/pt/raw/PTST2.png").read_bytes()
    source = PtSource(registry.get_info("pt"))
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "pt", transport=ReplayTransport({INDEX: payload, IMAGE: image}), temp_root=tmp_path)
    try:
        ref = (await source.discover(Query("pt", latest=True), context))[0]
        assert ref.station == "PTST2"
        assert ref.valid_time == datetime(2026, 9, 18, 2, 50, tzinfo=timezone.utc)
        assert ref.metadata["geometry_status"] == "verified_ipma_image_overlay"
        assert ref.metadata["value_semantics"] == "ordinal_ipma_rainfall_intensity_category"
        assert source.info.default_product is not None
        assert source.info.default_product.variables == ("rain_intensity",)
        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].sha256 == "fc44efba8f3c2b8ddfd71b9ba25d54282a2311c423044133a483d7a83eab18fa"
            field = source.decode(raw, context)
            assert field.variable == "rain_intensity"
            assert field.grid.kind == "geographic"
            assert field.grid.crs == "EPSG:4326"
            assert field.data.shape == (1526, 1500)
            assert field.data.attrs["kind"] == "categorical"
            assert field.data.attrs["units"] == "1"
            assert field.data.attrs["flag_values"] == list(range(17))
            assert field.data.attrs["flag_meanings"] == (
                "no_displayed_rain rate_0_05_to_0_1_mm_h rate_0_1_to_0_5_mm_h "
                "rate_0_5_to_1_mm_h rate_1_to_2_mm_h rate_2_to_4_mm_h rate_4_to_6_mm_h "
                "rate_6_to_8_mm_h rate_8_to_10_mm_h rate_10_to_20_mm_h rate_20_to_30_mm_h "
                "rate_30_to_40_mm_h rate_40_to_50_mm_h rate_50_to_70_mm_h rate_70_to_100_mm_h "
                "rate_100_to_200_mm_h rate_200_to_over_300_mm_h"
            )

            west, south, east, north = -19.87034, 30.01467, -12.89697, 35.96053
            assert field.grid.longitude[0] == pytest.approx(west + (east - west) / 3000)
            assert field.grid.longitude[-1] == pytest.approx(east - (east - west) / 3000)
            assert field.grid.latitude[0] == pytest.approx(north - (north - south) / 3052)
            assert field.grid.latitude[-1] == pytest.approx(south + (north - south) / 3052)

            pixels = np.asarray(Image.open(io.BytesIO(image)).convert("RGBA"))
            references = (
                ((0, 138, 80), 4),
                ((0, 150, 137), 3),
                ((0, 186, 188), 2),
                ((0, 116, 232), 1),
            )
            for rgb, category in references:
                mask = np.all(pixels[:, :, :3] == rgb, axis=2) & (pixels[:, :, 3] != 0)
                assert mask.any()
                assert np.all(field.data.values[mask] == category)
                assert np.all(field.quality.values[mask] == 0)

            assert field.data.values[0, 0] == 0
            assert field.quality.values[0, 0] == QUALITY_BELOW_DETECTION
            opaque_black = np.all(pixels == (0, 0, 0, 255), axis=2)
            assert opaque_black.any()
            assert np.isnan(field.data.values[opaque_black]).all()
            assert np.all(field.quality.values[opaque_black] == QUALITY_MISSING)
            assert field.provenance["value_semantics"] == "ordinal_ipma_rainfall_intensity_category"
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_pt_rejects_an_opaque_color_outside_the_official_legend(tmp_path):
    payload = json.dumps({"Madeira": [{"date": "2026-09-18 02:50", "path": "pcr_pst-2026-09-18T0250.png"}]}).encode()
    image = (Path(__file__).parents[1] / "fixtures/sources/pt/raw/PTST2.png").read_bytes()
    source = PtSource(registry.get_info("pt"))
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "pt", transport=ReplayTransport({INDEX: payload, IMAGE: image}), temp_root=tmp_path)
    try:
        ref = (await source.discover(Query("pt", latest=True), context))[0]
        pixels = np.asarray(Image.open(io.BytesIO(image)).convert("RGBA")).copy()
        pixels[0, 0] = (255, 255, 255, 255)
        encoded = io.BytesIO()
        Image.fromarray(pixels).save(encoded, format="PNG")
        raw = RawFrame(
            ref,
            (Artifact(name="unknown.png", role="data", media_type="image/png", payload=encoded.getvalue()),),
        )
        try:
            with pytest.raises(UnknownColorError):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_pt_wraps_non_utf8_timeline_as_decode_error(tmp_path):
    source = PtSource(registry.get_info("pt"))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "pt",
        transport=ReplayTransport({INDEX: b"\xff"}),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(DecodeError, match="IPMA returned invalid timeline JSON"):
            await source.discover_entries(context)
    finally:
        context.close()
