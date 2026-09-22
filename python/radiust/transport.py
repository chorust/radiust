"""Small async transport with explicit network opt-in and bounded reads."""

from __future__ import annotations

import asyncio
import ipaddress
import ssl
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Mapping
from dataclasses import dataclass
from dataclasses import field as dc_field
from datetime import datetime, timezone
from email.utils import parsedate_to_datetime
from urllib.parse import urlparse

import certifi

from .errors import AuthenticationError, TransportError


@dataclass(frozen=True)
class HTTPResponse:
    body: bytes
    status: int
    url: str
    headers: Mapping[str, str]


def _is_loopback(host: str | None) -> bool:
    if not host:
        return False
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return host in {"localhost", "ip6-localhost"}


class _CheckedRedirectHandler(urllib.request.HTTPRedirectHandler):
    def __init__(self, check):
        super().__init__()
        self._check = check

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        self._check(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


@dataclass
class HTTPTransport:
    allow_network: bool = False
    max_bytes: int = 512 * 1024 * 1024
    timeout: float = 30.0
    headers: Mapping[str, str] = dc_field(default_factory=dict)
    max_attempts: int = 3
    retry_backoff: float = 0.25
    x509_strict: bool = False

    def __post_init__(self) -> None:
        if self.max_bytes < 0 or self.timeout <= 0 or self.max_attempts < 1 or self.retry_backoff < 0:
            raise ValueError("transport limits must be positive")

    def _check(self, url: str) -> None:
        parsed = urlparse(url)
        if parsed.scheme not in {"http", "https"}:
            raise TransportError(f"unsupported URL scheme: {parsed.scheme}")
        if not self.allow_network and not _is_loopback(parsed.hostname):
            raise TransportError("public network access is disabled; enable it explicitly")

    def get_sync(self, url: str, *, headers: Mapping[str, str] | None = None) -> bytes:
        return self._request_sync(url, method="GET", headers=headers)

    def get_response_sync(self, url: str, *, headers: Mapping[str, str] | None = None) -> HTTPResponse:
        return self._request_response_sync(url, method="GET", headers=headers)

    def head_response_sync(self, url: str, *, headers: Mapping[str, str] | None = None) -> HTTPResponse:
        return self._request_response_sync(url, method="HEAD", headers=headers)

    def post_sync(
        self,
        url: str,
        data: Mapping[str, str] | bytes | None = None,
        *,
        headers: Mapping[str, str] | None = None,
    ) -> bytes:
        if isinstance(data, Mapping):
            body = urllib.parse.urlencode({str(key): str(value) for key, value in data.items()}).encode("utf-8")
        else:
            body = data
        return self._request_sync(url, method="POST", data=body, headers=headers)

    def _request_sync(
        self,
        url: str,
        *,
        method: str,
        data: bytes | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> bytes:
        return self._request_response_sync(url, method=method, data=data, headers=headers).body

    def _request_response_sync(
        self,
        url: str,
        *,
        method: str,
        data: bytes | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> HTTPResponse:
        self._check(url)
        request_headers = dict(self.headers)
        request_headers.update(headers or {})
        request = urllib.request.Request(url, data=data, headers=request_headers, method=method)
        for attempt in range(self.max_attempts):
            try:
                return self._request_once_response(request)
            except urllib.error.HTTPError as exc:
                if exc.code in {401, 403}:
                    raise AuthenticationError("HTTP authentication failed") from exc
                if exc.code not in {408, 425, 429} and exc.code < 500:
                    raise TransportError(f"HTTP request failed with status {exc.code}") from exc
                if attempt + 1 >= self.max_attempts:
                    raise TransportError(f"HTTP request failed with status {exc.code}") from exc
                self._wait(exc.headers.get("Retry-After"), attempt)
            except TransportError:
                raise
            except (urllib.error.URLError, TimeoutError, OSError) as exc:
                if attempt + 1 >= self.max_attempts:
                    raise TransportError("request failed after the retry budget was exhausted") from exc
                self._wait(None, attempt)
        raise TransportError("request failed after the retry budget was exhausted")

    def _request_once(self, request: urllib.request.Request) -> bytes:
        return self._request_once_response(request).body

    def _request_once_response(self, request: urllib.request.Request) -> HTTPResponse:
        opener = urllib.request.build_opener(
            _CheckedRedirectHandler(self._check),
            urllib.request.HTTPSHandler(context=self._tls_context()),
        )
        with opener.open(request, timeout=self.timeout) as response:
            content_length = response.headers.get("Content-Length")
            if request.get_method() != "HEAD" and content_length and int(content_length) > self.max_bytes:
                raise TransportError("response exceeds configured byte limit")
            chunks: list[bytes] = []
            size = 0
            while True:
                chunk = response.read(min(1024 * 1024, self.max_bytes - size + 1))
                if not chunk:
                    break
                size += len(chunk)
                if size > self.max_bytes:
                    raise TransportError("response exceeds configured byte limit")
                chunks.append(chunk)
            return HTTPResponse(
                body=b"".join(chunks),
                status=int(getattr(response, "status", 200)),
                url=str(response.geturl()),
                headers={str(key).lower(): str(value) for key, value in response.headers.items()},
            )

    def _tls_context(self) -> ssl.SSLContext:
        context = ssl.create_default_context()
        # Keep platform roots and augment them with Mozilla's maintained public
        # roots. Some platform OpenSSL bundles omit roots trusted by the
        # browser/system store; loading certifi adds trust anchors without
        # disabling chain or hostname verification.
        context.load_verify_locations(cafile=certifi.where())
        # Python 3.13 enables OpenSSL X509 strict mode by default. Some public
        # meteorological endpoints still serve otherwise-valid chains that lack
        # legacy extension details required by strict mode. Keep CA-chain and
        # hostname verification enabled while matching pre-3.13 compatibility;
        # callers can opt back into X509 strict validation explicitly.
        if not self.x509_strict and hasattr(ssl, "VERIFY_X509_STRICT"):
            context.verify_flags &= ~ssl.VERIFY_X509_STRICT
        return context

    def _get_once(self, request: urllib.request.Request) -> bytes:
        """Compatibility helper kept for callers that used the old private method."""
        return self._request_once(request)

    def _wait(self, retry_after: str | None, attempt: int) -> None:
        delay = self.retry_backoff * (2**attempt)
        if retry_after:
            try:
                delay = max(delay, float(retry_after))
            except ValueError:
                try:
                    retry_at = parsedate_to_datetime(retry_after)
                    if retry_at.tzinfo is None:
                        retry_at = retry_at.replace(tzinfo=timezone.utc)
                    delay = max(delay, (retry_at - datetime.now(timezone.utc)).total_seconds())
                except (TypeError, ValueError, OverflowError):
                    pass
        time.sleep(min(max(delay, 0.0), 60.0))

    async def get(self, url: str, *, headers: Mapping[str, str] | None = None) -> bytes:
        return await asyncio.to_thread(self.get_sync, url, headers=headers)

    async def get_response(self, url: str, *, headers: Mapping[str, str] | None = None) -> HTTPResponse:
        return await asyncio.to_thread(self.get_response_sync, url, headers=headers)

    async def head_response(self, url: str, *, headers: Mapping[str, str] | None = None) -> HTTPResponse:
        return await asyncio.to_thread(self.head_response_sync, url, headers=headers)

    async def post(
        self,
        url: str,
        data: Mapping[str, str] | bytes | None = None,
        *,
        headers: Mapping[str, str] | None = None,
    ) -> bytes:
        return await asyncio.to_thread(self.post_sync, url, data, headers=headers)
