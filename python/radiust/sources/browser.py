"""Bounded Playwright acquisition used by browser-backed sources."""

from __future__ import annotations

import asyncio
import hashlib
import mimetypes
from collections.abc import Awaitable, Callable, Mapping
from contextlib import AsyncExitStack, suppress
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlsplit

from ..context import SourceContext
from ..errors import AuthenticationError, MissingDependencyError, TransportError
from ..models import Artifact, FrameRef
from ..raw import RawFrame


def _default_playwright_factory() -> Any:
    try:
        from playwright.async_api import async_playwright
    except ImportError as exc:
        raise MissingDependencyError(
            "browser acquisition requires the 'playwright' extra"
        ) from exc
    return async_playwright()


def _check_browser_response(response: Any, *, stage: str) -> None:
    """Reject upstream HTTP errors without exposing a signed/tokenized URL."""
    if response is None:
        raise TransportError(f"browser {stage} returned no response")
    status = getattr(response, "status", 200)
    if status in (401, 403):
        raise AuthenticationError(f"browser {stage} refused the request (HTTP {status})")
    if not 200 <= status < 300:
        raise TransportError(f"browser {stage} failed (HTTP {status})")


class BrowserAcquirer:
    def __init__(self, playwright_factory: Callable[[], Any] | None = None) -> None:
        self._playwright_factory = playwright_factory or _default_playwright_factory

    @staticmethod
    async def _limit_page_requests(browser_context: Any, page: Any, context: SourceContext) -> None:
        """Keep browser-owned requests within the same local concurrency budget."""
        request_slots = asyncio.Semaphore(max(1, int(context.limits["request_concurrency"])))
        host_limit = max(1, int(context.limits["host_concurrency"]))
        host_slots: dict[str, asyncio.Semaphore] = {}
        leases: dict[int, tuple[asyncio.Semaphore, asyncio.Semaphore]] = {}

        def release(request: Any) -> None:
            lease = leases.pop(id(request), None)
            if lease is not None:
                lease[1].release()
                lease[0].release()

        async def route_request(route: Any) -> None:
            request = route.request
            host = (urlsplit(request.url).hostname or "_no_host").lower()
            host_slot = host_slots.setdefault(host, asyncio.Semaphore(host_limit))
            request_acquired = False
            host_acquired = False
            try:
                await request_slots.acquire()
                request_acquired = True
                await host_slot.acquire()
                host_acquired = True
                context.cancellation.check()
                leases[id(request)] = (request_slots, host_slot)
                request_acquired = False
                host_acquired = False
                await route.continue_()
            except BaseException:
                if id(request) in leases:
                    release(request)
                else:
                    if host_acquired:
                        host_slot.release()
                    if request_acquired:
                        request_slots.release()
                with suppress(Exception, asyncio.CancelledError):
                    await route.abort()
                raise

        page.on("requestfinished", release)
        page.on("requestfailed", release)
        await browser_context.route("**/*", route_request)

    @staticmethod
    async def _prepare_page(
        page: Any,
        browser_context: Any,
        context: SourceContext,
        *,
        warmup_url: str | None,
        headers: Mapping[str, str] | None,
        csrf_selector: str | None,
        csrf_header: str,
    ) -> None:
        if warmup_url:
            response = await page.goto(warmup_url, wait_until="domcontentloaded")
            context.cancellation.check()
            _check_browser_response(response, stage="warmup")
            if csrf_selector:
                token = await page.locator(csrf_selector).get_attribute("content")
                if token:
                    await browser_context.set_extra_http_headers(
                        {**dict(headers or {}), csrf_header: token}
                    )

    async def fetch_page(
        self,
        url: str,
        context: SourceContext,
        *,
        warmup_url: str | None = None,
        headers: Mapping[str, str] | None = None,
        csrf_selector: str | None = None,
        csrf_header: str = "X-CSRF-TOKEN",
    ) -> bytes:
        """Read an API response through a fresh bounded browser session.

        Discovery responses remain transient bytes: they are not assigned a
        synthetic observation time or persisted as user-facing RawFrames.
        """
        context.cancellation.check()
        manager = self._playwright_factory()
        async with AsyncExitStack() as stack:
            playwright = await stack.enter_async_context(manager)
            context.cancellation.check()
            browser = await playwright.chromium.launch(headless=True)
            stack.push_async_callback(browser.close)
            browser_context = await browser.new_context(extra_http_headers=dict(headers or {}))
            stack.push_async_callback(browser_context.close)
            page = await browser_context.new_page()
            await self._limit_page_requests(browser_context, page, context)
            await self._prepare_page(
                page, browser_context, context, warmup_url=warmup_url,
                headers=headers, csrf_selector=csrf_selector, csrf_header=csrf_header,
            )
            response = await page.goto(url, wait_until="domcontentloaded")
            context.cancellation.check()
            _check_browser_response(response, stage="discovery")
            data = await response.body()
            context.check_bytes(len(data))
            context.check_temp_bytes(len(data))
            context.cancellation.check()
            return data

    async def acquire(
        self,
        ref: FrameRef,
        context: SourceContext,
        *,
        page_url: str | None = None,
        warmup_url: str | None = None,
        trigger_selector: str | None = None,
        artifact_name: str | None = None,
        headers: Mapping[str, str] | None = None,
        csrf_selector: str | None = None,
        csrf_header: str = "X-CSRF-TOKEN",
    ) -> RawFrame:
        context.cancellation.check()
        root = context.temp_root
        if root is None:
            raise RuntimeError("source context has no temporary root")

        manager = self._playwright_factory()
        async with AsyncExitStack() as stack:
            playwright = await stack.enter_async_context(manager)
            context.cancellation.check()
            browser = await playwright.chromium.launch(headless=True)
            stack.push_async_callback(browser.close)
            browser_context = await browser.new_context(
                extra_http_headers=dict(headers or {})
            )
            stack.push_async_callback(browser_context.close)
            page = await browser_context.new_page()
            await self._limit_page_requests(browser_context, page, context)
            target = page_url or ref.uri
            await self._prepare_page(
                page, browser_context, context, warmup_url=warmup_url,
                headers=headers, csrf_selector=csrf_selector, csrf_header=csrf_header,
            )

            response = await page.goto(target, wait_until="domcontentloaded")
            context.cancellation.check()
            _check_browser_response(response, stage="navigation")

            if csrf_selector and not warmup_url:
                token = await page.locator(csrf_selector).get_attribute("content")
                if token:
                    await browser_context.set_extra_http_headers(
                        {**dict(headers or {}), csrf_header: token}
                    )

            if trigger_selector:
                async with page.expect_download() as download_info:
                    await page.click(trigger_selector)
                download = await download_info.value
                stream = await download.create_read_stream()
                if stream is None:
                    raise TransportError(
                        "browser download did not provide a readable stream"
                    )
                chunks: list[bytes] = []
                total = 0
                while True:
                    context.cancellation.check()
                    chunk = await stream.read(64 * 1024)
                    if not chunk:
                        break
                    total += len(chunk)
                    context.check_bytes(total)
                    context.check_temp_bytes(total)
                    chunks.append(chunk)
                data = b"".join(chunks)
                name = (
                    artifact_name
                    or getattr(download, "suggested_filename", None)
                    or ref.uri
                    or "browser-download.bin"
                )
            else:
                data = await response.body()
                context.check_bytes(len(data))
                context.check_temp_bytes(len(data))
                name = artifact_name or ref.uri

            context.cancellation.check()
            # URL query/fragment values may contain provider credentials or
            # signed parameters; keep them out of artifact names and manifests.
            name = unquote(urlsplit(name).path).replace("\\", "/").rsplit("/", 1)[-1]
            name = Path(name).name or "browser-response.bin"
            target_path = root / name
            target_path.write_bytes(data)
            artifact = Artifact(
                name=name,
                role="data",
                media_type=mimetypes.guess_type(name)[0]
                or "application/octet-stream",
                payload=target_path,
                source_revision=ref.revision,
                size_bytes=len(data),
                sha256=hashlib.sha256(data).hexdigest(),
            )
            context.check_artifacts((artifact,))
            return RawFrame(
                ref,
                (artifact,),
                metadata={
                    "acquisition": "playwright",
                    "csrf_used": bool(csrf_selector),
                },
            )


async def acquire_with_http_fallback(
    http_acquire: Callable[[], Awaitable[RawFrame]],
    browser_acquire: Callable[[], Awaitable[RawFrame]],
) -> RawFrame:
    try:
        return await http_acquire()
    except TransportError:
        return await browser_acquire()


__all__ = ["BrowserAcquirer", "acquire_with_http_fallback"]
