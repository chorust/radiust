from __future__ import annotations

import asyncio
import json
import subprocess
import sys

import pytest
from radiust import AsyncClient, Client, LocalViewRequest, _bridge
from radiust.config import load_config


def _offline_config(*, location: bool = True):
    values = {"runtime": {"allow_network": False}, "cache": {"enabled": False}}
    if location:
        values["location"] = {"longitude": 121.6, "latitude": 24.0}
    return load_config(values, environ={})


def test_local_view_models_import_in_a_fresh_interpreter():
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            "from radiust.models import LocalViewGrid, LocalViewRequest, LocalViewResult",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_local_view_request_defaults_and_explicit_overrides():
    default = LocalViewRequest().as_dict()
    assert default["center"] is None
    assert default["radius_km"] is None
    assert default["resolution_m"] is None
    assert default["mode"] == "auto"
    assert default["resampling"] == "nearest"

    explicit = LocalViewRequest(
        (121.6, 24.0), radius_km=25, resolution_m=250, source="rdcap", station="TWRCHL"
    ).as_dict()
    assert explicit["center"] == {"longitude": 121.6, "latitude": 24.0}
    assert explicit["radius_km"] == 25
    assert explicit["resolution_m"] == 250
    assert explicit["source"] == "rdcap"
    assert explicit["station"] == "TWRCHL"


@pytest.mark.parametrize(
    "center",
    [
        (121.6, 24.0, 5.0),
        (121.6,),
        {"lon": 121.6, "lat": 24.0},
        {"longitude": 121.6, "latitude": 24.0, "extra": 1},
        ("121.6", "24.0"),
        {"longitude": "121.6", "latitude": "24.0"},
        (True, False),
        "ab",
        (None, 24.0),
    ],
)
def test_local_view_request_rejects_malformed_center(center):
    with pytest.raises(ValueError):
        LocalViewRequest(center).as_dict()


def test_client_local_view_wraps_malformed_center_as_validation_error():
    from radiust.errors import UnsupportedQueryError

    with Client(config=_offline_config()) as client:
        with pytest.raises(UnsupportedQueryError) as caught:
            client.local_view(center=(121.6, 24.0, 5.0))
    assert caught.value.context.stage == "validate"


def test_location_environment_values_keep_origins():
    config = load_config(
        {"runtime": {"allow_network": False}},
        environ={
            "RADIUST_LOCATION__LONGITUDE": "122.25",
            "RADIUST_LOCATION__LATITUDE": "24.5",
            "RADIUST_LOCAL_VIEW__RADIUS_KM": "20",
        },
    )
    assert config.values["location"] == {"longitude": 122.25, "latitude": 24.5}
    assert config.values["local_view"]["radius_km"] == 20
    assert config.origins["location.longitude"] == "environment"
    assert config.origins["location.latitude"] == "environment"
    assert config.origins["local_view.radius_km"] == "environment"


def test_sync_and_async_local_view_share_offline_terminal_result():
    with Client(config=_offline_config()) as client:
        sync_result = client.local_view()

    async def run():
        async with AsyncClient(config=_offline_config()) as client:
            return await client.local_view()

    async_result = asyncio.run(run())
    assert sync_result.status == async_result.status == "no_candidate"
    assert sync_result.selected is async_result.selected is None
    assert sync_result.grid is async_result.grid is None
    assert sync_result.as_dict() == async_result.as_dict()
    assert sync_result.error == async_result.error
    assert sync_result.error["code"] == "network_disabled"
    assert sync_result.error["stage"] == "selection"
    assert sync_result.observation_coverage is None
    assert sync_result.selection
    for attempt in sync_result.selection:
        assert attempt["spatial_evidence"]["rank_reason"] == attempt["reason"]
        assert "observation_coverage" in attempt


def test_async_task_cancel_reaches_local_view_operation():
    class BlockingSession:
        def __init__(self):
            self.started = asyncio.Event()
            self.cancelled = False

        async def local_view(self, request):
            self.started.set()
            try:
                await asyncio.Event().wait()
            except asyncio.CancelledError:
                self.cancelled = True
                raise

    async def run():
        async with AsyncClient(config=_offline_config()) as client:
            session = BlockingSession()
            client._session = session
            task = asyncio.create_task(client.local_view())
            await session.started.wait()
            task.cancel()
            with pytest.raises(asyncio.CancelledError):
                await task
            assert session.cancelled

    asyncio.run(run())


@pytest.mark.parametrize("raw", [False, True])
def test_native_result_conversion_preserves_grid_masks_and_data_owner(raw, monkeypatch):
    selected_frame = object()
    field = object()

    class NativeRequest:
        def __init__(self, payload):
            self.payload = json.loads(payload)

    class NativeResult:
        field_count = 0 if raw else 1

        def metadata_json(self):
            return json.dumps(
                {
                    "status": "selected",
                    "selected": {
                        "source": "fixture",
                        "product": "reflectivity",
                        "station": "site-a",
                        "valid_time": "2026-10-10T00:00:00Z",
                    },
                    "grid": {
                        "center": {"longitude": 121.6, "latitude": 24.0},
                        "radius_m": 1000,
                        "requested_resolution_m": 1000,
                        "actual_resolution_m": 1000,
                        "shape": [2, 2],
                        "crs": "local-aeqd-wgs84-v1",
                        "affine": None,
                    },
                    "processing": {
                        "schema_version": 1,
                        "processing_id": "view-identity",
                        "input_logical_id": "source-frame-identity",
                        "input_receipts": [{"name": "frame.png", "sha256": "a" * 64}],
                        "requested_mode": "raw" if raw else "auto",
                        "data_kind": "raw_image" if raw else "numeric",
                    },
                    "selection": [],
                    "candidate_limit_reached": False,
                    "observation_coverage": {"kind": "unknown", "status": "unknown", "evidence": []},
                    "error": None,
                }
            )

        def x_axis(self):
            return [-1000.0, 0.0]

        def y_axis(self):
            return [1000.0, 0.0]

        def in_view_bytes(self):
            return b"\x01\x01\x01\x01"

        def source_domain_bytes(self):
            return b"\x01\x00\x01\x01"

        def selected(self):
            return selected_frame

        def field(self, index):
            assert index == 0
            return field

        def raw_image_rgba(self):
            return bytes(range(16)) if raw else None

    class NativeEngine:
        async def local_view(self, request):
            assert isinstance(request, NativeRequest)
            return NativeResult()

    class NativeCore:
        LocalViewRequest = NativeRequest

        @staticmethod
        def safety_value_json(payload):
            return json.dumps(json.loads(payload))

    session = object.__new__(_bridge.CoreEngineSession)
    session._engine = NativeEngine()
    monkeypatch.setattr(_bridge, "_core", NativeCore)
    result = asyncio.run(session.local_view(LocalViewRequest().as_dict()))

    assert result.status == "selected"
    assert result.grid.x == (-1000.0, 0.0)
    assert result.grid.y == (1000.0, 0.0)
    assert result.grid.in_view == b"\x01\x01\x01\x01"
    assert result.grid.source_domain == b"\x01\x00\x01\x01"
    if raw:
        assert result.raw_rgba == bytes(range(16))
        assert result.data == bytes(range(16))
    else:
        assert result.field() is field
    assert result.frame is selected_frame
    assert "x" not in result.as_dict()["grid"]
    assert result.processing["processing_id"] == "view-identity"
    assert result.as_dict()["processing"]["input_receipts"][0]["name"] == "frame.png"
    assert result.observation_coverage["status"] == "unknown"
    assert result.as_dict()["observation_coverage"]["kind"] == "unknown"
    with pytest.raises(TypeError):
        result.observation_coverage["status"] = "verified"


def test_native_result_conversion_failure_is_decode_error(monkeypatch):
    from types import SimpleNamespace
    from radiust.errors import DecodeError

    class NativeRequest:
        def __init__(self, payload):
            pass

    class NativeEngine:
        async def local_view(self, request):
            return SimpleNamespace(metadata_json=lambda: '{}', selected=lambda: None)

    session = object.__new__(_bridge.CoreEngineSession)
    session._engine = NativeEngine()
    monkeypatch.setattr(_bridge, "_core", SimpleNamespace(LocalViewRequest=NativeRequest))
    with pytest.raises(DecodeError) as caught:
        asyncio.run(session.local_view({}))
    assert caught.value.context.stage == "decode"
    assert caught.value.context.code == "invalid_local_view_result"


def test_raw_only_catalog_product_preserves_unknown_scientific_variables():
    from radiust.registry import registry

    product = next(product for product in registry.get_info("tw").products
                   if product.id == "observation-wide")
    assert product.variables == ()
    assert product.native_grid_kind == "pixel"
    assert not product.units
    assert product.metadata["scientific_decode"] == "blocked"
    assert product.metadata["spatial"]["observation_coverage"]["status"] == "unknown"
