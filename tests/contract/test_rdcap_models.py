from __future__ import annotations

from datetime import datetime, timezone

import pytest
from radiust.errors import UnsupportedQueryError
from radiust.models import (
    DiscoveryItem,
    DiscoveryReport,
    DiscoveryTarget,
    FrameRef,
    Query,
    SourceInfo,
    StationInfo,
)
from radiust.registry import _info_from_dict


def test_rdcap_query_accepts_country_station_or_short_code_and_rejects_path_aliases():
    for station in ("TWRCHL", "JPMAKI", "PHSUBI"):
        assert Query("rdcap", stations=(station,), latest=True).stations == (station,)
    assert Query("rdcap", stations=("RCHL",), latest=True).stations == ("RCHL",)
    for invalid in ("JPN/MAKI", "TWN/RCHL", "TWN/../RCHL", "USA/RCHL", "TWN/RCH/L", "TWN%2FRCHL"):
        with pytest.raises(UnsupportedQueryError, match="RDCAP station"):
            Query("rdcap", stations=(invalid,), latest=True)


def test_rdcap_station_identity_is_scoped_to_source_metadata_and_source_info():
    station = StationInfo(
        id="TWRCHL",
        name="Hua-Lien",
        longitude=None,
        latitude=None,
        product_ids=("reflectivity",),
        metadata={"source_id": "rdcap", "country": "TWN"},
    )
    source = SourceInfo(
        id="rdcap", description="RDCAP", adapter_version="1", products=(), stations=(station,)
    )
    assert source.stations[0].longitude is None
    assert source.stations[0].latitude is None
    with pytest.raises(ValueError, match="does not match"):
        SourceInfo(id="ph", description="PAGASA", adapter_version="1", products=(), stations=(station,))

    with pytest.raises(ValueError, match="path separator"):
        StationInfo(
            id="TWN/RCHL", name="wrong namespace", longitude=1, latitude=2,
            metadata={"source_id": "ph"},
        )


def test_registry_preserves_nested_metadata_and_unknown_coordinates():
    info = _info_from_dict(
        {
            "id": "rdcap",
            "description": "RDCAP",
            "adapter_version": "1",
            "metadata": {"country": "TWN"},
            "products": [
                {"id": "reflectivity", "variables": ["reflectivity"], "metadata": {"recent_query": {"latest": True}}}
            ],
            "stations": [
                {
                    "id": "TWRCHL",
                    "name": "Hua-Lien",
                    "longitude": None,
                    "latitude": None,
                    "product_ids": ["reflectivity"],
                    "metadata": {"country": "TWN"},
                }
            ],
        }
    )
    assert info.metadata["country"] == "TWN"
    assert info.products[0].metadata["recent_query"]["latest"] is True
    assert info.stations[0].metadata["source_id"] == "rdcap"
    assert info.stations[0].longitude is info.stations[0].latitude is None


@pytest.mark.parametrize("station", ["TWRCHL", "JPMAKI", "PHSUBI"])
def test_rdcap_frames_require_canonical_station_ids_and_targets_accept_selectors(station):
    valid_time = datetime(2026, 10, 1, tzinfo=timezone.utc)
    frame = FrameRef("rdcap", "reflectivity", valid_time, station=station)
    assert frame.station == station
    target = DiscoveryTarget("rdcap", "reflectivity", station)
    assert target.station == station
    short_target = DiscoveryTarget("rdcap", "reflectivity", "RCHL")
    assert DiscoveryItem(short_target, "ambiguous").target.station == "RCHL"
    with pytest.raises(ValueError, match="country-qualified"):
        DiscoveryItem(short_target, "success", "2026-10-01T00:00:00Z", frame={
            "source": "rdcap", "product": "reflectivity", "station": "RCHL",
            "valid_time": "2026-10-01T00:00:00Z",
        })
    for invalid in ("RCHL", "JPN/MAKI", "JP", "JPmaki", "JPMA-KI", "JP雷達", "USMAKI"):
        with pytest.raises(ValueError, match="country-qualified"):
            FrameRef("rdcap", "reflectivity", valid_time, station=invalid)
    with pytest.raises(ValueError, match="country-qualified"):
        DiscoveryTarget("rdcap", "reflectivity", "TWN/../RCHL")


@pytest.mark.parametrize("selector", ["NOPE", "RCHL"])
def test_rdcap_failure_reports_preserve_short_selectors_with_successful_frames(selector):
    report = DiscoveryReport.from_mapping({"items": [
        {
            "source": "rdcap", "product": "reflectivity", "station": selector,
            "status": "upstream_failed", "error": {"code": "catalog_unavailable"},
        },
        {
            "source": "rdcap", "product": "reflectivity", "station": "TWRCHL",
            "status": "success", "valid_time": "2026-10-01T00:00:00Z",
            "frame": {
                "source": "rdcap", "product": "reflectivity", "station": "TWRCHL",
                "valid_time": "2026-10-01T00:00:00Z", "base_time": None,
            },
        },
    ]})
    assert report.counts["success"] == report.counts["upstream_failed"] == 1
    failure = next(item for item in report.as_dict()["items"] if item["status"] != "success")
    assert failure["station"] == selector
    assert failure["error"]["code"] == "catalog_unavailable"


def test_rdcap_xarray_quality_keeps_annotation_bit_and_coordinates():
    import json
    import struct

    import numpy as np
    from radiust.science_adapters import to_xarray

    class NativeField:
        def to_json(self):
            return json.dumps({"source": "rdcap", "valid_time": "2026-10-01T00:00:00.000000Z"})

        def metadata_json(self):
            return json.dumps(
                {
                    "name": "reflectivity",
                    "shape": [1, 1],
                    "valid_time": "2026-10-01T00:00:00.000000Z",
                    "units": "dBZ",
                    "grid": {
                        "crs": "EPSG:4326",
                        "x": [121.0],
                        "y": [24.0],
                        "affine": [120.5, 1.0, 0.0, 24.5, 0.0, -1.0],
                    },
                    "provenance": ["source=rdcap", "station=TWRCHL"],
                }
            )

        def values_le_bytes(self):
            return struct.pack("<f", float("nan"))

        def quality_le_bytes(self):
            return struct.pack("<H", 65)

    result = to_xarray(NativeField())
    assert result.attrs["crs"] == "EPSG:4326"
    assert result.coords["quality"].dtype == np.dtype("uint16")
    assert result.coords["quality"].values.tolist() == [[65]]
    assert result.coords["quality"].attrs["flag_masks"].tolist() == [1, 2, 4, 8, 16, 32, 64]
    assert result.coords["quality"].attrs["flag_meanings"].endswith("source_annotation")
