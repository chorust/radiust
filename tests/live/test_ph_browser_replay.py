"""Opt-in local Chromium replay for PAGASA discovery and image acquisition.

This local service uses generated PNG bytes, not a provider scientific sample.
Run: RADIUST_TEST_ALLOW_LIVE=1 RADIUST_TEST_REAL_BROWSER=1 uv run --extra playwright pytest -q
     tests/live/test_ph_browser_replay.py
"""

from __future__ import annotations

import asyncio
import hashlib
import io
import os
import threading
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.models import FrameRef
from radiust.sources.browser import BrowserAcquirer

pytestmark = pytest.mark.live


def _png() -> bytes:
    buffer = io.BytesIO()
    Image.new("RGBA", (3, 2), (10, 20, 30, 255)).save(buffer, format="PNG")
    return buffer.getvalue()


@pytest.mark.asyncio
async def test_real_browser_reuses_provider_session_for_csrf_image(tmp_path):
    if os.environ.get("RADIUST_TEST_REAL_BROWSER") != "1":
        pytest.skip("set RADIUST_TEST_REAL_BROWSER=1 for local real-Chromium replay")

    payload = _png()
    accesses: list[tuple[str, str, str]] = []

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            accesses.append((self.path, self.headers.get("Cookie", ""), self.headers.get("X-CSRF-TOKEN", "")))
            if self.path == "/radar-page":
                data = b'<html><head><meta name="csrf-token" content="browser-replay-csrf"></head></html>'
                self.send_response(200)
                self.send_header("Content-Type", "text/html")
                self.send_header("Set-Cookie", "radar-session=browser-replay; Path=/; HttpOnly")
            elif self.path == "/radar.png" and "radar-session=browser-replay" in self.headers.get("Cookie", "") and self.headers.get("X-CSRF-TOKEN") == "browser-replay-csrf":
                data = payload
                self.send_response(200)
                self.send_header("Content-Type", "image/png")
            elif self.path == "/timeline" and "radar-session=browser-replay" in self.headers.get("Cookie", "") and self.headers.get("X-CSRF-TOKEN") == "browser-replay-csrf":
                data = b'{"data":{"timeline":[]}}'
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
            else:
                data = b"missing session or csrf token"
                self.send_response(403)
                self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, _format, *_args):
            return

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    root = f"http://127.0.0.1:{server.server_address[1]}"
    ref = FrameRef(
        source="ph", product="composite", station="PHCOMP4",
        valid_time=datetime(2026, 9, 18, 4, tzinfo=timezone.utc),
        uri=root + "/radar.png", locator_version="local-browser-replay-v1",
        revision="local-replay",
    )
    context = SourceContext(
        load_config({"runtime": {"allow_network": False}}, environ={}),
        "ph", temp_root=tmp_path,
    )
    try:
        timeline = await asyncio.wait_for(
            BrowserAcquirer().fetch_page(
                root + "/timeline", context, warmup_url=root + "/radar-page",
                csrf_selector='meta[name="csrf-token"]',
                headers={"Referer": root + "/radar-page"},
            ),
            timeout=40,
        )
        assert timeline == b'{"data":{"timeline":[]}}'
        raw = await asyncio.wait_for(
            BrowserAcquirer().acquire(
                ref, context, warmup_url=root + "/radar-page",
                csrf_selector='meta[name="csrf-token"]',
                headers={"Referer": root + "/radar-page"},
            ),
            timeout=40,
        )
        try:
            assert raw.bytes() == payload
            assert raw.artifacts[0].sha256 == hashlib.sha256(payload).hexdigest()
            assert raw.metadata["acquisition"] == "playwright"
            assert any(path == "/timeline" and "radar-session=browser-replay" in cookie and csrf == "browser-replay-csrf" for path, cookie, csrf in accesses)
            assert any(path == "/radar.png" and "radar-session=browser-replay" in cookie and csrf == "browser-replay-csrf" for path, cookie, csrf in accesses)
        finally:
            await raw.aclose()
    finally:
        context.close()
        server.shutdown()
        server.server_close()
        worker.join(timeout=5)
