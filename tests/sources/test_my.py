from datetime import datetime, timezone
from io import BytesIO

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import IntegrityError, TransportError, UnsupportedQueryError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.my import MySource
from radiust.transport import HTTPResponse

pytestmark = pytest.mark.source_adapter


class ResponseReplayTransport:
    def __init__(self, responses):
        self.responses = responses
        self.head_calls = []
        self.get_calls = []

    async def head_response(self, url, *, headers=None):
        self.head_calls.append(url)
        status_headers, _payload = self.responses[url]
        return HTTPResponse(b"", 200, url, status_headers)

    async def get(self, url, *, headers=None):
        self.get_calls.append(url)
        return self.responses[url][1]


def _gif() -> bytes:
    buffer = BytesIO()
    Image.new("RGB", (3, 2), (40, 80, 120)).save(buffer, format="GIF")
    return buffer.getvalue()


def _png() -> bytes:
    buffer = BytesIO()
    Image.new("RGB", (3, 2), (40, 80, 120)).save(buffer, format="PNG")
    return buffer.getvalue()


def test_my_registry_does_not_replace_provider_data_with_test_fixture():
    source = registry.get("my")
    assert isinstance(source, MySource)
    assert source.fixture_path is None


@pytest.mark.asyncio
async def test_my_discovery_binds_provider_last_modified_to_legacy_scan_time(tmp_path):
    payload = _png()
    transport = ResponseReplayTransport(
        {
            MySource.STATIONS["peninsular"]: (
                {"content-type": "image/gif", "last-modified": "Mon, 29 Dec 2025 06:59:01 GMT"},
                payload,
            ),
            MySource.STATIONS["east"]: (
                {"content-type": "image/gif", "last-modified": "Mon, 29 Dec 2025 07:01:01 GMT"},
                payload,
            ),
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "my",
        transport=transport,
        temp_root=tmp_path,
    )
    source = MySource(registry.get_info("my"))
    try:
        refs = await source.discover(Query("my", latest=True), context)
        by_station = {ref.station: ref for ref in refs}
        assert by_station["peninsular"].valid_time == datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc)
        assert by_station["east"].valid_time == datetime(2025, 12, 29, 6, 52, 1, tzinfo=timezone.utc)
        assert by_station["peninsular"].metadata["time_semantics"] == "legacy_last_modified_minus_9_minutes"
        assert by_station["peninsular"].metadata["time_binding_status"] == "legacy_rule_unverified"
        assert by_station["peninsular"].metadata["provider_content_type"] == "image/gif"
        assert by_station["peninsular"].metadata["provider_last_modified"] == "Mon, 29 Dec 2025 06:59:01 GMT"
        assert by_station["peninsular"].metadata["payload_media_type"] == "image/png"
        assert set(transport.head_calls) == set(MySource.STATIONS.values())

        raw = await source.download(by_station["peninsular"], context)
        try:
            assert raw.artifacts[0].media_type == "image/png"
            assert raw.artifacts[0].name.endswith(".png")
            assert raw.artifacts[0].payload.read_bytes() == payload
            assert raw.metadata["payload_media_type"] == "image/png"
            assert raw.metadata["provider_content_type"] == "image/gif"
            assert raw.metadata["provider_last_modified"] == "Mon, 29 Dec 2025 06:59:01 GMT"
            assert raw.metadata["content_type_mismatch"] is True
            assert transport.get_calls == [MySource.STATIONS["peninsular"]]
        finally:
            await raw.aclose()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_my_discovery_rejects_missing_last_modified_instead_of_using_now(tmp_path):
    transport = ResponseReplayTransport(
        {
            MySource.STATIONS["peninsular"]: ({"content-type": "image/gif"}, _gif()),
            MySource.STATIONS["east"]: ({"content-type": "image/gif"}, _gif()),
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "my",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(TransportError, match="Last-Modified"):
            await MySource(registry.get_info("my")).discover(Query("my", latest=True), context)
        assert transport.get_calls == []
    finally:
        context.close()


@pytest.mark.asyncio
async def test_my_download_rejects_unexpected_payload_format(tmp_path):
    transport = ResponseReplayTransport(
        {
            MySource.STATIONS["peninsular"]: (
                {"content-type": "image/gif", "last-modified": "Mon, 29 Dec 2025 06:59:01 GMT"},
                _gif(),
            ),
            MySource.STATIONS["east"]: (
                {"content-type": "image/gif", "last-modified": "Mon, 29 Dec 2025 07:01:01 GMT"},
                _gif(),
            ),
        }
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "my",
        transport=transport,
        temp_root=tmp_path,
    )
    source = MySource(registry.get_info("my"))
    try:
        ref = (await source.discover(Query("my", latest=True), context))[0]
        with pytest.raises(IntegrityError, match="payload format"):
            await source.download(ref, context)
    finally:
        context.close()
    assert not tmp_path.exists()


@pytest.mark.asyncio
async def test_my_rejects_historical_queries_before_head_requests(tmp_path):
    transport = ResponseReplayTransport({})
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "my",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(UnsupportedQueryError, match="only supports latest frames"):
            await MySource(registry.get_info("my")).discover(
                Query("my", at=datetime(2025, 12, 29, 6, 50, tzinfo=timezone.utc)),
                context,
            )
        assert transport.head_calls == []
    finally:
        context.close()
