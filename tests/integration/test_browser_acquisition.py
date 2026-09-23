from __future__ import annotations

import asyncio
import json
from dataclasses import replace
from datetime import datetime, timezone

import pytest
from radiust.config import load_config
from radiust.context import Cancellation, SourceContext
from radiust.errors import (
    AuthenticationError,
    MissingDependencyError,
    ResourceLimitError,
    TransportError,
)
from radiust.models import FrameRef
from radiust.raw import raw_manifest
from radiust.sources.browser import BrowserAcquirer, acquire_with_http_fallback


def _ref() -> FrameRef:
    return FrameRef(
        source="ph",
        product="composite",
        station="PHCOMP4",
        valid_time=datetime(2026, 9, 18, 4, 0, tzinfo=timezone.utc),
        uri="https://example.invalid/artifact.bin",
        locator={},
        locator_version="browser-test-v1",
        revision="browser-test",
    )


class _Response:
    def __init__(self, body: bytes, status: int = 200):
        self.body_bytes = body
        self.status = status

    async def body(self) -> bytes:
        return self.body_bytes


class _Stream:
    def __init__(self, body: bytes):
        self.body = body

    async def read(self, _size: int) -> bytes:
        body, self.body = self.body, b""
        return body


class _Download:
    suggested_filename = "artifact.bin"

    def __init__(self, body: bytes):
        self.body = body

    async def create_read_stream(self):
        return _Stream(self.body)


class _DownloadInfo:
    def __init__(self, body: bytes):
        self._download = _Download(body)
        self.value = self._value()

    async def _value(self):
        return self._download

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_args):
        return None


class _Locator:
    async def get_attribute(self, name: str):
        assert name == "content"
        return "offline-csrf-token"


class _Page:
    def __init__(self, body: bytes):
        self.body = body
        self.urls: list[str] = []
        self.context = None
        self.headers_by_url: list[dict[str, str]] = []
        self.events = {}

    async def goto(self, _url, **_kwargs):
        self.urls.append(_url)
        self.headers_by_url.append(dict(self.context.headers) if self.context else {})
        return _Response(self.body)

    def locator(self, _selector):
        return _Locator()

    def expect_download(self):
        return _DownloadInfo(self.body)

    def on(self, event, callback):
        self.events[event] = callback

    async def click(self, _selector):
        return None


class _BrowserContext:
    def __init__(self, body: bytes, headers):
        self.page = _Page(body)
        self.page.context = self
        self.headers = dict(headers)
        self.closed = False
        self.routes = {}

    async def new_page(self):
        return self.page

    async def set_extra_http_headers(self, headers):
        self.headers = dict(headers)

    async def route(self, pattern, handler):
        self.routes[pattern] = handler

    async def close(self):
        self.closed = True


class _Browser:
    def __init__(self, body: bytes):
        self.body = body
        self.context = None
        self.closed = False

    async def new_context(self, *, extra_http_headers):
        self.context = _BrowserContext(self.body, extra_http_headers)
        return self.context

    async def close(self):
        self.closed = True


class _Manager:
    def __init__(self, body: bytes):
        self.browser = _Browser(body)

    async def __aenter__(self):
        browser = self.browser

        class Chromium:
            async def launch(self, **_kwargs):
                return browser

        class Playwright:
            chromium = Chromium()

        return Playwright()

    async def __aexit__(self, *_args):
        return None


def _context(tmp_path, **runtime):
    return SourceContext(
        load_config({"runtime": {"allow_network": True, **runtime}}, environ={}),
        "ph",
        temp_root=tmp_path,
    )


@pytest.mark.asyncio
async def test_browser_navigation_csrf_download_and_cleanup(tmp_path):
    manager = _Manager(b"offline-browser-raw")
    raw = await BrowserAcquirer(lambda: manager).acquire(
        _ref(),
        _context(tmp_path),
        page_url="https://example.invalid/index.html",
        trigger_selector="#download",
        headers={"Referer": "https://example.invalid/"},
        csrf_selector='meta[name="csrf-token"]',
    )
    try:
        assert raw.bytes() == b"offline-browser-raw"
        assert manager.browser.context.headers["X-CSRF-TOKEN"] == "offline-csrf-token"
        assert manager.browser.context.closed
        assert manager.browser.closed
    finally:
        raw.close()


@pytest.mark.asyncio
async def test_browser_warmup_sets_csrf_header_before_image_request(tmp_path):
    manager = _Manager(b"offline-image-response")
    raw = await BrowserAcquirer(lambda: manager).acquire(
        _ref(),
        _context(tmp_path),
        warmup_url="https://example.invalid/radar-page",
        csrf_selector='meta[name="csrf-token"]',
        headers={"Referer": "https://example.invalid/radar-page"},
    )
    try:
        page = manager.browser.context.page
        assert page.urls == ["https://example.invalid/radar-page", _ref().uri]
        assert page.headers_by_url[0] == {"Referer": "https://example.invalid/radar-page"}
        assert page.headers_by_url[1]["X-CSRF-TOKEN"] == "offline-csrf-token"
        assert raw.bytes() == b"offline-image-response"
        assert manager.browser.context.closed
        assert manager.browser.closed
    finally:
        raw.close()


@pytest.mark.asyncio
async def test_browser_raw_manifest_excludes_signed_target_url(tmp_path):
    signed_url = "https://example.invalid/artifact.png?token=signed-image-secret"
    ref = replace(_ref(), uri=signed_url)
    manager = _Manager(b"offline-image-response")
    raw = await BrowserAcquirer(lambda: manager).acquire(ref, _context(tmp_path))
    try:
        manifest = raw_manifest(raw)
        assert "page_url" not in manifest["metadata"]
        assert raw.artifacts[0].name == "artifact.png"
        assert "signed-image-secret" not in json.dumps(manifest)
    finally:
        raw.close()


@pytest.mark.asyncio
async def test_browser_can_replay_timeline_as_transient_bytes_in_one_session(tmp_path):
    manager = _Manager(b'{"data":{"timeline":[]}}')
    context = _context(tmp_path)
    payload = await BrowserAcquirer(lambda: manager).fetch_page(
        "https://example.invalid/api/radar/timeline",
        context,
        warmup_url="https://example.invalid/radar-page",
        csrf_selector='meta[name="csrf-token"]',
        headers={"Referer": "https://example.invalid/radar-page"},
    )
    assert payload == b'{"data":{"timeline":[]}}'
    page = manager.browser.context.page
    assert page.urls == ["https://example.invalid/radar-page", "https://example.invalid/api/radar/timeline"]
    assert page.headers_by_url[1]["X-CSRF-TOKEN"] == "offline-csrf-token"
    assert manager.browser.context.closed
    assert manager.browser.closed
    assert list(tmp_path.iterdir()) == []  # Discovery is not a scientific RawFrame.


@pytest.mark.asyncio
async def test_browser_route_limits_active_requests_per_host(tmp_path):
    context = _context(tmp_path, request_concurrency=3, host_concurrency=1)
    browser_context = _BrowserContext(b"response", {})
    page = browser_context.page
    await BrowserAcquirer._limit_page_requests(browser_context, page, context)
    route_request = browser_context.routes["**/*"]
    continued = []

    class Request:
        def __init__(self, url):
            self.url = url

    class Route:
        def __init__(self, request):
            self.request = request

        async def continue_(self):
            continued.append(self.request)

        async def abort(self):
            raise AssertionError("unexpected request abort")

    requests = [
        Request("https://same.example/one"),
        Request("https://same.example/two"),
        Request("https://other.example/three"),
    ]
    tasks = [asyncio.create_task(route_request(Route(request))) for request in requests]
    await asyncio.sleep(0.02)

    assert len(continued) == 2
    assert {request.url.split("/")[2] for request in continued} == {
        "same.example", "other.example",
    }

    first_same_host = next(request for request in continued if "same.example" in request.url)
    page.events["requestfinished"](first_same_host)
    await asyncio.sleep(0.02)
    assert len(continued) == 3

    for request in tuple(continued):
        page.events["requestfinished"](request)
    await asyncio.gather(*tasks)
    context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["fetch_page", "acquire"])
async def test_browser_resources_close_before_playwright_manager_stops(tmp_path, operation):
    class LifecycleManager(_Manager):
        def __init__(self, body):
            super().__init__(body)
            self.active = False
            self.events = []

        async def __aenter__(self):
            self.active = True
            return await super().__aenter__()

        async def __aexit__(self, *_args):
            self.events.append("manager")
            self.active = False

    manager = LifecycleManager(b"browser-response")
    close_browser = manager.browser.close

    async def tracked_close_browser():
        assert manager.active, "the Playwright manager must still be active"
        assert manager.browser.context.closed, "browser context must close first"
        manager.events.append("browser")
        await close_browser()

    manager.browser.close = tracked_close_browser
    new_context = manager.browser.new_context

    async def tracked_context(*, extra_http_headers):
        browser_context = await new_context(extra_http_headers=extra_http_headers)
        close_context = browser_context.close

        async def tracked_close_context():
            assert manager.active, "the Playwright manager must still be active"
            manager.events.append("context")
            await close_context()

        browser_context.close = tracked_close_context
        return browser_context

    manager.browser.new_context = tracked_context
    context = _context(tmp_path)
    acquirer = BrowserAcquirer(lambda: manager)
    try:
        if operation == "fetch_page":
            await acquirer.fetch_page("https://example.invalid/timeline", context)
        else:
            raw = await acquirer.acquire(_ref(), context)
            raw.close()
        assert manager.events == ["context", "browser", "manager"]
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", ["fetch_page", "acquire"])
@pytest.mark.parametrize(("status", "error"), [(403, AuthenticationError), (503, TransportError)])
async def test_browser_rejects_error_status_and_releases_session(tmp_path, operation, status, error):
    manager = _Manager(b"provider returned an error page")
    page = manager.browser.new_context

    async def new_context(*, extra_http_headers):
        result = await page(extra_http_headers=extra_http_headers)
        result.page.goto = lambda *_args, **_kwargs: asyncio.sleep(
            0, result=_Response(b"provider returned an error page", status)
        )
        return result

    manager.browser.new_context = new_context
    acquirer = BrowserAcquirer(lambda: manager)
    context = _context(tmp_path)
    with pytest.raises(error):
        if operation == "fetch_page":
            await acquirer.fetch_page(_ref().uri, context)
        else:
            await acquirer.acquire(_ref(), context)
    assert manager.browser.context.closed
    assert manager.browser.closed
    assert list(tmp_path.iterdir()) == []


@pytest.mark.asyncio
async def test_browser_rejects_failed_warmup_before_loading_image(tmp_path):
    manager = _Manager(b"not authorized")
    new_context = manager.browser.new_context

    async def failed_warmup(*, extra_http_headers):
        result = await new_context(extra_http_headers=extra_http_headers)
        result.page.goto = lambda *_args, **_kwargs: asyncio.sleep(0, result=_Response(b"blocked", 403))
        return result

    manager.browser.new_context = failed_warmup
    with pytest.raises(AuthenticationError):
        await BrowserAcquirer(lambda: manager).acquire(
            _ref(), _context(tmp_path), warmup_url="https://example.invalid/radar-page"
        )
    assert manager.browser.context.page.urls == []
    assert manager.browser.context.closed
    assert manager.browser.closed


@pytest.mark.asyncio
async def test_browser_enforces_shared_size_limit(tmp_path):
    manager = _Manager(b"x" * 32)
    with pytest.raises(ResourceLimitError):
        await BrowserAcquirer(lambda: manager).acquire(
            _ref(), _context(tmp_path, max_artifact_bytes=8)
        )
    assert manager.browser.context.closed
    assert manager.browser.closed


@pytest.mark.asyncio
async def test_browser_cancellation_stops_before_launch(tmp_path):
    manager = _Manager(b"x")
    context = _context(tmp_path)
    context.cancellation = Cancellation()
    context.cancellation.cancel()
    with pytest.raises(asyncio.CancelledError):
        await BrowserAcquirer(lambda: manager).acquire(_ref(), context)
    assert manager.browser.context is None


@pytest.mark.asyncio
async def test_browser_cancellation_during_page_warmup_closes_browser(tmp_path):
    manager = _Manager(b"image")
    context = _context(tmp_path)
    create_context = manager.browser.new_context

    async def new_context(*, extra_http_headers):
        browser_context = await create_context(extra_http_headers=extra_http_headers)

        async def cancel_after_warmup(url, **_kwargs):
            assert url == "https://example.invalid/radar-page"
            context.cancellation.cancel()
            return _Response(b"warmup")

        browser_context.page.goto = cancel_after_warmup
        return browser_context

    manager.browser.new_context = new_context
    with pytest.raises(asyncio.CancelledError):
        await BrowserAcquirer(lambda: manager).acquire(
            _ref(), context, warmup_url="https://example.invalid/radar-page"
        )
    assert manager.browser.context.closed
    assert manager.browser.closed
    assert not list(tmp_path.glob("*.bin"))


@pytest.mark.asyncio
async def test_http_transport_failure_can_fall_back_to_browser():
    async def http():
        raise TransportError("http failed")

    sentinel = object()

    async def browser():
        return sentinel

    assert await acquire_with_http_fallback(http, browser) is sentinel


def test_browser_dependency_is_lazy_and_catalog_still_lists_sources(monkeypatch):
    from radiust.registry import sources

    assert any(info.id == "ph" for info in sources())
    acquirer = BrowserAcquirer()
    monkeypatch.setattr(
        acquirer,
        "_playwright_factory",
        lambda: (_ for _ in ()).throw(MissingDependencyError("missing playwright")),
    )
    with pytest.raises(MissingDependencyError):
        acquirer._playwright_factory()
