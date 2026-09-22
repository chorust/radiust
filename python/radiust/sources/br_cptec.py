"""Historical CPTEC nowcasting radar adapter.

The old CPTEC page exposed ``app.radares`` and a per-radar CAPPI JSON route.
The current site no longer exposes that route, so this adapter keeps the
legacy parser explicit and fails with no data when the upstream shape changes.
"""

from __future__ import annotations

import json
import re
from collections.abc import Iterable
from datetime import datetime, timezone
from typing import Any
from urllib.parse import urlencode, urljoin
from xml.etree import ElementTree

from ..context import SourceContext
from ..errors import DecodeError
from .legacy import LegacyImageSource, parse_provider_time


class BrCptecSource(LegacyImageSource):
    RADAR_PAGE_URL = "https://nowcasting.cptec.inpe.br/"
    RADAR_API_URL = "https://nowcasting.cptec.inpe.br/api/camadas/radar/{radar_id}/imagens"
    WMS_BASE_URL = "https://data.inpe.br/big/geoserver/dissm/wms"
    WMS_CAPABILITIES_URL = f"{WMS_BASE_URL}?service=WMS&request=GetCapabilities&version=1.3.0"
    PRODUCT_NAME = "CAPPI"

    @staticmethod
    def _bracketed(text: str, start: int) -> str | None:
        depth = 0
        quote: str | None = None
        escaped = False
        begin: int | None = None
        for index in range(start, len(text)):
            char = text[index]
            if quote is not None:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == quote:
                    quote = None
                continue
            if char in {"'", '"'}:
                quote = char
            elif char == "[":
                depth += 1
                begin = index if depth == 1 else begin
            elif char == "]" and depth:
                depth -= 1
                if depth == 0 and begin is not None:
                    return text[begin : index + 1]
        return None

    @classmethod
    def _radar_objects(cls, page: str) -> list[str]:
        match = re.search(r"app\.radares\s*=", page)
        if match is None:
            return []
        start = page.find("[", match.end())
        if start < 0:
            return []
        blob = cls._bracketed(page, start)
        if blob is None:
            return []
        objects: list[str] = []
        depth = 0
        begin: int | None = None
        quote: str | None = None
        escaped = False
        for index, char in enumerate(blob):
            if quote is not None:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == quote:
                    quote = None
                continue
            if char in {"'", '"'}:
                quote = char
            elif char == "{":
                depth += 1
                begin = index if depth == 1 else begin
            elif char == "}" and depth:
                depth -= 1
                if depth == 0 and begin is not None:
                    objects.append(blob[begin : index + 1])
                    begin = None
        return objects

    @staticmethod
    def _field(text: str, key: str) -> str | None:
        match = re.search(rf"\b{re.escape(key)}\s*:\s*(?:(['\"])(.*?)\1|(-?\d+))", text)
        if match is None:
            return None
        return match.group(2) if match.group(2) is not None else match.group(3)

    @classmethod
    def _radars(cls, page: str) -> list[tuple[str, str]]:
        result: list[tuple[str, str]] = []
        for item in cls._radar_objects(page):
            radar_id = cls._field(item, "id")
            name = cls._field(item, "nome")
            if radar_id and name:
                result.append((radar_id.strip(), name.strip()))
        return result

    @staticmethod
    def _products(value: Any) -> Iterable[dict[str, Any]]:
        pending = [value]
        while pending:
            current = pending.pop()
            if isinstance(current, dict):
                if "produto" in current and ("imagens" in current or "images" in current):
                    yield current
                pending.extend(value for value in current.values() if isinstance(value, (dict, list)))
            elif isinstance(current, list):
                pending.extend(current)

    @staticmethod
    def _images(entry: dict[str, Any]) -> list[dict[str, Any]]:
        values = entry.get("imagens", entry.get("images", []))
        return [item for item in values if isinstance(item, dict)] if isinstance(values, list) else []

    @staticmethod
    def _image_url(value: Any) -> str | None:
        if isinstance(value, str) and value:
            return value
        if isinstance(value, list):
            for item in value:
                candidate = BrCptecSource._image_url(item)
                if candidate:
                    return candidate
        if isinstance(value, dict):
            for key in ("url", "href", "link"):
                if isinstance(value.get(key), str) and value[key]:
                    return value[key]
        return None

    @staticmethod
    def _timestamp(file_date: Any, file_time: Any) -> datetime | None:
        if not isinstance(file_date, str) or not isinstance(file_time, str):
            return None
        for date_format in ("%Y-%m-%d", "%Y/%m/%d", "%d/%m/%Y", "%Y%m%d"):
            for time_format in ("%H:%M:%S", "%H:%M", "%H%M%S", "%H%M"):
                try:
                    date = datetime.strptime(file_date.strip(), date_format).date()
                    time = datetime.strptime(file_time.strip(), time_format).time()
                except ValueError:
                    continue
                return datetime.combine(date, time, tzinfo=timezone.utc)
        return None

    @staticmethod
    def _bbox(value: Any) -> tuple[float, float, float, float] | None:
        if not isinstance(value, list) or len(value) != 4 or not all(isinstance(item, (int, float)) for item in value):
            return None
        return tuple(float(item) for item in value)  # type: ignore[return-value]

    @staticmethod
    def _xml_name(tag: str) -> str:
        return tag.rsplit("}", 1)[-1]

    @classmethod
    def _xml_child_text(cls, element: ElementTree.Element, name: str) -> str | None:
        child = next((item for item in element if cls._xml_name(item.tag) == name), None)
        if child is None or child.text is None:
            return None
        return child.text.strip()

    @classmethod
    def _wms_entries(cls, payload: bytes) -> list[dict[str, Any]]:
        try:
            root = ElementTree.fromstring(payload)
        except ElementTree.ParseError as exc:
            raise DecodeError(f"CPTEC WMS GetCapabilities is not valid XML: {exc}") from exc

        entries: list[dict[str, Any]] = []
        for layer in root.iter():
            if cls._xml_name(layer.tag) != "Layer":
                continue
            name = cls._xml_child_text(layer, "Name")
            title = cls._xml_child_text(layer, "Title") or ""
            if not name or cls.PRODUCT_NAME not in title.upper():
                continue

            crs_values = {
                (item.text or "").strip().upper()
                for item in layer
                if cls._xml_name(item.tag) in {"CRS", "SRS"}
            }
            if "CRS:84" not in crs_values:
                continue
            bbox_element = next(
                (
                    item
                    for item in layer
                    if cls._xml_name(item.tag) == "BoundingBox"
                    and str(item.attrib.get("CRS", item.attrib.get("SRS", ""))).upper() == "CRS:84"
                ),
                None,
            )
            if bbox_element is not None:
                try:
                    bbox = tuple(
                        float(bbox_element.attrib[key]) for key in ("minx", "miny", "maxx", "maxy")
                    )
                except (KeyError, ValueError):
                    continue
            else:
                geographic = next(
                    (item for item in layer if cls._xml_name(item.tag) == "EX_GeographicBoundingBox"),
                    None,
                )
                if geographic is None:
                    continue
                try:
                    bbox = tuple(
                        float(cls._xml_child_text(geographic, field) or "")
                        for field in ("westBoundLongitude", "southBoundLatitude", "eastBoundLongitude", "northBoundLatitude")
                    )
                except ValueError:
                    continue

            time_values: list[str] = []
            for item in layer:
                if cls._xml_name(item.tag) not in {"Dimension", "Extent"}:
                    continue
                if str(item.attrib.get("name", "")).lower() != "time":
                    continue
                if item.text:
                    time_values.extend(value.strip() for value in item.text.split(",") if value.strip())
                default_time = item.attrib.get("default")
                if default_time:
                    time_values.append(default_time.strip())
            valid_times: list[datetime] = []
            for value in time_values:
                try:
                    valid_time = parse_provider_time(value)
                except (TypeError, ValueError, OverflowError):
                    continue
                if valid_time not in valid_times:
                    valid_times.append(valid_time)
            if not valid_times:
                continue

            style_element = next((item for item in layer if cls._xml_name(item.tag) == "Style"), None)
            style = cls._xml_child_text(style_element, "Name") if style_element is not None else None
            station = re.sub(r"[^A-Za-z0-9_-]", "", name.removeprefix("DISSM_Nowcasting_"))
            if not station:
                continue
            for valid_time in valid_times:
                stamp = valid_time.strftime("%Y%m%dT%H%M%SZ")
                params = {
                    "service": "WMS",
                    "version": "1.3.0",
                    "request": "GetMap",
                    "layers": name,
                    "styles": style or "",
                    "format": "image/png",
                    "transparent": "true",
                    "crs": "CRS:84",
                    "bbox": ",".join(str(value) for value in bbox),
                    "width": "512",
                    "height": "512",
                    "time": valid_time.strftime("%Y-%m-%dT%H:%M:%SZ"),
                }
                entries.append(
                    {
                        "url": f"{cls.WMS_BASE_URL}?{urlencode(params)}",
                        "name": f"{station}_{stamp}.png",
                        "station": station,
                        "valid_time": valid_time,
                        "bbox": bbox,
                        "revision": f"{name}-{stamp}",
                        "metadata": {
                            "interface": "wms",
                            "time_semantics": "wms_time_dimension_utc",
                            "provider_layer": name,
                            "provider_product": cls.PRODUCT_NAME,
                            "wms_crs": "CRS:84",
                            "geometry_status": "declared_bbox_pending_pixel_control_points",
                        },
                    }
                )
        return entries

    @staticmethod
    def _validate_image(payload: bytes, name: str) -> None:
        if b"serviceexceptionreport" in payload[:2048].lower():
            raise DecodeError("CPTEC WMS GetMap returned a ServiceException instead of an image")
        LegacyImageSource._validate_image(payload, name)

    async def discover_entries(self, context: SourceContext):
        page = (await self._get(context, self.RADAR_PAGE_URL)).decode("utf-8", errors="replace")
        entries: list[dict[str, Any]] = []
        radars = self._radars(page)
        if not radars:
            capabilities = await self._get(context, self.WMS_CAPABILITIES_URL)
            return self._wms_entries(capabilities)
        for radar_id, radar_name in radars:
            query = urlencode({"quantidade": 5, "nome": radar_name})
            endpoint = f"{self.RADAR_API_URL.format(radar_id=radar_id)}?{query}"
            payload = await self._get(context, endpoint)
            try:
                document = json.loads(payload)
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                raise DecodeError(f"CPTEC radar response is not valid JSON: {exc}") from exc
            for product in self._products(document):
                if str(product.get("produto", "")).strip().upper() != self.PRODUCT_NAME:
                    continue
                bbox = self._bbox(product.get("boundingBox", product.get("bounding_box")))
                for image in self._images(product):
                    valid_time = self._timestamp(image.get("fileDate"), image.get("fileTime"))
                    url = self._image_url(image.get("urls"))
                    if valid_time is None or not url:
                        continue
                    station = f"BRCPTEC{re.sub(r'[^A-Za-z0-9]+', '', radar_id)}"
                    entries.append(
                        {
                            "url": urljoin(self.RADAR_PAGE_URL, url),
                            "station": station,
                            "valid_time": valid_time,
                            "bbox": bbox,
                            "revision": f"{station}-{valid_time.strftime('%Y%m%dT%H%M%SZ')}",
                            "metadata": {
                                "time_semantics": "fileDate_fileTime_utc",
                                "provider_radar": radar_id,
                                "provider_product": self.PRODUCT_NAME,
                                "geometry_status": "provider_bbox_pending_control_points",
                            },
                        }
                    )
        return entries


__all__ = ["BrCptecSource"]
