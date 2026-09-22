from __future__ import annotations

import asyncio
import gc
import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest
import radiust.pipeline as pipeline
import xarray as xr
from pyproj import Transformer
from radiust import AsyncClient, Client, FrameRef, ProductInfo, SourceInfo
from radiust.errors import TransportError
from radiust.field import RadarDataset, RadarField
from radiust.grids import CartesianGrid, GeographicGrid
from radiust.models import Artifact
from radiust.raw import RawFrame
from radiust.transport import HTTPTransport


def _ref(source: str = "review-test") -> FrameRef:
    return FrameRef(source, "reflectivity", datetime(2026, 1, 1, tzinfo=timezone.utc), station="s0")


def _field(value: float = 1.0) -> RadarField:
    grid = GeographicGrid([100.0, 101.0], [20.0, 21.0])
    data = xr.DataArray(
        np.full((2, 2), value, dtype="float32"),
        dims=("latitude", "longitude"),
        name="reflectivity",
    )
    return RadarField(data, grid)


def test_cartesian_to_geographic_transforms_target_coordinates_before_sampling() -> None:
    to_mercator = Transformer.from_crs("EPSG:4326", "EPSG:3857", always_xy=True)
    x0, y0 = to_mercator.transform(10.0, 50.0)
    x1, y1 = to_mercator.transform(11.0, 51.0)
    grid = CartesianGrid([x0, x1], [y0, y1], crs="EPSG:3857")
    data = xr.DataArray(
        np.array([[1.0, 2.0], [3.0, 4.0]], dtype="float32"),
        dims=("y", "x"),
        name="reflectivity",
    )

    result = RadarField(data, grid).to_geographic(bbox=(10.0, 50.0, 11.0, 51.0), resolution=1.0)

    np.testing.assert_allclose(result.data.values, [[1.0, 2.0], [3.0, 4.0]])


class _TempFileSource:
    info = SourceInfo(
        id="temp-review-test",
        description="temporary file source",
        adapter_version="1",
        products=(ProductInfo(id="reflectivity", variables=("reflectivity",), default=True),),
    )

    async def discover(self, query, context):
        return []

    async def download(self, ref, context):
        path = context.temp_root / "payload.bin"
        path.write_bytes(b"payload")
        return RawFrame(ref, (Artifact("payload.bin", "data", "application/octet-stream", path),))

    def decode(self, raw, context):
        return raw.bytes()


def test_acquire_keeps_context_temporary_files_until_rawframe_closes(monkeypatch) -> None:
    source = _TempFileSource()
    monkeypatch.setattr(pipeline, "get_source", lambda _source_id: source)
    ref = _ref(source.info.id)

    with Client() as client:
        with client.acquire(ref) as raw:
            path = Path(raw.artifacts[0].payload)
            assert path.exists()
            assert raw.bytes() == b"payload"
        assert not path.exists()


@pytest.mark.asyncio
async def test_async_acquire_keeps_context_temporary_files_until_rawframe_closes(monkeypatch) -> None:
    source = _TempFileSource()
    monkeypatch.setattr(pipeline, "get_source", lambda _source_id: source)
    ref = _ref(source.info.id)

    async with AsyncClient() as client:
        async with client.acquire(ref) as raw:
            path = Path(raw.artifacts[0].payload)
            assert path.exists()
            assert raw.bytes() == b"payload"
        assert not path.exists()


def test_write_applies_variable_selection_before_encoding(tmp_path: Path) -> None:
    grid = GeographicGrid([100.0, 101.0], [20.0, 21.0])
    dataset = RadarDataset(
        xr.Dataset(
            {
                "a": (("latitude", "longitude"), np.ones((2, 2), dtype="float32")),
                "b": (("latitude", "longitude"), np.full((2, 2), 2.0, dtype="float32")),
            }
        ),
        grid,
    )

    with Client() as client:
        report = client.write(dataset, output=tmp_path, ref=_ref(), variable="a")

    output = Path(report.items[0].output_uri)
    with xr.open_dataset(output) as stored:
        assert "a" in stored.data_vars
        assert "b" not in stored.data_vars


def test_write_overwrite_replaces_same_identity_output(tmp_path: Path) -> None:
    ref = _ref()
    with Client() as client:
        first = client.write(_field(1.0), output=tmp_path, ref=ref)
        second = client.write(_field(9.0), output=tmp_path, ref=ref, overwrite=True)

    assert first.items[0].status == "written"
    assert second.items[0].status == "written"
    with xr.open_dataset(Path(second.items[0].output_uri)) as stored:
        np.testing.assert_allclose(stored["reflectivity"].values, 9.0)


class _StreamSource(_TempFileSource):
    info = SourceInfo(
        id="stream-review-test",
        description="stream source",
        adapter_version="1",
        products=(ProductInfo(id="reflectivity", variables=("reflectivity",), default=True),),
    )

    async def download(self, ref, context):
        return RawFrame(ref, (Artifact("payload.bin", "data", "application/octet-stream", b"payload"),))

    def decode(self, raw, context):
        return bytearray(1024 * 1024)


@pytest.mark.asyncio
async def test_stream_does_not_retain_emitted_payloads(monkeypatch) -> None:
    source = _StreamSource()
    monkeypatch.setattr(pipeline, "get_source", lambda _source_id: source)
    stream = AsyncClient().aiter_fetch([_ref(source.info.id)], max_prefetch=1)
    try:
        result = await anext(stream)
        assert result.data is not None
        assert stream._emitted[0].data is None
        del result
        gc.collect()
    finally:
        client = stream.client
        await stream.aclose()
        assert stream._emitted == {}
        await client.aclose()


class _DelayedSource(_StreamSource):
    info = SourceInfo(
        id="delayed-review-test",
        description="delayed source",
        adapter_version="1",
        products=(ProductInfo(id="reflectivity", variables=("reflectivity",), default=True),),
    )

    async def download(self, ref, context):
        await asyncio.sleep(0.15)
        return RawFrame(ref, (Artifact("payload.bin", "data", "application/octet-stream", b"payload"),))


@pytest.mark.asyncio
async def test_cancelled_async_download_never_commits_late_output(monkeypatch, tmp_path: Path) -> None:
    source = _DelayedSource()
    monkeypatch.setattr(pipeline, "get_source", lambda _source_id: source)
    client = AsyncClient()
    task = asyncio.create_task(client.download([_ref(source.info.id)], output=tmp_path, raw_only=True))
    await asyncio.sleep(0.03)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    await client.aclose()
    await asyncio.sleep(0.2)
    assert not list(tmp_path.rglob("*.manifest.json"))


@pytest.mark.asyncio
async def test_cancelled_async_write_never_commits_late_output(monkeypatch, tmp_path: Path) -> None:
    original = pipeline.encoder_for("netcdf")

    def slow_writer(value, path, *, options=None):
        time.sleep(0.15)
        return original.writer(value, path, options=options)

    monkeypatch.setattr(pipeline, "encoder_for", lambda _format: SimpleNamespace(writer=slow_writer))
    client = AsyncClient()
    task = asyncio.create_task(client.write(_field(), output=tmp_path, ref=_ref()))
    await asyncio.sleep(0.03)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    await client.aclose()
    await asyncio.sleep(0.2)
    assert not list(tmp_path.rglob("*.manifest.json"))


class _RedirectHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(302)
        self.send_header("Location", "http://0.0.0.0:9/blocked")
        self.end_headers()

    def log_message(self, format, *args):
        return


def test_redirect_destination_is_checked_against_network_policy() -> None:
    server = HTTPServer(("127.0.0.1", 0), _RedirectHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with pytest.raises(TransportError, match="public network"):
            HTTPTransport(max_attempts=1, timeout=0.2).get_sync(
                f"http://127.0.0.1:{server.server_port}/redirect"
            )
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=1)
