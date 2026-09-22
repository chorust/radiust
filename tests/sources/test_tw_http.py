from __future__ import annotations

import threading
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.tw_http import TwHttpSource
from radiust.transport import HTTPTransport

JS_URL = "https://www.cwa.gov.tw/Data/js/obs_img/Observe_radar.js"
IMAGE_URL = "https://www.cwa.gov.tw/Data/radar/CV1_3600_202609181050.png"


class ReplayTransport:
    def __init__(self, responses: dict[str, bytes]):
        self.responses = responses
        self.get_calls: list[tuple[str, dict[str, str] | None]] = []

    async def get(self, url: str, *, headers=None) -> bytes:
        self.get_calls.append((url, headers))
        return self.responses[url]


@pytest.mark.asyncio
async def test_tw_http_discovers_cv1_3600_and_preserves_raw(tmp_path):
    image = (Path(__file__).parents[1] / "fixtures/sources/tw-http/raw/CV1_3600.png").read_bytes()
    js = b'''0:{"img":'CV1_3600_202609181050.png', 'text':'2026/09/18 10:50'}'''
    transport = ReplayTransport({JS_URL: js, IMAGE_URL: image})
    context = SourceContext(load_config({"runtime": {"allow_network": True}}, environ={}), "tw-http", transport=transport, temp_root=tmp_path)
    source = TwHttpSource(registry.get_info("tw-http"))
    try:
        ref = (await source.discover(Query("tw-http", latest=True), context))[0]
        assert ref.station == "CV1_3600"
        assert ref.valid_time == datetime(2026, 9, 18, 2, 50, tzinfo=timezone.utc)
        assert ref.uri == IMAGE_URL
        raw_frame = await source.download(ref, context)
        try:
            assert raw_frame.artifacts[0].sha256 == "dd314e2cc79fbe9272162ef94972714310be31eb5eaf584ae2a80d4b839e95f7"
            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw_frame, context)
        finally:
            raw_frame.close()
        assert [url for url, _headers in transport.get_calls] == [JS_URL, IMAGE_URL]
        assert not any(
            "br" in value.lower()
            for _url, headers in transport.get_calls
            for key, value in (headers or {}).items()
            if key.lower() == "accept-encoding"
        )
    finally:
        context.close()


@pytest.mark.asyncio
async def test_tw_http_production_transport_does_not_advertise_brotli(tmp_path, monkeypatch):
    image = (Path(__file__).parents[1] / "fixtures/sources/tw-http/raw/CV1_3600.png").read_bytes()
    js = b'''0:{"img":'CV1_3600_202609181050.png', 'text':'2026/09/18 10:50'}'''
    responses = {"/Observe_radar.js": js, "/radar/CV1_3600_202609181050.png": image}
    observed_headers: list[tuple[str, dict[str, str]]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            payload = responses[self.path]
            observed_headers.append((self.path, {key.lower(): value for key, value in self.headers.items()}))
            self.send_response(200)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, _format, *_args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base_url = f"http://127.0.0.1:{server.server_port}"
    monkeypatch.setattr(TwHttpSource, "JS_URL", f"{base_url}/Observe_radar.js")
    monkeypatch.setattr(TwHttpSource, "IMAGE_BASE", f"{base_url}/radar/")
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "tw-http",
        transport=HTTPTransport(allow_network=True),
        temp_root=tmp_path,
    )
    source = TwHttpSource(registry.get_info("tw-http"))
    try:
        refs = await source.discover(Query("tw-http", latest=True), context)
        raw = await source.download(refs[0], context)
        raw.close()
    finally:
        context.close()
        server.shutdown()
        thread.join(timeout=2)
        server.server_close()

    assert [path for path, _headers in observed_headers] == [
        "/Observe_radar.js",
        "/radar/CV1_3600_202609181050.png",
    ]
    assert all("br" not in headers.get("accept-encoding", "").lower() for _, headers in observed_headers)
