from datetime import datetime, timezone
from io import BytesIO

import numpy as np
import xarray as xr
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.models import Artifact, FrameRef, ProductInfo, SourceInfo
from radiust.raw import RawFrame
from radiust.sources.legacy import LegacyImageSource


class GraySource(LegacyImageSource):
    value_mode = "legacy_gray_dbz"
    default_bbox = (100.0, 0.0, 102.0, 1.0)


def _info() -> SourceInfo:
    return SourceInfo(
        id="gray-test",
        description="test source",
        adapter_version="1",
        products=(
            ProductInfo(
                id="composite",
                variables=("reflectivity",),
                units={"reflectivity": "dBZ"},
                default=True,
            ),
        ),
    )


def _raw() -> RawFrame:
    image = Image.fromarray(np.array([[0, 16], [32, 224]], dtype=np.uint8), mode="L")
    payload = BytesIO()
    image.save(payload, format="PNG")
    ref = FrameRef(
        source="gray-test",
        product="composite",
        valid_time=datetime(2026, 9, 22, tzinfo=timezone.utc),
        locator={"fixture": True},
    )
    return RawFrame(
        ref,
        (Artifact(name="gray.png", role="data", media_type="image/png", payload=payload.getvalue()),),
        metadata={"bbox": (100.0, 0.0, 102.0, 1.0)},
    )


def test_legacy_image_source_uses_gray_dbz_decoder():
    raw = _raw()
    context = SourceContext(load_config({"runtime": {"allow_network": False}}, environ={}), "gray-test")
    try:
        field = GraySource(_info()).decode(raw, context)

        assert isinstance(field.data, xr.DataArray)
        np.testing.assert_allclose(field.data.values, [[0.0, 5.0], [10.0, 70.0]])
        assert field.data.attrs["units"] == "dBZ"
        assert field.provenance["value_semantics"] == "旧项目-gray-dbz-v1"
    finally:
        raw.close()
        context.close()
