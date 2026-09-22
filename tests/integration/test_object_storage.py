from __future__ import annotations

import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from radiust.errors import ResourceLimitError, StorageError
from radiust.storage.object import ObjectStore, parse_object_uri


class _PayloadHandler(BaseHTTPRequestHandler):
    payload = b"object-payload"

    def do_GET(self) -> None:  # noqa: N802
        if self.path != "/bucket/data.bin":
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.payload)))
        self.send_header("ETag", '"not-a-sha256"')
        self.end_headers()
        self.wfile.write(self.payload)

    def log_message(self, *_args: object) -> None:
        return


@pytest.fixture
def object_server() -> str:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _PayloadHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        thread.join(timeout=2)


def test_parse_s3_and_oss_uris_without_exposing_credentials() -> None:
    s3 = parse_object_uri("s3://bucket/path/to/data.bin", provider="s3", endpoint="https://objects.test")
    oss = parse_object_uri("oss://bucket/data.bin", provider="oss", endpoint="https://oss.test")

    assert s3.provider == "s3"
    assert s3.bucket == "bucket"
    assert s3.key == "path/to/data.bin"
    assert s3.url == "https://objects.test/bucket/path/to/data.bin"
    assert oss.url == "https://oss.test/bucket/data.bin"
    assert "secret" not in repr(s3).lower()


def test_parse_object_uri_rejects_a_provider_scheme_mismatch() -> None:
    with pytest.raises(StorageError, match="does not match"):
        parse_object_uri("s3://bucket/path/to/data.bin", provider="oss")


@pytest.mark.parametrize(
    ("uri", "region", "expected"),
    [
        ("s3://example-bucket/path/data.bin", None, "https://example-bucket.s3.amazonaws.com/path/data.bin"),
        ("oss://example-bucket/path/data.bin", None, "https://example-bucket.oss.aliyuncs.com/path/data.bin"),
        ("oss://example-bucket/path/data.bin", "cn-hangzhou", "https://example-bucket.oss-cn-hangzhou.aliyuncs.com/path/data.bin"),
    ],
)
def test_default_object_endpoints_do_not_repeat_bucket(uri, region, expected):
    assert parse_object_uri(uri, region=region).url == expected


@pytest.mark.parametrize(
    "uri,endpoint,expected",
    [
        ("s3://example-bucket/path/data.bin", "https://example-bucket.s3.amazonaws.com", "https://example-bucket.s3.amazonaws.com/path/data.bin"),
        ("oss://example-bucket/path/data.bin", "https://example-bucket.oss-cn-hangzhou.aliyuncs.com", "https://example-bucket.oss-cn-hangzhou.aliyuncs.com/path/data.bin"),
    ],
)
def test_explicit_virtual_hosted_endpoints_do_not_repeat_bucket(uri, endpoint, expected):
    assert parse_object_uri(uri, endpoint=endpoint).url == expected


@pytest.mark.asyncio
async def test_anonymous_object_read_hashes_bytes_and_does_not_use_etag(object_server: str) -> None:
    store = ObjectStore(f"{object_server}/bucket/data.bin")

    payload, receipt = await store.read(max_bytes=64)

    assert payload == b"object-payload"
    assert receipt.size_bytes == len(payload)
    assert receipt.sha256 == "dba193685c0b66bfac0194d1fd64731c762d60e6aa284b11b2eab9bbaaa0d2b8"
    assert receipt.etag is None


@pytest.mark.asyncio
async def test_object_read_enforces_limit_before_returning(object_server: str) -> None:
    store = ObjectStore(f"{object_server}/bucket/data.bin")

    with pytest.raises(ResourceLimitError, match="configured limit"):
        await store.read(max_bytes=1)
