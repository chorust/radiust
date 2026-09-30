"""Retained Taiwan CWA numeric-grid frame for Rust Engine offline contracts."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from pathlib import Path

from radiust.models import FrameRef

from tests.support.native_raw_cache import seed_native_raw_cache

FIXTURE_ROOT = Path(__file__).parents[1] / "fixtures/sources/tw"
_BASE_PROVIDER_TIME = b"2026-09-20T12:30:00+08:00"


def tw_grid_ref(valid_time: datetime | None = None) -> FrameRef:
    at = valid_time or datetime(2026, 9, 20, 4, 30, tzinfo=timezone.utc)
    revision = f"O-A0059-001-{int(at.timestamp())}"
    return FrameRef(
        "tw",
        "grid",
        at,
        station="CV1_3600",
        locator={
            "url": "https://cwaopendata.s3.ap-northeast-1.amazonaws.com/Observation/O-A0059-001.json",
            "name": "O-A0059-001.json",
            "media_type": "application/json",
            "artifacts": [],
            "station": "CV1_3600",
            "revision": revision,
            "time_semantics": "provider_grid_time",
            "geometry_status": "provider_native_twd67",
            "native_crs": "EPSG:3821",
            "grid_dimension": [881, 921],
            "grid_origin": [115.0, 18.0],
            "grid_resolution": 0.0125,
        },
        locator_version="tw-cwa-v2",
        revision=revision,
    )


def seed_tw_grid_cache(cache_root: Path, ref: FrameRef) -> None:
    provider_time = (ref.valid_time + timedelta(hours=8)).strftime("%Y-%m-%dT%H:%M:%S+08:00")
    payload = (FIXTURE_ROOT / "raw/O-A0059-001.json").read_bytes().replace(
        _BASE_PROVIDER_TIME, provider_time.encode("ascii")
    )
    seed_native_raw_cache(
        cache_root,
        ref,
        [("O-A0059-001.json", "application/json", payload)],
    )


def read_tw_grid_raw(ref: FrameRef) -> bytes:
    provider_time = (ref.valid_time + timedelta(hours=8)).strftime("%Y-%m-%dT%H:%M:%S+08:00")
    return (FIXTURE_ROOT / "raw/O-A0059-001.json").read_bytes().replace(
        _BASE_PROVIDER_TIME, provider_time.encode("ascii")
    )


def tw_offline_config(cache_root: Path, output_root: Path, temp_root: Path) -> dict[str, object]:
    return {
        "runtime": {"allow_network": False, "temp_root": str(temp_root)},
        "cache": {"enabled": True, "dir": str(cache_root)},
        "storage": {"output": str(output_root)},
    }
