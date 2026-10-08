from __future__ import annotations

import asyncio
import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

import pytest
from radiust import (
    AsyncClient,
    Client,
    ConfigError,
    FrameRef,
    ResourceLimitError,
    TransportError,
    UnsupportedQueryError,
    _bridge,
)
from radiust.models import Query

core = pytest.importorskip("radiust._core")


def _write_frame() -> FrameRef:
    return FrameRef(
        "my",
        "composite",
        datetime(2026, 9, 24, tzinfo=timezone.utc),
        station="east",
    )


def _write_field() -> Any:
    return core.RadarField(
        json.dumps(
            {
                "name": "reflectivity",
                "values": [1.5, 2.5, 3.5, 4.5],
                "shape": [2, 2],
                "quality": [0, 1, 2, 3],
                "units": "dBZ",
                "valid_time": "2026-09-24T00:00:00Z",
                "grid": {
                    "shape": [2, 2],
                    "crs": "EPSG:4326",
                    "x": [100.0, 101.0],
                    "y": [20.0, 21.0],
                    "affine": None,
                },
                "provenance": ["contract-fixture"],
            }
        )
    )


@pytest.mark.parametrize("format", ["png", "netcdf", "geotiff", "zarr"])
def test_rust_client_writes_memory_field_with_manifest_identity(tmp_path, format) -> None:
    frame = _write_frame()
    output_root = tmp_path / format

    with Client() as client:
        report = client.write(_write_field(), output=output_root, format=format, ref=frame)
        repeated = client.write(_write_field(), output=output_root, format=format, ref=frame)

    assert report.command == "write"
    assert report.counts["written"] == 1
    assert repeated.counts["skipped"] == 1
    item = report.items[0]
    assert item.ref.logical_id == frame.logical_id
    output = Path(item.output_uri)
    assert output.exists()
    manifest_path = output.with_name(output.name + ".manifest.json")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert manifest["logical_id"] == frame.logical_id
    assert manifest["raw_complete"] is False
    assert manifest["artifacts"]
    for artifact in manifest["artifacts"]:
        assert (output_root / artifact["relative_uri"]).is_file()


def test_memory_png_write_retains_render_options_and_identity(tmp_path):
    frame = _write_frame()
    with Client() as client:
        first = client.write(_write_field(), output=tmp_path, format="png", ref=frame,
                             vmin=0.0, vmax=10.0)
        repeated = client.write(_write_field(), output=tmp_path, format="png", ref=frame,
                                vmin=0.0, vmax=10.0)
    assert first.counts["written"] == repeated.counts["skipped"] == 1
    output = Path(first.items[0].output_uri)
    manifest = json.loads(Path(str(output) + ".manifest.json").read_text())
    assert manifest["processing_spec"]["options"]["vmax"] == 10.0
    sidecar = json.loads(output.with_suffix(".render.json").read_text())
    assert sidecar["vmin"] == 0.0
    assert sidecar["vmax"] == 10.0


def test_rust_client_write_requires_frame_identity() -> None:
    with Client() as client, pytest.raises(ValueError, match="requires the source FrameRef"):
        client.write(_write_field())


def test_rust_client_write_rejects_a_field_time_that_does_not_match_its_frame(tmp_path) -> None:
    frame = FrameRef(
        "my",
        "composite",
        datetime(2026, 9, 25, tzinfo=timezone.utc),
        station="east",
    )
    output = tmp_path / "mismatched-time"

    with Client() as client, pytest.raises(ValueError, match="frame and field valid_time differ"):
        client.write(_write_field(), output=output, ref=frame)

    assert not output.exists()


def test_rust_client_write_selects_dataset_variable(tmp_path) -> None:
    first = json.loads(_write_field().to_json())
    second = {
        **first,
        "name": "rain_rate",
        "values": [0.1, 0.2, 0.3, 0.4],
        "units": "mm/h",
    }
    dataset = core.RadarDataset(
        json.dumps(
            {
                "fields": [first, second],
                "valid_time": first["valid_time"],
                "source": "fixture",
            }
        )
    )
    frame = _write_frame()

    with Client() as client:
        report = client.write(
            dataset,
            output=tmp_path,
            format="netcdf",
            ref=frame,
            variable="rain_rate",
        )

    assert report.counts["written"] == 1
    manifest = json.loads(
        Path(report.items[0].output_uri)
        .with_name("decoded.nc.manifest.json")
        .read_text(encoding="utf-8")
    )
    assert manifest["processing_spec"]["variable"] == "rain_rate"


@pytest.mark.asyncio
async def test_async_rust_client_writes_memory_field(tmp_path) -> None:
    frame = _write_frame()
    async with AsyncClient() as client:
        report = await client.write(
            _write_field(), output=tmp_path / "async", format="netcdf", ref=frame
        )

    assert report.command == "write"
    assert report.counts["written"] == 1
    assert report.items[0].ref.logical_id == frame.logical_id
    assert Path(report.items[0].output_uri).is_file()


def test_native_engine_reuses_a_cancellable_core_lifetime() -> None:
    engine = core.Engine(json.dumps({"runtime": {"allow_network": False}}))

    assert engine.cancelled is False
    engine.cancel()
    assert engine.cancelled is True


def test_sync_client_cancel_stops_a_blocked_operation_from_another_thread() -> None:
    import queue
    import threading

    from radiust.config import load_config

    client_queue = queue.Queue()
    started = threading.Event()
    finished = threading.Event()
    outcome = {}

    def run_client_operation() -> None:
        client = Client(config=load_config(environ={}))
        client_queue.put(client)

        async def blocked_operation() -> None:
            started.set()
            await asyncio.Event().wait()

        try:
            client._run(blocked_operation())
            outcome["result"] = "returned"
        except asyncio.CancelledError:
            outcome["result"] = "cancelled"
        finally:
            client.close()
            finished.set()

    thread = threading.Thread(target=run_client_operation)
    thread.start()
    client = client_queue.get(timeout=2)
    assert started.wait(timeout=2)
    client.cancel()
    thread.join(timeout=2)

    assert not thread.is_alive()
    assert finished.is_set()
    assert outcome["result"] == "cancelled"


def test_native_sync_errors_map_to_sdk_domain_errors() -> None:
    with pytest.raises(ResourceLimitError, match="payload size") as caught:
        _bridge.validate_size(5, 4)

    assert caught.value.stage == "acquire"
    assert isinstance(caught.value.cause, ValueError)


@pytest.mark.asyncio
async def test_native_engine_discovery_returns_a_typed_report_and_frame() -> None:
    report = await _bridge.native_discover_report(
        {"runtime": {"allow_network": True}}, {"source": "fr"}
    )

    assert report.total == 1
    assert report.interrupted is False
    serialized = json.loads(report.to_json())
    assert serialized["counts"]["success"] == 1
    frame = report.frame(0)
    del report
    assert frame.source == "fr"
    assert frame.product == "composite"
    assert frame.station == "FRCOMP"
    assert len(frame.logical_id) == 64
    assert "locator" not in json.loads(frame.to_json())

    decoded = await _bridge.native_discover(
        {"runtime": {"allow_network": True}}, {"source": "fr"}
    )
    assert decoded["counts"]["success"] == 1


def test_public_query_serialization_preserves_core_time_filters() -> None:
    query = Query(
        "fr",
        latest=True,
        base_time=datetime(2026, 9, 24, 6, 30, tzinfo=timezone.utc),
        max_age=timedelta(minutes=5),
    )

    payload = _bridge._native_query_payload(query)

    assert payload["selector"] == {"kind": "latest"}
    assert payload["base_time"] == "2026-09-24T06:30:00+00:00"
    assert payload["max_age_secs"] == 300

    ranged = Query(
        "fr",
        start=datetime(2026, 9, 24, 6, tzinfo=timezone.utc),
        end=datetime(2026, 9, 24, 7, tzinfo=timezone.utc),
    )
    assert _bridge._native_query_payload(ranged)["selector"] == {
        "kind": "range",
        "start": "2026-09-24T06:00:00+00:00",
        "end": "2026-09-24T07:00:00+00:00",
    }


@pytest.mark.asyncio
async def test_native_engine_session_reuses_one_cancellable_engine() -> None:
    session = _bridge.CoreEngineSession({"runtime": {"allow_network": True}})

    first = await session.discover_report(Query("fr", latest=True))
    second = await session.discover_report(Query("fr", latest=True))

    assert first.total == second.total == 1
    assert first.frame(0).logical_id == second.frame(0).logical_id
    assert session.cancelled is False
    session.cancel()
    assert session.cancelled is True


@pytest.mark.asyncio
async def test_native_engine_session_exposes_sanitized_operation_events() -> None:
    session = _bridge.CoreEngineSession({"runtime": {"allow_network": False}})
    events = session.subscribe_events()

    await session.discover_report(Query("fr", latest=True))
    payload = json.loads(events.drain_json())

    assert [event["stage"] for event in payload][0] == "started"
    assert payload[-1]["stage"] == "completed"
    assert all(event["operation"] == "discover" for event in payload)
    assert all(
        set(event) <= {"operation_id", "operation", "stage", "progress"}
        for event in payload
    )
    assert events.dropped_events == 0


@pytest.mark.asyncio
async def test_native_binding_errors_expose_safe_code_stage_and_retryability() -> None:
    from radiust import FrameRef

    frame = _bridge._native_frames(
        [
            FrameRef(
                "fr",
                "composite",
                datetime(2026, 9, 24, tzinfo=timezone.utc),
                station="FRCOMP",
            )
        ]
    )[0]
    session = _bridge.CoreEngineSession(
        {"runtime": {"allow_network": False}, "cache": {"enabled": False}}
    )

    with pytest.raises(UnsupportedQueryError) as caught:
        await session.fetch_raw(frame)

    assert caught.value.context.code == "network_restricted"
    assert caught.value.context.stage == "acquire"
    assert caught.value.context.retryable is False
    assert "https://" not in str(caught.value)


def test_sync_client_progress_uses_rust_engine_discovery_events() -> None:
    events: list[tuple[str, int, int | None]] = []
    with (
        Client(config={"runtime": {"allow_network": False}}) as client,
        pytest.raises(TransportError, match="network access is disabled"),
    ):
        client.discover(
            Query("fr", latest=True), progress=lambda *event: events.append(event)
        )
    assert events[0] == ("discover", 0, None)
    assert any(stage == "discover_targets" and total == 1 for stage, _, total in events)
    assert ("discover", 0, 0) not in events


@pytest.mark.asyncio
async def test_async_client_progress_callbacks_are_isolated_across_concurrent_calls() -> None:
    first: list[tuple[str, int, int | None]] = []
    second: list[tuple[str, int, int | None]] = []
    async with AsyncClient(config={"runtime": {"allow_network": False}}) as client:
        results = await asyncio.gather(
            client.discover(Query("fr", latest=True), progress=lambda *event: first.append(event)),
            client.discover(Query("fr", latest=True), progress=lambda *event: second.append(event)),
            return_exceptions=True,
        )

    assert all(isinstance(result, TransportError) for result in results)
    for events in (first, second):
        assert events.count(("discover", 0, None)) == 1
        assert ("discover", 0, 0) not in events
        assert sum(stage == "discover_targets" for stage, _, _ in events) == 1


@pytest.mark.asyncio
async def test_async_client_operations_without_progress_remain_concurrent(monkeypatch) -> None:
    import radiust.rust_client as rust_client

    both_entered = asyncio.Event()
    release = asyncio.Event()
    active = 0
    maximum_active = 0

    async def delayed_discover(_session, _query):
        nonlocal active, maximum_active
        active += 1
        maximum_active = max(maximum_active, active)
        if active == 2:
            both_entered.set()
        await asyncio.wait_for(release.wait(), timeout=1)
        active -= 1
        return []

    monkeypatch.setattr(rust_client, "_discover_frames", delayed_discover)
    async with AsyncClient(config={"runtime": {"allow_network": False}}) as client:
        first = asyncio.create_task(client.discover(Query("fr", latest=True)))
        second = asyncio.create_task(client.discover(Query("fr", latest=True)))
        await asyncio.wait_for(both_entered.wait(), timeout=1)
        release.set()
        assert await asyncio.gather(first, second) == [[], []]

    assert maximum_active == 2


@pytest.mark.asyncio
async def test_native_fetch_maps_network_opt_in_to_permission_error(tmp_path) -> None:
    temp_root = tmp_path / "temporary"
    output = tmp_path / "output"
    engine = core.Engine(
        json.dumps(
            {
                "runtime": {"allow_network": False, "temp_root": str(temp_root)},
                "storage": {"output": str(output)},
            }
        )
    )
    frame = core.FrameRef(
        json.dumps(
            {
                "source": "my",
                "product": "composite",
                "station": "east",
                "valid_time": "2026-09-24T00:00:00Z",
                "logical_id": "not-used-before-network-check",
            }
        )
    )

    with pytest.raises(PermissionError, match="network access"):
        await engine.fetch_raw(frame)

    assert not temp_root.exists()
    assert not output.exists()


@pytest.mark.asyncio
async def test_native_batch_fetch_dry_run_keeps_frame_order_and_statuses() -> None:
    frames = [
        core.FrameRef(
            json.dumps(
                {
                    "source": source,
                    "product": "composite",
                    "station": station,
                    "valid_time": "2026-09-24T00:00:00Z",
                    "logical_id": f"{source}-{station}",
                }
            )
        )
        for source, station in (("vn", "VN01"), ("my", "east"), ("ca", "CA01"))
    ]

    report = await _bridge.native_fetch_many_raw(
        {"runtime": {"allow_network": False}},
        frames,
        on_error="continue",
        dry_run=True,
    )

    assert report.total == 3
    assert report.planned == 3
    assert report.success == report.failed == report.cancelled == report.not_started == 0
    assert [report.item(index).input_index for index in range(report.total)] == [0, 1, 2]
    assert [report.item(index).frame().source for index in range(report.total)] == [
        "vn",
        "my",
        "ca",
    ]
    assert all(report.item(index).status == "planned" for index in range(report.total))
    assert all(report.item(index).raw() is None for index in range(report.total))
    serialized = json.loads(report.to_json())
    assert serialized["planned"] == 3
    assert [item["frame"]["station"] for item in serialized["items"]] == [
        "VN01",
        "east",
        "CA01",
    ]

    with pytest.raises(UnsupportedQueryError, match="on_error"):
        await _bridge.native_fetch_many_raw(
            {"runtime": {"allow_network": False}},
            frames,
            on_error="invalid",
            dry_run=True,
        )


@pytest.mark.asyncio
async def test_native_batch_fetch_stop_returns_deterministic_partial_results() -> None:
    frames = [
        {
            "source": source,
            "product": "composite",
            "station": station,
            "valid_time": "2026-09-24T00:00:00Z",
            "logical_id": f"{source}-{station}",
        }
        for source, station in (("vn", "VN01"), ("my", "east"), ("ca", "CA01"))
    ]
    config = {"runtime": {"allow_network": False, "frame_concurrency": 1}}

    snapshots = []
    for _ in range(3):
        report = await _bridge.native_fetch_many_raw(
            config, frames, on_error="stop", dry_run=False
        )
        snapshots.append(
            (
                [report.item(index).input_index for index in range(report.total)],
                [report.item(index).status for index in range(report.total)],
                [report.item(index).error for index in range(report.total)],
            )
        )
        assert report.total == 3
        assert report.success == 0
        assert report.failed == 1
        assert report.not_started == 2
        assert report.cancelled == report.planned == 0
        assert all(report.item(index).raw() is None for index in range(report.total))

    assert snapshots[0] == snapshots[1] == snapshots[2]
    assert snapshots[0][0] == [0, 1, 2]
    assert snapshots[0][1] == ["failed", "not_started", "not_started"]
    assert "network access" in snapshots[0][2][0]


@pytest.mark.asyncio
async def test_native_download_bindings_return_ordered_plans_without_writing(tmp_path) -> None:
    output = tmp_path / "output"
    config = {
        "runtime": {"allow_network": False},
        "storage": {"output": str(output)},
    }
    frames = [
        {
            "source": source,
            "product": product,
            "station": None,
            "valid_time": "2026-09-24T00:00:00Z",
            "logical_id": f"{source}-{product}",
        }
        for source, product in (("rainviewer", "composite"), ("tw", "grid"))
    ]

    raw_report = await _bridge.native_download_raw_only(
        config, frames, on_error="continue", dry_run=True
    )
    session = _bridge.CoreEngineSession(config)
    png_report = await session.download_decoded(
        frames, format="png", on_error="collect", dry_run=True
    )
    netcdf_report = await session.download_decoded(
        frames, format="netcdf", on_error="stop", dry_run=True
    )
    geotiff_report = await session.download_decoded(
        frames, format="geotiff", on_error="collect", dry_run=True
    )
    zarr_report = await session.download_decoded(
        frames, format="zarr", on_error="continue", dry_run=True
    )
    processed_report = await session.download_decoded(
        frames, format="netcdf",
        dry_run=True,
        processing={
            "variable": "reflectivity",
            "grid": "geographic",
            "bbox": [-1.0, -1.0, 1.0, 1.0],
            "resolution": 1.0,
            "resampling": "bilinear",
        },
    )

    for report in (
        raw_report,
        png_report,
        netcdf_report,
        geotiff_report,
        zarr_report,
        processed_report,
    ):
        assert report.total == report.planned == 2
        assert report.written == report.failed == report.not_started == 0
        assert [report.item(index).input_index for index in range(report.total)] == [0, 1]
        assert [report.item(index).status for index in range(report.total)] == [
            "planned",
            "planned",
        ]
        assert [report.item(index).frame().source for index in range(report.total)] == [
            "rainviewer",
            "tw",
        ]

    templated = await session.download_decoded(
        frames,
        format="png",
        on_error="continue",
        dry_run=True,
        output_template="{source}/{date}/{product}_{hour}.{ext}",
    )
    assert templated.total == templated.planned == 2
    assert not output.exists()

    with pytest.raises(UnsupportedQueryError, match="on_error"):
        await session.download_decoded(frames, format="png", on_error="invalid", dry_run=True)
    with pytest.raises(UnsupportedQueryError, match="on_error"):
        await session.download_decoded(
            frames, format="netcdf", on_error="invalid", dry_run=True
        )
    with pytest.raises(UnsupportedQueryError, match="on_error"):
        await session.download_decoded(
            frames, format="geotiff", on_error="invalid", dry_run=True
        )
    with pytest.raises(UnsupportedQueryError, match="on_error"):
        await session.download_decoded(
            frames, format="zarr", on_error="invalid", dry_run=True
        )
    with pytest.raises(ConfigError, match="bbox and resolution require grid=geographic"):
        await session.download_decoded(
            frames, format="netcdf",
            dry_run=True,
            processing={"bbox": [-1.0, -1.0, 1.0, 1.0]},
        )


def test_native_radar_field_converts_to_xarray_only_when_requested() -> None:
    import struct

    np = pytest.importorskip("numpy")
    xr = pytest.importorskip("xarray")
    from radiust import to_xarray

    field_document = {
        "name": "reflectivity",
        "values": [1.5, 2.5, 3.5, 4.5],
        "shape": [2, 2],
        "quality": [0, 1, 2, 3],
        "units": "dBZ",
        "valid_time": "2026-09-24T00:00:00Z",
        "grid": {
            "shape": [2, 2],
            "crs": "EPSG:4326",
            "x": [100.0, 101.0],
            "y": [20.0, 21.0],
            "affine": None,
        },
        "provenance": ["fixture"],
    }
    field = core.RadarField(json.dumps(field_document))

    converted = to_xarray(field)

    assert isinstance(converted, xr.DataArray)
    assert converted.dtype == np.float32
    assert converted.dims == ("y", "x")
    assert converted.attrs["units"] == "dBZ"
    assert converted.attrs["crs"] == "EPSG:4326"
    assert converted.attrs["valid_time"] == "2026-09-24T00:00:00Z"
    np.testing.assert_array_equal(converted.coords["x"].values, [100.0, 101.0])
    np.testing.assert_array_equal(converted.coords["y"].values, [20.0, 21.0])
    assert converted.coords["quality"].dtype == np.uint16
    np.testing.assert_array_equal(converted.values, [[1.5, 2.5], [3.5, 4.5]])
    np.testing.assert_array_equal(converted.coords["quality"].values, [[0, 1], [2, 3]])

    class MissingValueBinding:
        def to_json(self) -> str:
            return field.to_json()

        def metadata_json(self) -> str:
            return field.metadata_json()

        @staticmethod
        def values_le_bytes() -> bytes:
            return struct.pack("<4f", 1.5, 2.5, float("nan"), 4.5)

        @staticmethod
        def quality_le_bytes() -> bytes:
            return struct.pack("<4H", 0, 0, 1, 0)

    missing_field = MissingValueBinding()
    converted_missing = to_xarray(missing_field)
    assert np.isnan(converted_missing.values[1, 0])
    assert converted_missing.coords["quality"].values[1, 0] == 1

    second_field = {
        **field_document,
        "name": "rain_rate",
        "values": [0.1, 0.2, 0.3, 0.4],
        "units": "mm/h",
    }
    dataset = core.RadarDataset(
        json.dumps(
            {
                "fields": [field_document, second_field],
                "valid_time": "2026-09-24T00:00:00Z",
                "source": "fixture",
            }
        )
    )
    converted_dataset = to_xarray(dataset)
    assert isinstance(converted_dataset, xr.Dataset)
    assert converted_dataset.attrs["source"] == "fixture"
    assert converted_dataset.attrs["valid_time"] == "2026-09-24T00:00:00Z"
    assert set(converted_dataset.data_vars) == {
        "reflectivity",
        "reflectivity_quality",
        "rain_rate",
        "rain_rate_quality",
    }
    assert converted_dataset["rain_rate_quality"].dtype == np.uint16
    retained_field = dataset.field(0)
    del dataset
    assert retained_field.name == "reflectivity"
    assert retained_field.values_le_bytes() == field.values_le_bytes()


def test_native_dataset_rejects_fields_with_different_valid_times() -> None:
    field_document = {
        "name": "reflectivity",
        "values": [1.5, 2.5, 3.5, 4.5],
        "shape": [2, 2],
        "quality": [0, 1, 2, 3],
        "units": "dBZ",
        "valid_time": "2026-09-24T00:00:00Z",
        "grid": {
            "shape": [2, 2],
            "crs": "EPSG:4326",
            "x": [100.0, 101.0],
            "y": [20.0, 21.0],
            "affine": None,
        },
        "provenance": ["fixture"],
    }
    second_field = {
        **field_document,
        "name": "rain_rate",
        "valid_time": "2026-09-24T00:05:00Z",
    }

    with pytest.raises(ValueError, match="dataset fields must share valid_time"):
        core.RadarDataset(
            json.dumps(
                {
                    "fields": [field_document, second_field],
                    "valid_time": field_document["valid_time"],
                    "source": "fixture",
                }
            )
        )


def test_to_xarray_reports_missing_optional_science_dependencies(monkeypatch) -> None:
    import sys

    from radiust import to_xarray

    monkeypatch.setitem(sys.modules, "numpy", None)
    with pytest.raises(ImportError, match=r"to_xarray\(\) requires NumPy and xarray"):
        to_xarray(object())


@pytest.mark.asyncio
async def test_native_engine_regrids_in_rust_and_preserves_quality_flags() -> None:
    import numpy as np

    field_document = {
        "name": "reflectivity",
        "values": [0.0, 10.0, 20.0, 30.0],
        "shape": [2, 2],
        "quality": [0, 0, 0, 0],
        "units": "dBZ",
        "valid_time": "2026-09-24T00:00:00Z",
        "grid": {
            "shape": [2, 2],
            "crs": "EPSG:4326",
            "x": [100.0, 101.0],
            "y": [20.0, 21.0],
            "affine": None,
        },
        "provenance": ["source=fixture"],
    }
    target = {
        "shape": [1, 1],
        "crs": "EPSG:4326",
        "x": [100.5],
        "y": [20.5],
        "affine": None,
    }

    result = await _bridge.native_regrid(
        {"runtime": {"allow_network": False}}, field_document, target, method="bilinear"
    )

    expected = 10.0 * np.log10((1.0 + 10.0 + 100.0 + 1000.0) / 4.0)
    assert result.shape == [1, 1]
    assert abs(result.copy_values()[0] - expected) < 1.0e-5
    assert np.frombuffer(result.quality_le_bytes(), dtype="<u2").tolist() == [16]
    metadata = json.loads(result.metadata_json())
    assert metadata["grid"]["x"] == [100.5]
    assert "operation=regrid,resampling=bilinear" in metadata["provenance"]


def test_public_clients_fetch_a_rust_field_without_formal_download(monkeypatch) -> None:
    import asyncio
    import struct

    from radiust import AsyncClient, Client, FrameRef, rust_client

    frame = _bridge._native_frames(
        [FrameRef("rainviewer", "composite", datetime(2026, 9, 24, tzinfo=timezone.utc))]
    )[0]
    field_document = {
        "name": "reflectivity",
        "values": [1.0],
        "shape": [1, 1],
        "quality": [0],
        "units": "dBZ",
        "valid_time": "2026-09-24T00:00:00Z",
        "grid": {
            "shape": [1, 1],
            "crs": "EPSG:4326",
            "x": [100.0],
            "y": [20.0],
            "affine": None,
        },
        "provenance": ["fixture"],
    }
    calls: list[str] = []

    class FakeSession:
        def __init__(self, _config):
            pass

        async def fetch_raw(self, got_frame):
            assert got_frame.source == "rainviewer"
            calls.append("fetch_raw")
            return object()

        async def decode_science(self, _raw):
            calls.append("decode_science")
            return core.RadarField(json.dumps(field_document))

        def cancel(self):
            calls.append("cancel")

    async def discover(_session, _query):
        calls.append("discover")
        return [frame]

    monkeypatch.setattr(rust_client._bridge, "CoreEngineSession", FakeSession)
    monkeypatch.setattr(rust_client, "_discover_frames", discover)

    with Client(config={"runtime": {"allow_network": False}}) as client:
        sync_value = client.fetch(Query("rainviewer", latest=True))

    async def fetch_async() -> Any:
        async with AsyncClient(config={"runtime": {"allow_network": False}}) as client:
            return await client.fetch(Query("rainviewer", latest=True))

    async_value = asyncio.run(fetch_async())

    for fetched in (sync_value, async_value):
        assert isinstance(fetched, core.RadarField)
        assert fetched.values_le_bytes() == struct.pack("<f", 1.0)
        assert fetched.quality_le_bytes() == struct.pack("<H", 0)
        metadata = json.loads(fetched.metadata_json())
        assert metadata["name"] == "reflectivity"
        assert metadata["units"] == "dBZ"
        assert metadata["grid"]["crs"] == "EPSG:4326"
    assert calls.count("fetch_raw") == calls.count("decode_science") == 2
    assert calls.count("discover") == 2
    assert not {"download", "commit"}.intersection(calls)


def test_public_sdk_raw_option_is_accepted_by_rust_download_dispatch(tmp_path) -> None:
    from datetime import datetime, timezone

    from radiust import Client, FrameRef

    frame = FrameRef(
        "rainviewer",
        "composite",
        datetime(2026, 9, 24, tzinfo=timezone.utc),
        revision="offline-contract",
    )
    output = tmp_path / "offline-raw"
    with Client(config={"runtime": {"allow_network": False}}) as client:
        report = client.download(
            [frame],
            output=output,
            format="netcdf",
            raw=True,
            variable="reflectivity",
            grid="geographic",
            bbox=(-1.0, -1.0, 1.0, 1.0),
            resolution=1.0,
            resampling="bilinear",
        )

    # The source fetch is correctly rejected by the network policy. Reaching a
    # Rust DownloadReport (instead of the old blanket UnsupportedQueryError)
    # proves the public raw option crossed the SDK binding boundary.
    assert report.command == "download"
    assert report.counts["failed"] == 1
    assert not list(output.rglob("*.manifest.json"))


def test_public_rust_clients_preserve_single_frame_ambiguity(monkeypatch) -> None:
    import asyncio
    from datetime import timedelta

    from radiust import AmbiguousFrameError, AsyncClient, Client, FrameRef, rust_client

    frames = _bridge._native_frames(
        [
            FrameRef("rainviewer", "composite", datetime(2026, 9, 24, tzinfo=timezone.utc)),
            FrameRef(
                "rainviewer",
                "composite",
                datetime(2026, 9, 24, tzinfo=timezone.utc) + timedelta(minutes=5),
            ),
        ]
    )

    class FakeSession:
        def __init__(self, _config):
            pass

    async def discover(_session, _query):
        return frames

    monkeypatch.setattr(rust_client._bridge, "CoreEngineSession", FakeSession)
    monkeypatch.setattr(rust_client, "_discover_frames", discover)
    with (
        Client(config={"runtime": {"allow_network": False}}) as client,
        pytest.raises(AmbiguousFrameError, match="returned 2 frames"),
    ):
        client.fetch(Query("rainviewer", latest=True))

    async def fetch_async() -> None:
        async with AsyncClient(config={"runtime": {"allow_network": False}}) as client:
            with pytest.raises(AmbiguousFrameError, match="returned 2 frames"):
                await client.fetch(Query("rainviewer", latest=True))

    asyncio.run(fetch_async())


def test_public_fetch_many_keeps_rust_partial_results_and_concurrency() -> None:
    from radiust import BatchError, Client, FrameRef

    frames = [
        FrameRef("fr", "composite", datetime(2026, 9, 24, tzinfo=timezone.utc), station="FRCOMP"),
        FrameRef("my", "composite", datetime(2026, 9, 24, tzinfo=timezone.utc), station="east"),
    ]
    with (
        Client(config={"runtime": {"allow_network": False}}) as client,
        pytest.raises(BatchError) as caught,
    ):
        client.fetch_many(frames, on_error="raise", max_concurrency=1)

    partial = caught.value.partial_result
    assert partial.counts["failed"] == 1
    assert partial.counts["not_started"] == 1
    assert [item.ref.source for item in partial.items] == ["fr", "my"]


@pytest.mark.asyncio
async def test_engine_download_output_root_is_selected_per_call(tmp_path) -> None:
    from radiust import FrameRef

    configured = tmp_path / "configured"
    selected = tmp_path / "selected"
    native_frame = _bridge._native_frames(
        [
            FrameRef(
                "fr",
                "composite",
                datetime(2026, 9, 24, tzinfo=timezone.utc),
                station="FRCOMP",
            )
        ]
    )[0]
    engine = core.Engine(
        json.dumps(
            {
                "runtime": {"allow_network": False},
                "storage": {"output": str(configured)},
            }
        )
    )

    report = await engine.download_raw_only(
        [native_frame], "collect", False, False, str(selected)
    )

    assert report.failed == 1
    assert selected.is_dir()
    assert not configured.exists()
