from __future__ import annotations

import asyncio
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

import pytest
from radiust import AsyncClient, BatchError, Client, FrameRef, Query, _core, rust_client
from radiust.config import EffectiveConfig
from radiust.errors import ErrorContext
from radiust.identity import safe_ref


class NativeFrame:
    source = "rdcap"
    product = "reflectivity"
    station = "TWN/RCHL"
    logical_id = "a" * 64
    revision = None

    def to_json(self) -> str:
        return json.dumps(
            {
                "source": self.source,
                "product": self.product,
                "station": self.station,
                "valid_time": "2026-10-01T06:05:08.000000Z",
                "logical_id": "a" * 64,
                "locator_version": "rdcap-csr-v1",
                "locator": {"url": "https://rdcap.cwa.gov.tw/file?ft=private-ticket"},
            }
        )


class NativeReport:
    def __init__(self) -> None:
        self._frame = NativeFrame()

    def to_json(self) -> str:
        return json.dumps(
            {
                "schema_version": 1,
                "query": {
                    "source": "rdcap",
                    "sources": [],
                    "product": "reflectivity",
                    "stations": ["TWN/RCHL"],
                    "selector": {"kind": "latest"},
                    "max_age_secs": None,
                },
                "counts": {"total": 1, "success": 1},
                "items": [
                    {
                        "target": {
                            "source": "rdcap",
                            "product": "reflectivity",
                            "station": "TWN/RCHL",
                        },
                        "status": "success",
                        "valid_time": "2026-10-01T06:05:08.000000Z",
                        "frame": {
                            "source": "rdcap",
                            "product": "reflectivity",
                            "station": "TWN/RCHL",
                            "valid_time": "2026-10-01T06:05:08.000000Z",
                            "base_time": None,
                        },
                        "error": None,
                    }
                ],
                "interrupted": False,
            }
        )

    def frame(self, index: int) -> NativeFrame:
        assert index == 0
        return self._frame


class MixedNativeReport(NativeReport):
    def to_json(self) -> str:
        document = json.loads(super().to_json())
        document["counts"].update({"total": 3, "success": 1, "no_data": 1, "upstream_failed": 1})
        document["items"].extend(
            [
                {
                    "target": {"source": "rdcap", "product": "reflectivity", "station": "JPN/ISHI"},
                    "status": "no_data",
                    "valid_time": None,
                    "frame": None,
                    "error": None,
                },
                {
                    "target": {"source": "rdcap", "product": "reflectivity", "station": "PHL/SUBI"},
                    "status": "upstream_failed",
                    "valid_time": None,
                    "frame": None,
                    "error": {
                        "code": "ticket_exhausted",
                        "message": "provider request failed",
                        "stage": "acquire",
                        "retryable": True,
                    },
                },
            ]
        )
        return json.dumps(document)


class NativeBatchItem:
    def __init__(self, frame: NativeFrame, status: str = "success", data: object = None) -> None:
        self._frame = frame
        self.status = status
        self._data = data
        self.error = ""

    def frame(self) -> NativeFrame:
        return self._frame

    def error_details(self) -> str | None:
        return None

    def data(self) -> object:
        return self._data


class NativeBatchReport:
    def __init__(self, frames: list[NativeFrame], data: object) -> None:
        self._items = [NativeBatchItem(frame, data=data) for frame in frames]
        self.total = len(self._items)

    def item(self, index: int) -> NativeBatchItem:
        return self._items[index]


class FakeRaw:
    def __init__(self) -> None:
        self.closed = False

    def close(self) -> None:
        self.closed = True


class FakeNativeStream:
    def __init__(self, frames: list[NativeFrame], data: object) -> None:
        self._items = iter(NativeBatchItem(frame, data=data) for frame in frames)

    async def next(self) -> tuple[NativeBatchItem | None, None, None]:
        return next(self._items, None), None, None

    async def close(self) -> None:
        pass


class FakeSession:
    def __init__(self, _config: object) -> None:
        self.report = MixedNativeReport()
        self.field = object()
        self.fetch_many_calls = 0
        self.raw = FakeRaw()

    async def discover_report(self, _query: object) -> NativeReport:
        if getattr(_query, "stations", ()):
            return NativeReport()
        return self.report

    async def replay_raw_manifest(self, _path: object) -> object:
        return self.field

    async def fetch_raw(self, _frame: object) -> FakeRaw:
        return self.raw

    async def decode_science(self, raw: FakeRaw) -> object:
        assert not raw.closed
        return self.field

    def open_fetch_stream(
        self, frames: list[NativeFrame], **_options: object
    ) -> FakeNativeStream:
        return FakeNativeStream(frames, self.field)

    async def fetch_many_decoded(self, frames: list[NativeFrame], **_options: object) -> NativeBatchReport:
        self.fetch_many_calls += 1
        return NativeBatchReport(frames, self.field)


@pytest.fixture
def fake_native_session(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(rust_client._bridge, "CoreEngineSession", FakeSession)


def test_discovery_report_keeps_frame_handle_private_and_serialization_safe(
    fake_native_session: None,
) -> None:
    config = EffectiveConfig(values={}, origins={})
    with Client(config=config) as client:
        report = client.discover_report(Query("rdcap", stations=("TWN/RCHL",), latest=True))
        frame = report.frame(0)
        assert frame.source == "rdcap"
        assert report.counts["success"] == 1
        serialized = report.to_json()
        assert "private-ticket" not in serialized
        assert "locator" not in serialized
        assert report.as_dict()["items"][0]["frame"]["station"] == "TWN/RCHL"


@pytest.mark.asyncio
async def test_async_discovery_and_manifest_replay_use_the_same_native_engine(
    fake_native_session: None,
) -> None:
    config = EffectiveConfig(values={}, origins={})
    async with AsyncClient(config=config) as client:
        report = await client.discover_report(Query("rdcap", stations=("TWN/RCHL",), latest=True))
        assert report.frame(0).station == "TWN/RCHL"
        assert await client.replay_raw_manifest("raw-manifest.json") is client._session.field


def test_error_context_keeps_a_typed_native_code() -> None:
    context = ErrorContext(stage="decode", retryable=False, code="decode_unverified")
    from radiust.errors import DecodeError

    error = DecodeError("unsupported registration", context=context)
    assert error.code == "decode_unverified"
    assert error.as_dict()["stage"] == "decode"
    assert error.as_dict()["retryable"] is False


def test_fetch_many_collect_keeps_partial_discovery_report_and_successful_frame(
    fake_native_session: None,
) -> None:
    config = EffectiveConfig(values={}, origins={})
    with Client(config=config) as client:
        result = client.fetch_many(Query("rdcap", latest=True), on_error="collect")

    assert len(result.items) == 1
    assert result.items[0].status == "success"
    assert result.items[0].ref.station == "TWN/RCHL"
    assert result.discovery_report is not None
    assert result.discovery_counts["success"] == 1
    assert result.discovery_counts["no_data"] == 1
    assert result.discovery_counts["upstream_failed"] == 1
    safe_json = result.discovery_report.to_json()
    assert "private-ticket" not in safe_json
    assert "ticket_exhausted" in safe_json


def test_fetch_many_raise_keeps_discovery_failures_and_native_frame_handle(
    fake_native_session: None,
) -> None:
    config = EffectiveConfig(values={}, origins={})
    with Client(config=config) as client, pytest.raises(BatchError) as caught:
        client.fetch_many(Query("rdcap", latest=True), on_error="raise")

    partial = caught.value.partial_result
    assert partial.items == ()
    assert partial.discovery_report is not None
    assert partial.discovery_counts["upstream_failed"] == 1
    assert caught.value.cause.context.code == "ticket_exhausted"
    success_index = next(
        index for index, item in enumerate(partial.discovery_report.items) if item.status == "success"
    )
    assert partial.discovery_report.frame(success_index).station == "TWN/RCHL"
    assert client._session.fetch_many_calls == 0


@pytest.mark.asyncio
async def test_async_fetch_many_keeps_discovery_report(fake_native_session: None) -> None:
    config = EffectiveConfig(values={}, origins={})
    async with AsyncClient(config=config) as client:
        result = await client.fetch_many(Query("rdcap", latest=True), on_error="continue")

    assert result.discovery_counts["total"] == 3
    assert len(result.items) == 1
    assert result.items[0].ref.station == "TWN/RCHL"


def test_raw_context_closes_raw_while_decoded_field_remains_usable(
    fake_native_session: None,
) -> None:
    config = EffectiveConfig(values={}, origins={})
    with Client(config=config) as client:
        ref = client.discover_report(Query("rdcap", stations=("TWN/RCHL",), latest=True)).frame(0)
        with client.acquire(ref) as raw:
            field = client.decode(raw)
        assert raw.closed
        assert field is client._session.field


def _native_rdcap_field():
    return _core.RadarField(json.dumps({
        "name": "reflectivity",
        "values": [1.5, 2.5, 3.5, 4.5],
        "shape": [2, 2],
        "quality": [0, 1, 0, 65],
        "units": "dBZ",
        "valid_time": "2026-10-01T06:05:08.000000Z",
        "grid": {
            "shape": [2, 2],
            "crs": "EPSG:4326",
            "x": [120.0, 120.1],
            "y": [24.0, 24.1],
            "affine": None,
        },
        "provenance": ["source=rdcap", "fixture=offline"],
    }))


def _rdcap_frame_ref() -> FrameRef:
    return FrameRef(
        "rdcap",
        "reflectivity",
        datetime(2026, 10, 1, 6, 5, 8, tzinfo=timezone.utc),
        station="TWN/RCHL",
        locator_version="rdcap-csr-v1",
    )


def test_sync_client_writes_an_independent_rdcap_field(tmp_path) -> None:
    with Client(config={"runtime": {"allow_network": False}}) as client:
        report = client.write(
            _native_rdcap_field(),
            ref=_rdcap_frame_ref(),
            output=tmp_path,
            format="png",
        )

    assert report.command == "write"
    assert report.counts["written"] == 1
    assert list(tmp_path.rglob("*.png"))


@pytest.mark.asyncio
async def test_async_client_writes_an_independent_rdcap_field(tmp_path) -> None:
    async with AsyncClient(config={"runtime": {"allow_network": False}}) as client:
        report = await client.write(
            _native_rdcap_field(),
            ref=_rdcap_frame_ref(),
            output=tmp_path,
            format="netcdf",
        )

    assert report.command == "write"
    assert report.counts["written"] == 1
    assert list(tmp_path.rglob("*.nc"))


def _write_rdcap_raw_manifest(root: Path) -> FrameRef:
    station = "TWN/RCHL"
    key = "1790834708000"
    valid_time = datetime.fromtimestamp(int(key) / 1000, tz=timezone.utc)
    payload = (Path(__file__).parent / "fixtures/sources/rdcap/TWN/RCHL/file-response.reconstructed.json").read_bytes()
    content_sha256 = hashlib.sha256(payload).hexdigest()
    ref = FrameRef(
        "rdcap",
        "reflectivity",
        valid_time,
        station=station,
        locator={"country": "TWN", "station_code": "RCHL", "key": key},
        locator_version="rdcap-csr-v1",
        revision=content_sha256,
    )
    safe = safe_ref(ref)
    binding = {
        "schema_version": 1,
        "source": "rdcap",
        "product": "reflectivity",
        "station": station,
        "country": "TWN",
        "station_code": "RCHL",
        "key": key,
        "valid_time": safe["valid_time"],
        "logical_id": safe["logical_id"],
        "content_sha256": content_sha256,
        "content_size_bytes": len(payload),
    }
    binding_bytes = json.dumps(binding, ensure_ascii=False, indent=2).encode("utf-8")
    raw_root = root / "raw"
    raw_root.mkdir(parents=True)
    (raw_root / "file-response.json").write_bytes(payload)
    (raw_root / "binding.json").write_bytes(binding_bytes)
    artifacts = []
    for name, media_type, content in (
        ("file-response.json", "application/json", payload),
        ("binding.json", "application/json", binding_bytes),
    ):
        artifacts.append({
            "name": name,
            "media_type": media_type,
            "size_bytes": len(content),
            "sha256": hashlib.sha256(content).hexdigest(),
        })
    manifest = {"schema_version": 1, "raw_complete": True, "ref": safe, "artifacts": artifacts}
    manifest_path = root / "raw-manifest.json"
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    return ref


def _read_rdcap_output(format_name: str, path: Path):
    import numpy as np

    if format_name == "png":
        from PIL import Image

        return np.asarray(Image.open(path).convert("RGBA"))
    if format_name == "netcdf":
        import xarray as xr

        with xr.open_dataset(path, engine="h5netcdf", decode_times=False) as dataset:
            return (
                np.asarray(dataset["reflectivity"].values),
                np.asarray(dataset["quality"].values),
                dataset["crs"].attrs.get("spatial_ref"),
            )
    if format_name == "geotiff":
        import rasterio

        quality_path = path.with_name(path.stem + "_quality.tif")
        with rasterio.open(path) as dataset:
            values = dataset.read(1)
            crs = dataset.crs.to_string() if dataset.crs else None
        with rasterio.open(quality_path) as dataset:
            quality = dataset.read(1)
        return values, quality, crs
    import xarray as xr

    with xr.open_zarr(path, consolidated=False) as dataset:
        return (
            np.asarray(dataset["reflectivity"].values),
            np.asarray(dataset["quality"].values),
            dataset["crs"].attrs.get("spatial_ref"),
        )


def test_same_raw_frame_matches_cli_sync_and_async_decoded_outputs(tmp_path: Path) -> None:
    np = pytest.importorskip("numpy")
    pytest.importorskip("PIL")
    pytest.importorskip("xarray")
    pytest.importorskip("h5netcdf")
    pytest.importorskip("rasterio")
    pytest.importorskip("zarr")
    from click.testing import CliRunner
    from radiust.cli.main import main

    manifest = tmp_path / "input" / "raw-manifest.json"
    ref = _write_rdcap_raw_manifest(manifest.parent)
    formats = ("png", "netcdf", "geotiff", "zarr")
    cli_root, sync_root, async_root = (tmp_path / name for name in ("cli", "sync", "async"))
    cli_temp_root = tmp_path / "cli-temp"
    cli_config = tmp_path / "config.yaml"
    cli_config.write_text(
        f"runtime:\n  allow_network: false\n  temp_root: {cli_temp_root}\n",
        encoding="utf-8",
    )

    cli_result = CliRunner().invoke(
        main,
        [
            "--conf",
            str(cli_config),
            "replay",
            str(manifest),
            "--output",
            str(cli_root),
            "--format",
            ",".join(formats),
            "--json",
        ],
    )
    assert cli_result.exit_code == 0, cli_result.output
    cli_report = json.loads(cli_result.output)
    safe = safe_ref(ref)
    assert cli_report["query"]["station"] == ref.station
    assert cli_report["query"]["logical_id"] == safe["logical_id"]
    assert cli_report["query"]["valid_time"] == safe["valid_time"]
    assert cli_report["counts"]["written"] == len(formats)
    assert all(item["logical_id"] == safe["logical_id"] for item in cli_report["items"])

    config = {
        "runtime": {
            "allow_network": False,
            "temp_root": str(tmp_path / "sdk-temp"),
        }
    }
    with Client(config=config) as client:
        sync_field = client.replay_raw_manifest(manifest)
        sync_values = sync_field.copy_values()
        sync_metadata = json.loads(sync_field.metadata_json())
        sync_quality = np.frombuffer(sync_field.quality_le_bytes(), dtype="<u2").reshape(
            sync_field.shape
        ).copy()
        sync_results = {
            format_name: client.write(sync_field, ref=ref, output=sync_root, format=format_name)
            for format_name in formats
        }
        assert all(result.counts["written"] == 1 for result in sync_results.values())

    async def write_async_outputs():
        async with AsyncClient(config=config) as client:
            async_field = await client.replay_raw_manifest(manifest)
            values = async_field.copy_values()
            metadata = json.loads(async_field.metadata_json())
            quality = np.frombuffer(async_field.quality_le_bytes(), dtype="<u2").reshape(
                async_field.shape
            ).copy()
            results = {}
            for format_name in formats:
                results[format_name] = await client.write(
                    async_field, ref=ref, output=async_root, format=format_name
                )
            return values, quality, metadata, results

    async_values, async_quality, async_metadata, async_results = asyncio.run(
        write_async_outputs()
    )
    assert all(result.counts["written"] == 1 for result in async_results.values())

    np.testing.assert_array_equal(sync_values, async_values)
    np.testing.assert_array_equal(sync_quality, async_quality)
    for metadata in (sync_metadata, async_metadata):
        assert metadata["valid_time"] == safe["valid_time"]
        assert metadata["units"] == "dBZ"
        assert f"frame_logical_id={safe['logical_id']}" in metadata["provenance"]
    cli_items = cli_report["items"]
    for format_name in formats:
        sync_path = Path(sync_results[format_name].items[0].output_uri)
        async_path = Path(async_results[format_name].items[0].output_uri)
        cli_path = Path(next(
            item["output_uri"]
            for item in cli_items
            if item["output_uri"].endswith({"png": ".png", "netcdf": ".nc", "geotiff": ".tif", "zarr": ".zarr"}[format_name])
        ))
        cli_output = _read_rdcap_output(format_name, cli_path)
        sync_output = _read_rdcap_output(format_name, sync_path)
        async_output = _read_rdcap_output(format_name, async_path)
        if format_name == "png":
            np.testing.assert_array_equal(cli_output, sync_output)
            np.testing.assert_array_equal(sync_output, async_output)
            continue
        for output in (cli_output, sync_output, async_output):
            np.testing.assert_array_equal(output[0], np.asarray(sync_values).reshape(output[0].shape))
            np.testing.assert_array_equal(output[1], np.asarray(sync_quality).reshape(output[1].shape))
            assert output[2] is not None
            assert output[2] == "EPSG:4326" or "WGS 84" in output[2]
        np.testing.assert_array_equal(cli_output[0], sync_output[0])
        np.testing.assert_array_equal(sync_output[0], async_output[0])
        np.testing.assert_array_equal(cli_output[1], sync_output[1])
        np.testing.assert_array_equal(sync_output[1], async_output[1])


@pytest.mark.asyncio
async def test_async_raw_context_closes_raw_and_keeps_field(fake_native_session: None) -> None:
    config = EffectiveConfig(values={}, origins={})
    async with AsyncClient(config=config) as client:
        ref = (await client.discover_report(Query("rdcap", stations=("TWN/RCHL",), latest=True))).frame(0)
        async with client.acquire(ref) as raw:
            field = await client.decode(raw)
        assert raw.closed
        assert field is client._session.field


def test_sync_iter_fetch_uses_the_discovered_rdcap_frame(fake_native_session: None) -> None:
    config = EffectiveConfig(values={"runtime": {"frame_concurrency": 1}}, origins={})
    with Client(config=config) as client:
        client._session.report = NativeReport()
        with client.iter_fetch(Query("rdcap", latest=True), max_prefetch=1) as stream:
            values = list(stream)

    assert len(values) == 1
    assert values[0].ref.station == "TWN/RCHL"
    assert values[0].status == "success"
    assert values[0].data is client._session.field
