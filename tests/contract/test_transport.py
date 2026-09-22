from __future__ import annotations

import ssl
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import certifi
import pytest
from radiust.transport import HTTPTransport


@pytest.mark.asyncio
async def test_head_response_exposes_headers_without_fetching_image_body():
    class Handler(BaseHTTPRequestHandler):
        def do_HEAD(self):
            self.send_response(200)
            self.send_header("Content-Type", "image/gif")
            self.send_header("Last-Modified", "Mon, 29 Dec 2025 06:59:01 GMT")
            self.send_header("Content-Length", "128")
            self.end_headers()

        def log_message(self, _format, *_args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        response = await HTTPTransport(allow_network=False, max_bytes=16, max_attempts=1).head_response(
            f"http://127.0.0.1:{server.server_port}/radar.gif"
        )
    finally:
        server.shutdown()
        thread.join(timeout=2)
        server.server_close()

    assert response.status == 200
    assert response.body == b""
    assert response.headers["content-type"] == "image/gif"
    assert response.headers["last-modified"] == "Mon, 29 Dec 2025 06:59:01 GMT"


def test_tls_context_adds_certifi_roots_without_disabling_verification(monkeypatch):
    class RecordingContext:
        verify_mode = ssl.CERT_REQUIRED
        check_hostname = True
        verify_flags = 0

        def load_verify_locations(self, *, cafile):
            self.cafile = cafile

    context = RecordingContext()
    bundle = "/test/certifi/cacert.pem"
    monkeypatch.setattr(ssl, "create_default_context", lambda: context)
    monkeypatch.setattr(certifi, "where", lambda: bundle)

    result = HTTPTransport()._tls_context()

    assert result is context
    assert context.cafile == bundle
    assert context.verify_mode == ssl.CERT_REQUIRED
    assert context.check_hostname is True
