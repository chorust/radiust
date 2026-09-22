"""Taiwan CWA anonymous S3 observation radar acquisition."""

from __future__ import annotations

import json
from io import BytesIO

import numpy as np
import xarray as xr
from PIL import Image

from ..context import SourceContext
from ..decoders.exact import QUALITY_MISSING, QUALITY_OUTSIDE
from ..errors import DecodeError, NoDataError, StaleFrameError, UnsupportedQueryError
from ..field import RadarField
from ..grids import GeographicGrid
from ..models import Query, format_time
from ..raw import RawFrame
from .legacy import LegacyImageSource, parse_provider_time


class TwSource(LegacyImageSource):
    BUCKET_BASE = "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation"
    PRODUCT_KEY = "O-A0058-005"
    GRID_KEY = "O-A0059-001"
    GRID_PRODUCT = "grid"

    @classmethod
    def _png_metadata(cls, payload: bytes):
        """Read provider time, declared coverage and dimensions from companion JSON."""
        try:
            document = json.loads(payload)
            provider = document["cwaopendata"]
            if provider["dataid"] != cls.PRODUCT_KEY:
                raise ValueError("unexpected CWA PNG product id")
            dataset = provider["dataset"]
            resource = dataset["resource"]
            expected_url = f"{cls.BUCKET_BASE}/{cls.PRODUCT_KEY}.png"
            if resource["ProductURL"] != expected_url or resource["mimeType"] != "image/png":
                raise ValueError("unexpected CWA PNG resource URL or type")
            params = dataset["datasetInfo"]["parameterSet"]
            west, east = (float(value) for value in params["LongitudeRange"].split("-"))
            south, north = (float(value) for value in params["LatitudeRange"].split("-"))
            width, height = (int(value) for value in params["ImageDimension"].lower().split("x"))
            if not np.isfinite([west, east, south, north]).all() or not (-180 <= west < east <= 180 and -90 <= south < north <= 90):
                raise ValueError("invalid CWA PNG declared geographic coverage")
            if width < 1 or height < 1:
                raise ValueError("invalid CWA PNG image dimensions")
            return parse_provider_time(dataset["DateTime"]), (west, south, east, north), (height, width)
        except (UnicodeError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
            raise DecodeError(f"invalid CWA PNG metadata: {exc}") from exc

    async def discover(self, query: Query, context: SourceContext):
        if query.product != self.GRID_PRODUCT:
            return await super().discover(query, context)
        if query.source != self.info.id or query.base_time is not None:
            raise UnsupportedQueryError("invalid Taiwan grid query")
        url = f"{self.BUCKET_BASE}/{self.GRID_KEY}.json"
        payload = await self._get(context, url)
        try:
            document = json.loads(payload)
            if document["cwaopendata"]["dataid"] != self.GRID_KEY:
                raise ValueError("unexpected CWA numeric product id")
            dataset = document["cwaopendata"]["dataset"]
            valid_time = parse_provider_time(dataset["datasetInfo"]["parameterSet"]["DateTime"])
        except (UnicodeError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
            raise DecodeError(f"CWA numeric grid metadata is invalid: {exc}") from exc
        ref = self._make_ref(
            {
                "url": url,
                "station": "CV1_3600",
                "valid_time": valid_time,
                "revision": f"{self.GRID_KEY}-{int(valid_time.timestamp())}",
                "metadata": {"time_semantics": "provider_grid_time", "geometry_status": "provider_native_twd67"},
            },
            self.GRID_PRODUCT,
        )
        if query.stations and ref.station not in query.stations:
            raise NoDataError("CWA numeric grid has no matching station")
        if query.at is not None and ref.valid_time != query.at:
            raise NoDataError("CWA numeric grid does not provide the requested historical frame")
        if query.start is not None and not query.start <= ref.valid_time < query.end:
            raise NoDataError("CWA numeric grid has no frame in the requested interval")
        if query.max_age is not None:
            from datetime import datetime, timezone

            age = datetime.now(timezone.utc) - ref.valid_time
            if age.total_seconds() >= 0 and age > query.max_age:
                raise NoDataError("CWA numeric grid frame is older than max_age")
        return [ref]

    async def discover_entries(self, context: SourceContext):
        metadata_url = f"{self.BUCKET_BASE}/{self.PRODUCT_KEY}.json"
        payload = await self._get(context, metadata_url)
        valid_time, declared_extent, image_shape = self._png_metadata(payload)
        image_url = f"{self.BUCKET_BASE}/{self.PRODUCT_KEY}.png"
        return [
            {
                "url": image_url,
                "station": "CV1_3600",
                "valid_time": valid_time,
                "metadata_url": metadata_url,
                "revision": f"{self.PRODUCT_KEY}-{int(valid_time.timestamp())}",
                "metadata": {
                    "time_semantics": "provider_metadata_time",
                    "geometry_status": "unverified",
                    "provider_declared_extent": declared_extent,
                    "provider_image_shape": image_shape,
                    "object_transport": "anonymous_s3_https",
                },
            }
        ]

    async def download(self, ref, context: SourceContext) -> RawFrame:
        raw = await super().download(ref, context)
        if ref.product == self.GRID_PRODUCT:
            return raw
        try:
            if ref.product != "observation":
                raise UnsupportedQueryError(f"unsupported CWA product: {ref.product}")
            metadata = next((item for item in raw.artifacts if item.role == "metadata"), None)
            if metadata is None:
                raise DecodeError("CWA PNG companion metadata is missing")
            provider_time, declared_extent, image_shape = self._png_metadata(raw.bytes(metadata.name))
            if provider_time != ref.valid_time:
                raise StaleFrameError("CWA PNG metadata time changed after discovery; rediscover latest")
            if declared_extent != tuple(ref.metadata["provider_declared_extent"]) or image_shape != tuple(ref.metadata["provider_image_shape"]):
                raise StaleFrameError("CWA PNG coverage or dimensions changed after discovery; rediscover latest")
            image_artifact = next((item for item in raw.artifacts if item.role == "data"), None)
            if image_artifact is None:
                raise DecodeError("CWA PNG image artifact is missing")
            with Image.open(BytesIO(raw.bytes(image_artifact.name))) as image:
                actual_shape = (image.height, image.width)
            if actual_shape != image_shape:
                raise DecodeError("CWA PNG image dimensions disagree with provider metadata")
            return raw
        except BaseException:
            raw.close()
            raise

    def decode(self, raw: RawFrame, context: SourceContext) -> RadarField:
        if raw.ref.product != self.GRID_PRODUCT:
            return super().decode(raw, context)
        raw._ensure_open()
        context.cancellation.check()
        artifact = next((item for item in raw.artifacts if item.role == "data"), raw.artifacts[0])
        payload = artifact.payload if isinstance(artifact.payload, bytes) else artifact.payload.read_bytes()
        context.check_bytes(len(payload))
        try:
            document = json.loads(payload)
            if document["cwaopendata"]["dataid"] != self.GRID_KEY:
                raise ValueError("unexpected CWA product id")
            dataset = document["cwaopendata"]["dataset"]
            params = dataset["datasetInfo"]["parameterSet"]
            contents = dataset["contents"]
            width, height = int(params["GridDimensionX"]), int(params["GridDimensionY"])
            west, south, step = (float(params[key]) for key in ("StartPointLongitude", "StartPointLatitude", "GridResolution"))
            if params["Reflectivity"] != "dBZ" or "TWD67" not in contents["contentDescription"]:
                raise ValueError("unsupported CWA numeric grid units or datum")
            if width < 1 or height < 1 or not np.isfinite([west, south, step]).all() or step <= 0:
                raise ValueError("invalid CWA numeric grid dimensions or resolution")
            if parse_provider_time(params["DateTime"]) != raw.ref.valid_time:
                raise ValueError("CWA grid time changed after discovery")
            context.check_pixels(width * height * 2)
            # The provider declares the first value as the southwest point,
            # increasing east first, then north. Array row 0 is therefore south.
            values = np.fromstring(contents["content"], sep=",", dtype=np.float32)
            if values.size != width * height or not np.isfinite(values).all():
                raise ValueError("CWA numeric grid contains incomplete or non-finite values")
            values = values.reshape((height, width))
            quality = np.zeros(values.shape, dtype=np.uint16)
            quality[values == -99] = QUALITY_MISSING
            quality[values == -999] = QUALITY_OUTSIDE
            values[quality != 0] = np.nan
            longitude = west + step * np.arange(width, dtype=np.float64)
            latitude = south + step * np.arange(height, dtype=np.float64)
            grid = GeographicGrid(longitude, latitude, crs="EPSG:3821")
        except (KeyError, TypeError, ValueError, OverflowError) as exc:
            raise DecodeError(f"invalid CWA numeric reflectivity grid: {exc}") from exc
        coords = {"latitude": latitude, "longitude": longitude}
        data = xr.DataArray(values, dims=("latitude", "longitude"), coords=coords, name="reflectivity", attrs={"units": "dBZ", "standard_name": "equivalent_reflectivity_factor"})
        flags = xr.DataArray(quality, dims=data.dims, coords=coords, name="quality")
        return RadarField(data, grid, flags, provenance={"source": self.info.id, "product": raw.ref.product, "station": raw.ref.station, "valid_time": format_time(raw.ref.valid_time), "upstream_uri": raw.ref.uri, "native_crs": "EPSG:3821", "decoder": "cwa-O-A0059-001-v1", "missing_values": {"-99": "invalid", "-999": "outside_coverage_or_quality_removed"}})


__all__ = ["TwSource"]
