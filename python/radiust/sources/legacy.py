"""Shared building blocks for adapters migrated from the legacy scrapers.

The old repository coupled discovery, downloading, decoding and object-store
publishing in one ``RadarScraper`` class.  The adapters in this module keep the
provider-specific discovery rules in small subclasses while sharing the new
three-stage source contract and resource limits.
"""

from __future__ import annotations

import hashlib
import io
import json
import mimetypes
import re
from abc import ABC
from collections.abc import Iterable, Mapping
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, ClassVar
from urllib.parse import parse_qsl, unquote, urljoin, urlparse

import numpy as np
import xarray as xr
from PIL import Image

from ..context import SourceContext
from ..decoders.exact import QUALITY_UNKNOWN, ExactPaletteDecoder
from ..decoders.gray_dbz import LegacyGrayDbzDecoder
from ..errors import (
    AuthenticationError,
    DecodeError,
    ErrorContext,
    GeoreferencingError,
    IntegrityError,
    NoDataError,
    ResourceLimitError,
    TransportError,
    UnsupportedQueryError,
)
from ..field import RadarField
from ..grids import GeographicGrid
from ..logging import redact
from ..models import Artifact, FrameRef, Query, SourceInfo, format_time, utc_datetime
from ..raw import RawFrame
from .base import FixtureSource, Source


def parse_provider_time(value: Any, *, timezone_name: timezone = timezone.utc) -> datetime:
    """Parse the timestamp shapes used by the legacy providers."""

    if isinstance(value, datetime):
        return utc_datetime(value)
    if isinstance(value, (int, float)):
        return datetime.fromtimestamp(float(value), timezone.utc)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"invalid provider time: {value!r}")
    text = value.strip()
    if text.isdigit() and len(text) in {10, 13}:
        seconds = float(text) / (1000 if len(text) == 13 else 1)
        return datetime.fromtimestamp(seconds, timezone.utc)
    parsed = datetime.fromisoformat(text.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone_name)
    return utc_datetime(parsed)


def filename_from_url(url: str, fallback: str = "frame.bin") -> str:
    name = Path(unquote(urlparse(url).path)).name
    if not name or name in {".", ".."}:
        return fallback
    # Providers occasionally put path separators or control bytes in a key.
    name = re.sub(r"[^A-Za-z0-9._-]", "_", name)
    return name[:180] or fallback


def first_json_object(payload: bytes) -> Any:
    try:
        return json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise DecodeError(f"provider response is not valid JSON: {exc}") from exc


def extract_urls(text: str, *, extensions: Iterable[str] = ("png", "gif", "jpg", "jpeg", "webp")) -> list[str]:
    suffix = "|".join(re.escape(ext) for ext in extensions)
    return re.findall(rf"(?:https?://|/)[^\"'<>\s]+\.(?:{suffix})(?:\?[^\"'<>\s]*)?", text, flags=re.IGNORECASE)


_CREDENTIAL_FIELD = re.compile(
    r"(?:token|(?:api|access)[_-]?key|secret|password|credential|signature|authorization|cookie)",
    re.IGNORECASE,
)


def _configured_request_secrets(
    context: SourceContext,
    source_id: str,
    url: str,
    headers: Mapping[str, str] | None,
    body: Mapping[str, Any] | bytes | str | None = None,
) -> tuple[str, ...]:
    """Return in-memory credential values relevant to one request for redaction."""
    values: set[str] = set()

    def add(value: Any) -> None:
        if value in (None, ""):
            return
        secret = str(value).strip()
        if not secret:
            return
        values.add(secret)
        scheme, separator, credential = secret.partition(" ")
        if separator and scheme.lower() in {"basic", "bearer", "token"} and credential:
            values.add(credential)

    def add_sensitive_mapping(value: Any) -> None:
        if isinstance(value, Mapping):
            for key, item in value.items():
                if _CREDENTIAL_FIELD.search(str(key)):
                    add(item)
                else:
                    add_sensitive_mapping(item)
        elif isinstance(value, (list, tuple)):
            for item in value:
                add_sensitive_mapping(item)

    sources = context.config.values.get("sources", {})
    configured = sources.get(source_id, {}) if isinstance(sources, Mapping) else {}
    if isinstance(configured, Mapping):
        add_sensitive_mapping(configured)
    for key, value in (headers or {}).items():
        if _CREDENTIAL_FIELD.search(str(key)) and value not in (None, ""):
            add(value)
    parsed_url = urlparse(url)
    if parsed_url.username:
        add(unquote(parsed_url.username))
    if parsed_url.password:
        add(unquote(parsed_url.password))
    for key, value in parse_qsl(parsed_url.query, keep_blank_values=True):
        if _CREDENTIAL_FIELD.search(key) and value:
            add(value)
    if isinstance(body, Mapping):
        add_sensitive_mapping(body)
    elif isinstance(body, (bytes, str)):
        raw_body = body.decode("utf-8", errors="ignore") if isinstance(body, bytes) else body
        if len(raw_body) <= 1_000_000:
            try:
                body_value = json.loads(raw_body)
            except (UnicodeDecodeError, json.JSONDecodeError):
                body_value = None
            if isinstance(body_value, (Mapping, list, tuple)):
                add_sensitive_mapping(body_value)
            else:
                for key, value in parse_qsl(raw_body, keep_blank_values=True):
                    if _CREDENTIAL_FIELD.search(key) and value:
                        add(value)
    return tuple(sorted(values, key=len, reverse=True))


def _sanitized_transport_error(
    exc: Exception,
    *,
    context: SourceContext,
    source_id: str,
    url: str,
    headers: Mapping[str, str] | None,
    body: Mapping[str, Any] | bytes | str | None = None,
) -> Exception | None:
    """Return a safe public error when request credentials appeared in its message."""
    original = str(exc)
    message = redact(original)
    for secret in _configured_request_secrets(context, source_id, url, headers, body):
        message = message.replace(secret, "<redacted>")
    if message == original:
        return None
    if isinstance(exc, AuthenticationError):
        return AuthenticationError(message, context=exc.context)
    if isinstance(exc, ResourceLimitError):
        return ResourceLimitError(message, context=exc.context)
    if isinstance(exc, IntegrityError):
        return IntegrityError(message, context=exc.context)
    if isinstance(exc, TransportError):
        return TransportError(message, context=exc.context)
    return TransportError(message, context=ErrorContext(stage="acquire", source=source_id))


class LegacyImageSource(Source, ABC):
    """Base class for a provider whose canonical raw artifact is an image."""

    discovery_url: ClassVar[str | None] = None
    default_bbox: ClassVar[tuple[float, float, float, float] | None] = None
    provider_timezone: ClassVar[timezone] = timezone.utc
    historical: ClassVar[bool] = True
    value_mode: ClassVar[str] = "unsupported"
    palette: ClassVar[Mapping[tuple[int, int, int, int], float | None | tuple[float | None, int]] | None] = None
    strict_palette: ClassVar[bool] = False

    def __init__(self, info: SourceInfo, fixture_path: Path | None = None) -> None:
        self.info = info
        self.fixture_path = fixture_path

    @property
    def _fixture_delegate(self) -> FixtureSource | None:
        if self.fixture_path is not None and self.fixture_path.exists():
            return FixtureSource(self.info, self.fixture_path)
        return None

    async def _get(self, context: SourceContext, url: str, *, headers: Mapping[str, str] | None = None) -> bytes:
        context.cancellation.check()
        getter = context.transport.get
        try:
            if headers:
                try:
                    return await getter(url, headers=headers)
                except TypeError:
                    # Small replay transports intentionally implement only get(url).
                    return await getter(url)
            return await getter(url)
        except Exception as exc:
            public_error = _sanitized_transport_error(
                exc,
                context=context,
                source_id=self.info.id,
                url=url,
                headers=headers,
            )
            if public_error is None:
                raise
            raise public_error from None

    async def _request(
        self,
        context: SourceContext,
        url: str,
        *,
        method: str = "GET",
        body: Mapping[str, str] | bytes | None = None,
        headers: Mapping[str, str] | None = None,
    ) -> bytes:
        method = method.upper()
        if method == "GET":
            return await self._get(context, url, headers=headers)
        if method != "POST":
            raise UnsupportedQueryError(f"source {self.info.id} requires unsupported HTTP method {method}")
        context.cancellation.check()
        poster = getattr(context.transport, "post", None)
        if poster is None:
            raise UnsupportedQueryError(f"source {self.info.id} requires a transport with POST support")
        try:
            try:
                return await poster(url, body, headers=headers)
            except TypeError:
                return await poster(url, body)
        except Exception as exc:
            public_error = _sanitized_transport_error(
                exc,
                context=context,
                source_id=self.info.id,
                url=url,
                headers=headers,
                body=body,
            )
            if public_error is None:
                raise
            raise public_error from None

    async def discover_entries(self, context: SourceContext) -> list[dict[str, Any]]:
        if self.discovery_url is None:
            raise NoDataError(f"source {self.info.id} has no discovery endpoint")
        payload = await self._get(context, self.discovery_url)
        return self.parse_discovery(payload, self.discovery_url)

    async def discover_entries_for_query(self, context: SourceContext, query: Query) -> list[dict[str, Any]]:
        """Allow directory adapters to apply station filters before network traversal."""
        return await self.discover_entries(context)

    def parse_discovery(self, payload: bytes, source_url: str) -> list[dict[str, Any]]:
        """Default parser for JSON lists or HTML pages containing image URLs."""

        content_type = payload[:1]
        if content_type in {b"[", b"{"}:
            value = first_json_object(payload)
            candidates = value if isinstance(value, list) else value.get("items", []) if isinstance(value, dict) else []
            entries: list[dict[str, Any]] = []
            for item in candidates:
                if not isinstance(item, dict):
                    continue
                url = item.get("url") or item.get("image") or item.get("path")
                timestamp = item.get("valid_time") or item.get("time") or item.get("timestamp") or item.get("dateTimeISO")
                if not isinstance(url, str) or timestamp is None:
                    continue
                try:
                    valid_time = parse_provider_time(timestamp, timezone_name=self.provider_timezone)
                except (TypeError, ValueError, OverflowError):
                    continue
                entries.append({"url": urljoin(source_url, url), "valid_time": valid_time, **item})
            return entries

        text = payload.decode("utf-8", errors="replace")
        entries = []
        for url in extract_urls(text):
            timestamp = self._time_from_text(url)
            if timestamp is not None:
                entries.append({"url": urljoin(source_url, url), "valid_time": timestamp})
        return entries

    def _time_from_text(self, text: str) -> datetime | None:
        for pattern, fmt in (
            (r"(20\d{12})", "%Y%m%d%H%M%S"),
            (r"(20\d{10})", "%Y%m%d%H%M"),
            (r"(20\d{8})", "%Y%m%d"),
        ):
            match = re.search(pattern, text)
            if match:
                try:
                    parsed = datetime.strptime(match.group(1), fmt).replace(tzinfo=self.provider_timezone)
                    return utc_datetime(parsed)
                except ValueError:
                    continue
        return None

    def _product(self, query: Query) -> str:
        product = self.info.default_product
        if product is None:
            raise UnsupportedQueryError(f"source {self.info.id} requires an explicit product")
        if query.product is not None and query.product != product.id:
            raise UnsupportedQueryError(f"unsupported product for {self.info.id}: {query.product}")
        return product.id

    def _make_ref(self, entry: Mapping[str, Any], product: str) -> FrameRef:
        url = str(entry["url"])
        valid_time = entry.get("valid_time")
        if not isinstance(valid_time, datetime):
            valid_time = parse_provider_time(valid_time, timezone_name=self.provider_timezone)
        station = entry.get("station") or entry.get("radar_id")
        if station is not None:
            station = str(station)
        locator = {"url": url, "artifacts": tuple(entry.get("artifacts", ()))}
        bbox = entry.get("bbox", self.default_bbox)
        if bbox is not None:
            locator["bbox"] = tuple(bbox)
        if entry.get("headers"):
            locator["headers"] = dict(entry["headers"])
        for key in (
            "station",
            "timestamp_str",
            "revision",
            "metadata_url",
            "method",
            "body",
            "name",
            "media_type",
            "payload_sha256",
        ):
            if key in entry and entry[key] is not None:
                locator[key] = entry[key]
        revision = entry.get("revision") or self._revision(url, valid_time)
        metadata = dict(entry.get("metadata", {}))
        metadata.setdefault("time_semantics", "provider_frame_time")
        return FrameRef(
            source=self.info.id,
            product=product,
            station=station,
            valid_time=valid_time,
            uri=url,
            locator=locator,
            locator_version=f"{self.info.id}-legacy-v1",
            metadata=metadata,
            revision=str(revision),
        )

    @staticmethod
    def _revision(url: str, valid_time: datetime) -> str:
        return hashlib.sha256(f"{url}\0{format_time(valid_time)}".encode()).hexdigest()[:16]

    async def discover(self, query: Query, context: SourceContext) -> list[FrameRef]:
        delegate = self._fixture_delegate
        if delegate is not None:
            return await delegate.discover(query, context)
        if query.source != self.info.id:
            raise UnsupportedQueryError(f"source mismatch: {query.source}")
        if self.info.availability == "retired":
            evidence = self.info.availability_evidence or "upstream service has been retired"
            raise UnsupportedQueryError(f"source {self.info.id} is retired: {evidence}")
        product = self._product(query)
        if query.base_time is not None:
            raise UnsupportedQueryError(f"source {self.info.id} does not expose base times")
        if not self.historical and (query.at is not None or query.start is not None):
            raise UnsupportedQueryError(f"source {self.info.id} only supports latest frames")
        entries = await self.discover_entries_for_query(context, query)
        refs: list[FrameRef] = []
        for entry in entries:
            try:
                ref = self._make_ref(entry, product)
            except (KeyError, TypeError, ValueError):
                continue
            if query.stations and ref.station not in query.stations:
                continue
            refs.append(ref)
        if query.at is not None:
            refs = [ref for ref in refs if ref.valid_time == query.at]
        elif query.start is not None:
            refs = [ref for ref in refs if query.start <= ref.valid_time < query.end]  # type: ignore[operator]
        elif query.latest:
            latest: dict[str | None, FrameRef] = {}
            for ref in refs:
                if ref.station not in latest or ref.valid_time > latest[ref.station].valid_time:
                    latest[ref.station] = ref
            refs = list(latest.values())
            if query.max_age is not None:
                now = datetime.now(timezone.utc)
                refs = [ref for ref in refs if ref.valid_time > now or now - ref.valid_time <= query.max_age]
        refs.sort(key=lambda ref: (ref.valid_time, ref.station or "", ref.logical_id))
        if not refs:
            raise NoDataError(f"source {self.info.id} has no matching frame")
        return refs

    async def download(self, ref: FrameRef, context: SourceContext) -> RawFrame:
        delegate = self._fixture_delegate
        if delegate is not None:
            return await delegate.download(ref, context)
        context.cancellation.check()
        root = context.temp_root
        if root is None:
            raise RuntimeError("source context has no temporary root")
        loc = dict(ref.locator)
        requested = [
            {
                "url": loc["url"],
                "name": str(loc.get("name") or filename_from_url(str(loc["url"]))),
                "role": "data",
                "media_type": loc.get("media_type"),
            }
        ]
        extra = loc.get("artifacts", ())
        if isinstance(extra, Mapping):
            extra = (extra,)
        requested.extend(dict(item) for item in extra if isinstance(item, Mapping))
        if loc.get("metadata_url"):
            requested.append({"url": loc["metadata_url"], "name": filename_from_url(str(loc["metadata_url"]), "metadata.json"), "role": "metadata", "media_type": "application/json"})
        artifacts: list[Artifact] = []
        for item in requested:
            context.cancellation.check()
            url = str(item["url"])
            payload = await self._request(
                context,
                url,
                method=str(loc.get("method", "GET")),
                body=loc.get("body"),
                headers=loc.get("headers"),
            )
            context.check_bytes(len(payload))
            name = filename_from_url(str(item.get("name") or url))
            target = root / name
            target.write_bytes(payload)
            media_type = str(item.get("media_type") or mimetypes.guess_type(name)[0] or "application/octet-stream")
            if media_type.startswith("image/") or Path(name).suffix.lower() in {".png", ".gif", ".jpg", ".jpeg", ".webp"}:
                self._validate_image(payload, name)
            artifacts.append(
                Artifact(
                    name=name,
                    role=str(item.get("role", "data")),
                    media_type=media_type,
                    payload=target,
                    source_revision=ref.revision,
                    size_bytes=len(payload),
                    sha256=hashlib.sha256(payload).hexdigest(),
                )
            )
        if not artifacts:
            raise NoDataError(f"source {self.info.id} returned no artifacts")
        context.check_frame_bytes(sum(item.size_bytes or 0 for item in artifacts))
        return RawFrame(ref, tuple(artifacts), metadata={"bbox": loc.get("bbox", self.default_bbox), "value_mode": self.value_mode})

    @staticmethod
    def _validate_image(payload: bytes, name: str) -> None:
        try:
            with Image.open(io.BytesIO(payload)) as image:
                image.verify()
        except Exception as exc:
            raise IntegrityError(f"invalid image artifact {name}: {exc}") from exc

    def decode(self, raw: RawFrame, context: SourceContext) -> RadarField | xr.Dataset:
        delegate = self._fixture_delegate
        if delegate is not None:
            return delegate.decode(raw, context)
        raw._ensure_open()
        artifact = next((item for item in raw.artifacts if item.role == "data"), raw.artifacts[0])
        payload = artifact.payload if isinstance(artifact.payload, bytes) else Path(artifact.payload).read_bytes()
        try:
            if Path(artifact.name).suffix.lower() in {".nc", ".netcdf"}:
                return xr.open_dataset(io.BytesIO(payload)).load()
            with Image.open(io.BytesIO(payload)) as image:
                rgba = np.asarray(image.convert("RGBA"), dtype=np.uint8)
        except Exception as exc:
            raise DecodeError(f"unable to decode {artifact.name}: {exc}") from exc
        context.check_pixels(int(rgba.shape[0] * rgba.shape[1]) * 2)
        if self.value_mode == "legacy_gray_dbz":
            values, quality = LegacyGrayDbzDecoder().decode(rgba)
            value_semantics = "旧项目-gray-dbz-v1"
        elif self.palette is None:
            raise DecodeError(
                f"source {self.info.id} has no verified scientific decoder; preserve raw data or use raw-only mode"
            )
        else:
            values, quality = ExactPaletteDecoder(self.palette, strict=self.strict_palette).decode(rgba)
            value_semantics = "provider_palette"
        values = np.asarray(values, dtype=np.float32)
        values[quality & QUALITY_UNKNOWN != 0] = np.nan
        raw_bbox = raw.metadata.get("bbox", self.default_bbox)
        if raw_bbox is None:
            raise GeoreferencingError(f"source {self.info.id} has no verified native geometry")
        bbox = tuple(raw_bbox)
        west, south, east, north = (float(value) for value in bbox)
        height, width = values.shape
        longitude = np.linspace(west, east, width, dtype=np.float64)
        latitude = np.linspace(north, south, height, dtype=np.float64)
        grid = GeographicGrid(longitude, latitude)
        product = self.info.default_product
        if product is None:
            raise DecodeError(f"source {self.info.id} has no default product")
        variable = product.variables[0]
        data = xr.DataArray(values, dims=("latitude", "longitude"), coords={"latitude": latitude, "longitude": longitude}, name=variable, attrs={"units": product.units.get(variable, "1")})
        quality_array = xr.DataArray(quality, dims=data.dims, coords=data.coords, name="quality")
        return RadarField(
            data,
            grid,
            quality_array,
            {
                "source": self.info.id,
                "product": raw.ref.product,
                "station": raw.ref.station,
                "valid_time": format_time(raw.ref.valid_time),
                "value_semantics": value_semantics,
                "raw": artifact.name,
            },
        )


__all__ = ["LegacyImageSource", "extract_urls", "filename_from_url", "first_json_object", "parse_provider_time"]
