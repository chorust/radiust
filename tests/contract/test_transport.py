from __future__ import annotations

import asyncio
import ssl
import threading
import time
import urllib.request
from urllib.parse import urlparse

import certifi
import pytest
from radiust.errors import TransportError
from radiust.transport import HTTPResponse, HTTPTransport, _CheckedRedirectHandler


@pytest.mark.asyncio
async def test_head_response_exposes_headers_without_fetching_image_body(monkeypatch):
    transport = HTTPTransport(allow_network=False, max_bytes=16, max_attempts=1)
    opened = {}

    class FakeResponse:
        status = 200
        headers = {
            "Content-Type": "image/gif",
            "Last-Modified": "Mon, 29 Dec 2025 06:59:01 GMT",
            "Content-Length": "128",
        }

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            pass

        def geturl(self):
            return "http://127.0.0.1/radar.gif"

        def read(self, _size):
            return b""

    class FakeOpener:
        def open(self, request, timeout):
            opened["method"] = request.get_method()
            opened["timeout"] = timeout
            return FakeResponse()

    monkeypatch.setattr("radiust.transport.urllib.request.build_opener", lambda *_args: FakeOpener())
    response = await transport.head_response("http://127.0.0.1/radar.gif")

    assert response.status == 200
    assert response.body == b""
    assert response.headers["content-type"] == "image/gif"
    assert response.headers["last-modified"] == "Mon, 29 Dec 2025 06:59:01 GMT"
    assert opened == {"method": "HEAD", "timeout": 30.0}


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


@pytest.mark.asyncio
async def test_transport_caps_global_and_per_host_request_peaks(monkeypatch):
    transport = HTTPTransport(allow_network=True, max_attempts=1, request_concurrency=3, host_concurrency=2)
    lock = threading.Lock()
    active = 0
    peak_global = 0
    active_by_host = {}
    peak_by_host = {}

    def fake_response(request):
        nonlocal active, peak_global
        host = urlparse(request.full_url).hostname
        with lock:
            active += 1
            active_by_host[host] = active_by_host.get(host, 0) + 1
            peak_global = max(peak_global, active)
            peak_by_host[host] = max(peak_by_host.get(host, 0), active_by_host[host])
        time.sleep(0.04)
        with lock:
            active -= 1
            active_by_host[host] -= 1
        return HTTPResponse(b"ok", 200, request.full_url, {})

    monkeypatch.setattr(transport, "_request_once_response", fake_response)
    urls = [f"http://host-{index % 2}.example.test/{index}" for index in range(12)]
    results = await asyncio.gather(*(transport.get(url) for url in urls))

    assert results == [b"ok"] * len(urls)
    assert peak_global == 3
    assert max(peak_by_host.values()) == 2
    assert active == 0


def test_redirect_checks_policy_then_moves_and_releases_host_lease():
    transport = HTTPTransport(allow_network=True, host_concurrency=1)
    old_slot = transport._host_slot("old.example")
    assert old_slot.acquire(blocking=False)
    transport._host_local.current = ("old.example", old_slot)
    handler = _CheckedRedirectHandler(transport._check, transport._redirect_host)
    try:
        redirected = handler.redirect_request(
            urllib.request.Request("http://old.example/path"), None, 302, "Found", {},
            "http://new.example/path",
        )
        assert redirected is not None
        assert redirected.full_url == "http://new.example/path"
        assert transport._host_local.current[0] == "new.example"
        transport._release_host_lease()
        assert old_slot.acquire(blocking=False)
        old_slot.release()
    finally:
        transport._release_host_lease()

    restricted = HTTPTransport()
    old_slot = restricted._host_slot("127.0.0.1")
    assert old_slot.acquire(blocking=False)
    restricted._host_local.current = ("127.0.0.1", old_slot)
    called = []
    denied = _CheckedRedirectHandler(restricted._check, lambda url: called.append(url))
    try:
        with pytest.raises(TransportError, match="public network"):
            denied.redirect_request(
                urllib.request.Request("http://127.0.0.1/path"), None, 302, "Found", {},
                "https://public.example/path",
            )
        assert called == []
        assert restricted._host_local.current[0] == "127.0.0.1"
    finally:
        restricted._release_host_lease()


def test_transport_rejects_oversized_response_before_reading_body(monkeypatch):
    state = {"reads": 0}

    class FakeResponse:
        status = 200
        headers = {"Content-Length": "4"}

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            pass

        def geturl(self):
            return "https://provider.example/data"

        def read(self, _size):
            state["reads"] += 1
            return b"data"

    class FakeOpener:
        def open(self, _request, timeout):
            return FakeResponse()

    monkeypatch.setattr("radiust.transport.urllib.request.build_opener", lambda *_args: FakeOpener())
    transport = HTTPTransport(allow_network=True, max_bytes=3, max_attempts=1)

    with pytest.raises(TransportError, match="response exceeds configured byte limit"):
        transport.get_sync("https://provider.example/data")

    assert state["reads"] == 0


@pytest.mark.parametrize(("request_concurrency", "host_concurrency"), [(0, 1), (1, 0), (-1, 1)])
def test_transport_rejects_non_positive_concurrency_limits(request_concurrency, host_concurrency):
    with pytest.raises(ValueError, match="transport limits"):
        HTTPTransport(request_concurrency=request_concurrency, host_concurrency=host_concurrency)
