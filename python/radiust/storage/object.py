"""Anonymous object reads with provider-aware URI normalization."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from urllib.parse import quote, unquote, urlsplit

from .. import _bridge
from ..errors import (
    AuthenticationError,
    ErrorContext,
    OutputConflict,
    ResourceLimitError,
    StorageError,
    TransportError,
)
from ..transport import HTTPTransport


@dataclass(frozen=True, slots=True)
class ObjectLocation:
    provider: str
    bucket: str
    key: str
    url: str


@dataclass(frozen=True, slots=True)
class ObjectReceipt:
    size_bytes: int
    sha256: str
    etag: str | None = None


def parse_object_uri(
    uri: str,
    *,
    provider: str | None = None,
    endpoint: str | None = None,
    region: str | None = None,
) -> ObjectLocation:
    parsed = urlsplit(uri)
    scheme = parsed.scheme.lower()
    selected = (provider or scheme).lower()
    if selected in {"aws", "s3-compatible", "minio"}:
        selected = "s3"
    if selected in {"aliyun-oss"}:
        selected = "oss"
    if scheme in {"s3", "oss"} and selected != scheme:
        raise StorageError(f"object URI scheme {scheme} does not match provider {selected}")
    if scheme in {"http", "https"} and selected not in {"http", "https", "s3", "oss"}:
        raise StorageError(f"unsupported object provider: {selected}")
    if scheme not in {"s3", "oss", "http", "https"}:
        raise StorageError(f"unsupported object URI scheme: {scheme or '<missing>'}")
    if parsed.username or parsed.password:
        raise StorageError("object URI must not contain credentials")
    if parsed.query or parsed.fragment:
        raise StorageError("object URI must not contain a query or fragment")

    if scheme in {"http", "https"}:
        key = _safe_key(unquote(parsed.path.lstrip("/")))
        if not key:
            raise StorageError("object URL must contain a path")
        return ObjectLocation(selected if selected in {"s3", "oss"} else "http", parsed.netloc, key, uri)

    bucket = parsed.netloc
    key = _safe_key(unquote(parsed.path.lstrip("/")))
    if not bucket or not key:
        raise StorageError("object URI requires a bucket and key")
    virtual_hosted = endpoint is None
    if endpoint is None:
        if scheme == "s3":
            endpoint = f"https://{quote(bucket, safe='')}.s3.amazonaws.com"
        elif region:
            endpoint = f"https://{quote(bucket, safe='')}.oss-{quote(region, safe='')}.aliyuncs.com"
        else:
            endpoint = f"https://{quote(bucket, safe='')}.oss.aliyuncs.com"
    endpoint_parts = urlsplit(endpoint)
    if endpoint_parts.scheme not in {"http", "https"} or endpoint_parts.username or endpoint_parts.password:
        raise StorageError("object endpoint must be an HTTP(S) URL without credentials")
    if endpoint_parts.query or endpoint_parts.fragment or not endpoint_parts.netloc:
        raise StorageError("object endpoint must not contain a query or fragment")
    if not virtual_hosted:
        hostname = (endpoint_parts.hostname or "").lower()
        bucket_host = f"{bucket.lower()}."
        if hostname.startswith(bucket_host):
            suffix = hostname[len(bucket_host):]
            virtual_hosted = (
                (scheme == "s3" and (suffix == "s3.amazonaws.com" or (suffix.startswith("s3.") and suffix.endswith(".amazonaws.com"))))
                or (scheme == "oss" and (suffix == "oss.aliyuncs.com" or (suffix.startswith("oss-") and suffix.endswith(".aliyuncs.com"))))
            )
    base = endpoint.rstrip("/")
    object_path = quote(key, safe="/~")
    if not virtual_hosted:
        object_path = f"{quote(bucket, safe='')}/{object_path}"
    url = f"{base}/{object_path}"
    return ObjectLocation(selected, bucket, key, url)


class ObjectStore:
    """Read anonymous HTTP/object-provider data and compute a real SHA-256."""

    def __init__(
        self,
        uri: str,
        *,
        provider: str | None = None,
        endpoint: str | None = None,
        region: str | None = None,
        transport: HTTPTransport | None = None,
        anonymous: bool = True,
    ) -> None:
        self.location = parse_object_uri(uri, provider=provider, endpoint=endpoint, region=region)
        self._transport = transport
        self._anonymous = anonymous

    async def read(self, *, max_bytes: int = 512 * 1024 * 1024) -> tuple[bytes, ObjectReceipt]:
        if max_bytes < 0:
            raise ResourceLimitError("object byte limit must be non-negative")
        if not self._anonymous:
            raise AuthenticationError(
                "credentialed object reads require the configured Rust provider chain",
                context=ErrorContext(stage="acquire", source=self.location.provider),
            )
        transport = self._transport or HTTPTransport(max_bytes=max_bytes)
        try:
            payload = await transport.get(self.location.url)
        except TransportError as exc:
            if "exceeds configured byte limit" in str(exc):
                raise ResourceLimitError(
                    f"object exceeds configured limit {max_bytes}",
                    context=ErrorContext(stage="acquire", source=self.location.provider),
                    cause=exc,
                ) from exc
            raise
        if len(payload) > max_bytes:
            raise ResourceLimitError(
                f"object exceeds configured limit {max_bytes}",
                context=ErrorContext(stage="acquire", source=self.location.provider),
            )
        return payload, ObjectReceipt(len(payload), hashlib.sha256(payload).hexdigest())

    def __repr__(self) -> str:
        return f"ObjectStore(provider={self.location.provider!r}, bucket={self.location.bucket!r}, key={self.location.key!r})"


class RustObjectBackend:
    """Remote commit backend backed by the standard wheel's OpenDAL adapter."""

    def __init__(
        self,
        uri: str,
        *,
        endpoint: str | None = None,
        region: str | None = None,
        access_key_id: str | None = None,
        secret_access_key: str | None = None,
        anonymous: bool = False,
        max_bytes: int = 512 * 1024 * 1024,
    ) -> None:
        self.location = parse_object_uri(uri, endpoint=endpoint, region=region)
        if self.location.provider not in {"s3", "oss"}:
            raise StorageError("remote output requires an s3:// or oss:// URI")
        self._endpoint = endpoint
        self._region = region or ("us-east-1" if self.location.provider == "s3" else None)
        self._access_key_id = access_key_id
        self._secret_access_key = secret_access_key
        self._anonymous = anonymous
        self._max_bytes = max_bytes

    async def get(self, key: str) -> bytes | None:
        result = await _bridge.object_read(
            self.location.provider,
            self.location.bucket,
            "",
            key,
            endpoint=self._endpoint,
            region=self._region,
            access_key_id=self._access_key_id,
            secret_access_key=self._secret_access_key,
            anonymous=self._anonymous,
            max_bytes=self._max_bytes,
        )
        if result is None:
            return None
        payload, size_bytes, sha256 = result
        if len(payload) != size_bytes or _bridge.sha256(payload) != sha256:
            raise StorageError("remote object receipt failed verification", context=ErrorContext(stage="acquire"))
        return payload

    async def put(self, key: str, payload: bytes, *, overwrite: bool) -> None:
        try:
            size_bytes, sha256 = await _bridge.object_write(
                self.location.provider,
                self.location.bucket,
                "",
                key,
                [payload],
                endpoint=self._endpoint,
                region=self._region,
                access_key_id=self._access_key_id,
                secret_access_key=self._secret_access_key,
                anonymous=self._anonymous,
                overwrite=overwrite,
                max_bytes=self._max_bytes,
            )
        except StorageError as exc:
            if "object already exists" in str(exc):
                raise OutputConflict(str(exc), context=ErrorContext(stage="commit"), cause=exc) from exc
            raise
        if size_bytes != len(payload) or sha256 != _bridge.sha256(payload):
            raise StorageError("remote object receipt failed verification", context=ErrorContext(stage="commit"))


def _safe_key(key: str) -> str:
    parts = [part for part in key.split("/") if part]
    if not parts or any(part in {".", ".."} for part in parts):
        raise StorageError("object key must be a safe relative path")
    return "/".join(parts)


__all__ = ["ObjectLocation", "ObjectReceipt", "ObjectStore", "RustObjectBackend", "parse_object_uri"]
