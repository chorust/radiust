"""PAGASA's browser fallback must preserve raw data and cancel promptly."""

from __future__ import annotations

import asyncio
import hashlib
import io
import json
from dataclasses import replace
from datetime import datetime, timezone

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import AuthenticationError, IntegrityError, TransportError
from radiust.identity import artifact_bytes
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.ph import PhSource


def _image() -> bytes:
    out = io.BytesIO()
    Image.new("RGBA", (2, 2), (0, 0, 0, 255)).save(out, format="PNG")
    return out.getvalue()


class _ReplayTransport:
    def __init__(self, *, failure: Exception | None = None):
        self.failure = failure
        self.urls: list[str] = []

    async def get(self, url: str, *, headers=None):
        self.urls.append(url)
        if url == PhSource.BASE_URL:
            return b'<meta name="csrf-token" content="csrf-replay">'
        if url.startswith(PhSource.TIMELINE_URL):
            assert headers["X-CSRF-TOKEN"] == "csrf-replay"
            return json.dumps({"data": {"timeline": [{
                "observed_at": "2026-09-18 12:00:00",
                "image_url": "https://www.panahon.gov.ph/radar/202609180400.png",
            }]}}).encode()
        if self.failure is not None:
            raise self.failure
        return _image()


class _Browser:
    def __init__(self, data: bytes):
        self.data = data
        self.calls: list[tuple] = []
        self.page_calls: list[tuple] = []

    async def fetch_page(self, url, context, **options):
        self.page_calls.append((url, context, options))
        return json.dumps({"data": {"timeline": [{
            "observed_at": "2026-09-18 12:00:00",
            "image_url": "https://www.panahon.gov.ph/radar/202609180400.png",
        }]}}).encode()

    async def acquire(self, ref, context, **options):
        from radiust.models import Artifact
        from radiust.raw import RawFrame

        self.calls.append((ref, context, options))
        target = context.temp_root / "browser.png"
        target.write_bytes(self.data)
        return RawFrame(ref, (Artifact(
            name="browser.png", role="data", media_type="image/png", payload=target,
            size_bytes=len(self.data), sha256=hashlib.sha256(self.data).hexdigest(),
        ),), metadata={"acquisition": "playwright"})


def _context(tmp_path, transport, *, allow_network=True):
    return SourceContext(
        load_config({
            "runtime": {"allow_network": allow_network},
            "sources": {"ph": {"timeline_token": "offline-test-token"}},
        }, environ={}),
        "ph", transport=transport, temp_root=tmp_path,
    )


@pytest.mark.asyncio
async def test_ph_uses_browser_only_for_transport_failure_after_valid_discovery(tmp_path):
    transport = _ReplayTransport(failure=TransportError("offline HTTP failure"))
    browser = _Browser(_image())
    context = _context(tmp_path, transport)
    source = PhSource(registry.get_info("ph"), browser_acquirer=browser)
    try:
        refs = await source.discover(Query("ph", latest=True), context)
        assert len(refs) == 1
        assert refs[0].valid_time == datetime(2026, 9, 18, 4, tzinfo=timezone.utc)
        raw = await source.download(refs[0], context)
        try:
            assert artifact_bytes(raw.artifacts[0]) == _image()
            assert raw.ref == refs[0]
            assert raw.metadata["acquisition"] == "playwright"
            assert browser.calls[0][2]["warmup_url"] == source.BASE_URL
            assert browser.calls[0][2]["csrf_selector"] == 'meta[name="csrf-token"]'
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("failed_stage", ["page", "timeline"])
async def test_ph_browser_discovery_keeps_provider_time_and_session_csrf(tmp_path, failed_stage):
    transport = _ReplayTransport()
    ordinary_get = transport.get

    async def failing_get(url, *, headers=None):
        if (failed_stage == "page" and url == PhSource.BASE_URL) or (
            failed_stage == "timeline" and url.startswith(PhSource.TIMELINE_URL)
        ):
            raise TransportError("provider needs browser session")
        return await ordinary_get(url, headers=headers)

    transport.get = failing_get
    browser = _Browser(_image())
    context = _context(tmp_path, transport)
    source = PhSource(registry.get_info("ph"), browser_acquirer=browser)
    try:
        refs = await source.discover(Query("ph", latest=True), context)
        assert len(refs) == 1
        assert refs[0].valid_time == datetime(2026, 9, 18, 4, tzinfo=timezone.utc)
        assert len(browser.page_calls) == 1
        url, _, options = browser.page_calls[0]
        assert url.startswith(PhSource.TIMELINE_URL)
        assert options["warmup_url"] == PhSource.BASE_URL
        assert options["csrf_selector"] == 'meta[name="csrf-token"]'
    finally:
        context.close()


@pytest.mark.asyncio
async def test_ph_browser_discovery_error_does_not_echo_configured_token(tmp_path):
    transport = _ReplayTransport()
    ordinary_get = transport.get

    async def failing_get(url, *, headers=None):
        if url == PhSource.BASE_URL:
            raise TransportError("HTTP session unavailable")
        return await ordinary_get(url, headers=headers)

    transport.get = failing_get
    browser = _Browser(_image())

    async def url_echoing_fetch(url, context, **options):
        raise RuntimeError(f"browser navigation failed for {url}")

    browser.fetch_page = url_echoing_fetch
    context = _context(tmp_path, transport)
    source = PhSource(registry.get_info("ph"), browser_acquirer=browser)
    try:
        with pytest.raises(Exception) as raised:
            await source.discover(Query("ph", latest=True), context)
        assert "offline-test-token" not in str(raised.value)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_ph_browser_acquisition_error_does_not_echo_signed_image_url(tmp_path):
    transport = _ReplayTransport(failure=TransportError("HTTP artifact request failed"))
    browser = _Browser(_image())

    async def url_echoing_acquire(ref, context, **options):
        raise RuntimeError(f"browser navigation failed for {ref.uri}")

    browser.acquire = url_echoing_acquire
    context = _context(tmp_path, transport)
    source = PhSource(registry.get_info("ph"), browser_acquirer=browser)
    try:
        ref = (await source.discover(Query("ph", latest=True), context))[0]
        ref = replace(ref, uri="https://www.panahon.gov.ph/radar/frame.png?token=signed-image-secret")
        with pytest.raises(TransportError) as raised:
            await source.download(ref, context)
        assert "signed-image-secret" not in str(raised.value)
        assert raised.value.__cause__ is None
    finally:
        context.close()


@pytest.mark.asyncio
async def test_ph_http_discovery_error_does_not_echo_token_when_fallback_is_disabled(tmp_path):
    transport = _ReplayTransport()
    ordinary_get = transport.get

    async def echoing_failure(url, *, headers=None):
        if url == PhSource.BASE_URL:
            return await ordinary_get(url, headers=headers)
        raise RuntimeError(f"request failed for {url}")

    transport.get = echoing_failure
    context = _context(tmp_path, transport, allow_network=False)
    source = PhSource(registry.get_info("ph"), browser_acquirer=_Browser(_image()))
    try:
        with pytest.raises(TransportError) as raised:
            await source.discover(Query("ph", latest=True), context)
        assert "offline-test-token" not in str(raised.value)
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", [AuthenticationError("unauthorized"), IntegrityError("invalid PNG")])
async def test_ph_never_bypasses_auth_or_integrity_errors(tmp_path, failure):
    browser = _Browser(_image())
    context = _context(tmp_path, _ReplayTransport(failure=failure))
    source = PhSource(registry.get_info("ph"), browser_acquirer=browser)
    try:
        ref = (await source.discover(Query("ph", latest=True), context))[0]
        with pytest.raises(type(failure)):
            await source.download(ref, context)
        assert browser.calls == []
    finally:
        context.close()


@pytest.mark.asyncio
async def test_ph_does_not_launch_browser_after_cancel_or_when_network_is_disabled(tmp_path):
    browser = _Browser(_image())
    transport = _ReplayTransport(failure=TransportError("offline HTTP failure"))
    context = _context(tmp_path, transport)
    source = PhSource(registry.get_info("ph"), browser_acquirer=browser)
    try:
        ref = (await source.discover(Query("ph", latest=True), context))[0]
        context.cancellation.cancel()
        with pytest.raises(asyncio.CancelledError):
            await source.download(ref, context)
    finally:
        context.close()
    assert browser.calls == []

    local_context = _context(tmp_path / "offline", transport, allow_network=False)
    try:
        # A previously discovered reference must not bypass the caller's network policy.
        with pytest.raises(TransportError):
            await source.download(ref, local_context)
    finally:
        local_context.close()
    assert browser.calls == []


@pytest.mark.asyncio
async def test_ph_browser_result_requires_a_valid_image(tmp_path):
    context = _context(tmp_path, _ReplayTransport(failure=TransportError("offline HTTP failure")))
    browser = _Browser(b"<html>not an image</html>")
    source = PhSource(registry.get_info("ph"), browser_acquirer=browser)
    try:
        ref = (await source.discover(Query("ph", latest=True), context))[0]
        with pytest.raises(IntegrityError):
            await source.download(ref, context)
    finally:
        context.close()
