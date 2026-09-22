from __future__ import annotations

import json
from datetime import datetime, timezone
from io import BytesIO

import numpy as np
import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, StaleFrameError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.tw import TwSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_tw_binds_png_to_official_json_time_and_preserves_both_artifacts(tmp_path, fixture_root):
    metadata_url = f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.json"
    image_url = f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.png"
    transport = ReplayTransport(
        {
            metadata_url: (fixture_root / "tw/raw/O-A0058-005.json").read_bytes(),
            image_url: (fixture_root / "tw/raw/O-A0058-005.png").read_bytes(),
        }
    )
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "tw", transport=transport, temp_root=tmp_path)
    source = TwSource(registry.get_info("tw"))
    ref = (await source.discover(Query("tw", latest=True), context))[0]
    assert ref.station == "CV1_3600"
    assert ref.valid_time == datetime(2026, 9, 18, 3, 40, tzinfo=timezone.utc)
    assert ref.metadata["provider_declared_extent"] == (115.0, 17.75, 126.5, 29.25)
    assert ref.metadata["provider_image_shape"] == (3600, 3600)
    assert ref.metadata["geometry_status"] == "unverified"
    raw = await source.download(ref, context)
    try:
        assert [artifact.role for artifact in raw.artifacts] == ["data", "metadata"]
        assert raw.artifacts[0].sha256 == "abfb1f72fa191d154e942b8be3bdab61610ae70f07cc137d41f557b4597be293"
        with pytest.raises(DecodeError, match="verified scientific decoder"):
            source.decode(raw, context)
    finally:
        raw.close()
        context.close()


@pytest.mark.asyncio
async def test_tw_png_rejects_metadata_rollover_after_discovery(tmp_path, fixture_root):
    metadata_url = f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.json"
    image_url = f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.png"
    discovery = (fixture_root / "tw/raw/O-A0058-005.json").read_bytes()
    newer = json.loads(discovery)
    newer["cwaopendata"]["dataset"]["DateTime"] = "2026-09-18T11:50:00+08:00"

    class RollingTransport:
        metadata_requests = 0

        async def get(self, url):
            if url == metadata_url:
                self.metadata_requests += 1
                return discovery if self.metadata_requests == 1 else json.dumps(newer).encode()
            assert url == image_url
            return (fixture_root / "tw/raw/O-A0058-005.png").read_bytes()

    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "tw", transport=RollingTransport(), temp_root=tmp_path,
    )
    source = TwSource(registry.get_info("tw"))
    try:
        ref = (await source.discover(Query("tw", latest=True), context))[0]
        with pytest.raises(StaleFrameError, match="time changed"):
            await source.download(ref, context)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_tw_png_rejects_changed_product_identity(tmp_path, fixture_root):
    metadata_url = f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.json"
    image_url = f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.png"
    discovery = (fixture_root / "tw/raw/O-A0058-005.json").read_bytes()
    altered = json.loads(discovery)
    altered["cwaopendata"]["dataid"] = "O-A0058-001"

    class ChangedProductTransport:
        metadata_requests = 0

        async def get(self, url):
            if url == metadata_url:
                self.metadata_requests += 1
                return discovery if self.metadata_requests == 1 else json.dumps(altered).encode()
            assert url == image_url
            return (fixture_root / "tw/raw/O-A0058-005.png").read_bytes()

    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "tw", transport=ChangedProductTransport(), temp_root=tmp_path,
    )
    source = TwSource(registry.get_info("tw"))
    try:
        ref = (await source.discover(Query("tw", latest=True), context))[0]
        with pytest.raises(DecodeError, match="product id"):
            await source.download(ref, context)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_tw_png_rejects_image_dimensions_different_from_provider_metadata(tmp_path, fixture_root):
    metadata_url = f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.json"
    image_url = f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.png"
    stream = BytesIO()
    Image.new("RGBA", (2, 3)).save(stream, format="PNG")
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}), "tw",
        transport=ReplayTransport({
            metadata_url: (fixture_root / "tw/raw/O-A0058-005.json").read_bytes(),
            image_url: stream.getvalue(),
        }), temp_root=tmp_path,
    )
    source = TwSource(registry.get_info("tw"))
    try:
        ref = (await source.discover(Query("tw", latest=True), context))[0]
        with pytest.raises(DecodeError, match="dimensions disagree"):
            await source.download(ref, context)
    finally:
        context.close()


def _numeric_fixture(*, content: str = "1.0,-9.900E+01,3.0,-9.990E+02,5.0,6.0", time: str = "2026-09-20T12:30:00+08:00") -> bytes:
    return json.dumps({
        "cwaopendata": {
            "dataid": "O-A0059-001",
            "dataset": {
                "datasetInfo": {"parameterSet": {
                    "StartPointLongitude": "115.0", "StartPointLatitude": "18.0",
                    "GridResolution": "0.0125", "GridDimensionX": "3", "GridDimensionY": "2",
                    "DateTime": time, "Reflectivity": "dBZ",
                }},
                "contents": {"contentDescription": "first southwest, east then north, TWD67", "content": content},
            },
        },
    }).encode()


@pytest.mark.asyncio
async def test_tw_numeric_product_preserves_native_twd67_axes_values_and_missing_flags(tmp_path):
    url = f"{TwSource.BUCKET_BASE}/{TwSource.GRID_KEY}.json"
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "tw", transport=ReplayTransport({url: _numeric_fixture()}), temp_root=tmp_path)
    source = TwSource(registry.get_info("tw"))
    try:
        ref = (await source.discover(Query("tw", product="grid", latest=True), context))[0]
        assert ref.valid_time == datetime(2026, 9, 20, 4, 30, tzinfo=timezone.utc)
        assert ref.uri == url
        raw = await source.download(ref, context)
        try:
            field = source.decode(raw, context)
            assert field.grid.crs == "EPSG:3821"
            assert field.grid.shape == (2, 3)
            assert field.grid.longitude == pytest.approx((115.0, 115.0125, 115.025))
            assert field.grid.latitude == pytest.approx((18.0, 18.0125))
            np.testing.assert_allclose(field.data.values, [[1, np.nan, 3], [np.nan, 5, 6]])
            np.testing.assert_array_equal(field.quality.values, [[0, 1, 0], [2, 0, 0]])
            assert field.data.attrs["units"] == "dBZ"
            dataset = field.to_dataset()
            assert dataset.crs.attrs["spatial_ref"] == "EPSG:3821"
            assert dataset.crs.attrs["semi_major_axis"] == 6378160.0
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("content", ["1,2,3,4,5", "1,2,3,4,5,nan"])
async def test_tw_numeric_grid_rejects_incomplete_or_nonfinite_payload(tmp_path, content):
    url = f"{TwSource.BUCKET_BASE}/{TwSource.GRID_KEY}.json"
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "tw", transport=ReplayTransport({url: _numeric_fixture(content=content)}), temp_root=tmp_path)
    source = TwSource(registry.get_info("tw"))
    try:
        ref = (await source.discover(Query("tw", product="grid", latest=True), context))[0]
        raw = await source.download(ref, context)
        try:
            with pytest.raises(DecodeError, match="incomplete or non-finite"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_tw_numeric_grid_detects_upstream_frame_rollover(tmp_path):
    url = f"{TwSource.BUCKET_BASE}/{TwSource.GRID_KEY}.json"

    class RollingTransport:
        calls = 0

        async def get(self, requested):
            assert requested == url
            self.calls += 1
            if self.calls == 1:
                return _numeric_fixture()
            return _numeric_fixture(time="2026-09-20T12:40:00+08:00")

    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "tw", transport=RollingTransport(), temp_root=tmp_path)
    source = TwSource(registry.get_info("tw"))
    try:
        ref = (await source.discover(Query("tw", product="grid", latest=True), context))[0]
        raw = await source.download(ref, context)
        try:
            with pytest.raises(DecodeError, match="time changed"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_tw_official_numeric_raw_replay_matches_reference_pixels_and_quality(tmp_path, fixture_root):
    """Replay the retained provider payload, including its full scientific grid."""
    fixture = json.loads((fixture_root / "tw/fixture.json").read_text(encoding="utf-8"))
    frame = next(item for item in fixture["frames"] if item["product"] == "grid")
    artifact = frame["artifacts"][0]
    payload = (fixture_root / "tw" / artifact["path"]).read_bytes()
    url = f"{TwSource.BUCKET_BASE}/{TwSource.GRID_KEY}.json"
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "tw", transport=ReplayTransport({url: payload}), temp_root=tmp_path,
    )
    source = TwSource(registry.get_info("tw"))
    try:
        ref = (await source.discover(Query("tw", product="grid", latest=True), context))[0]
        assert ref.valid_time.isoformat().replace("+00:00", "Z") == frame["valid_time"]
        raw = await source.download(ref, context)
        try:
            assert raw.artifacts[0].sha256 == artifact["sha256"]
            field = source.decode(raw, context)
            metadata = frame["metadata"]
            assert field.grid.shape == tuple(metadata["reference_shape"])
            assert field.grid.crs == metadata["reference_grid"]["crs"]
            assert field.grid.extent == (115.0, 18.0, 126.5, 29.0)
            quality = field.quality.values
            values = field.data.values
            assert int(np.count_nonzero(quality == 1)) == metadata["reference_quality"]["missing_minus_99"]
            assert int(np.count_nonzero(quality == 2)) == metadata["reference_quality"]["outside_or_removed_minus_999"]
            assert int(np.isfinite(values).sum()) == metadata["reference_quality"]["finite"]
            for pixel in metadata["reference_pixels"]:
                row, column = pixel["row"], pixel["column"]
                assert values[row, column] == pixel["value"]
                assert quality[row, column] == pixel["quality"]
        finally:
            raw.close()
    finally:
        context.close()
