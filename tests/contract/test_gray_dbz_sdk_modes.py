"""Source mode forwarding and compatibility boundaries for the thin SDK."""

from __future__ import annotations

import asyncio
import json
from datetime import datetime, timezone
from types import SimpleNamespace

import pytest
from radiust import FrameRef, _bridge
from radiust.errors import DecodeError, UnsupportedQueryError
from radiust.rust_client import Client
from radiust.streaming import _fetch_item_result


class _SessionEngine:
    def __init__(self) -> None:
        self.calls: list[tuple[object, ...]] = []
        self.science_calls = 0
        self.dbz_error: Exception | None = None

    async def replay_raw_manifest(self, *args: object) -> object:
        self.calls.append(tuple(args))
        return object()

    async def decode_science(self, _raw: object) -> object:
        self.science_calls += 1
        return SimpleNamespace(name="reflectivity", units="dBZ")


def _session(engine: object) -> _bridge.CoreEngineSession:
    session = object.__new__(_bridge.CoreEngineSession)
    session._engine = engine
    return session


def test_replay_mode_none_preserves_one_argument_call_and_dbz_is_keyword_mode():
    engine = _SessionEngine()
    session = _session(engine)

    old_value = asyncio.run(session.replay_raw_manifest("raw-manifest.json"))
    assert old_value is not None
    assert engine.calls[-1] == ("raw-manifest.json",)

    dbz_value = asyncio.run(session.replay_raw_manifest("raw-manifest.json", mode="dbz"))
    assert dbz_value is not None
    assert engine.calls[-1] == ("raw-manifest.json", "dbz")


def test_old_extension_allows_none_but_reports_missing_replay_mode_capability():
    class OldEngine:
        async def replay_raw_manifest(self, _path: str) -> object:
            return "legacy-science-result"

    session = _session(OldEngine())
    assert asyncio.run(session.replay_raw_manifest("raw-manifest.json")) == "legacy-science-result"
    with pytest.raises(UnsupportedQueryError, match="does not support replay mode=dbz"):
        asyncio.run(session.replay_raw_manifest("raw-manifest.json", mode="dbz"))


def test_missing_direct_dbz_method_can_fallback_only_after_unit_check():
    engine = _SessionEngine()
    session = _session(engine)
    frame = SimpleNamespace(source="tw", product="grid")
    raw = SimpleNamespace(frame=lambda: frame)

    value = asyncio.run(session.decode_dbz(raw))
    assert value.name == "reflectivity"
    assert value.units == "dBZ"
    assert engine.science_calls == 1


def test_failure_from_present_dbz_method_never_falls_back():
    class CurrentEngine(_SessionEngine):
        async def decode_dbz(self, _raw: object) -> object:
            raise ValueError("decoder failed")

    engine = CurrentEngine()
    session = _session(engine)
    with pytest.raises(DecodeError, match="decoder failed"):
        asyncio.run(session.decode_dbz(object()))
    assert engine.science_calls == 0


def test_numeric_dbz_read_and_write_forward_receipt_bound_core_objects(tmp_path):
    class NumericEngine:
        def __init__(self):
            self.read_args = None
            self.write_args = None

        async def read_dbz_file(self, *args):
            self.read_args = args
            return "raster-result"

        async def write_raster_to(self, *args):
            self.write_args = args
            return json.dumps({"status": "written", "output_uri": "out/file.nc", "manifest": {}})

    engine = NumericEngine()
    session = _session(engine)
    assert asyncio.run(session.read_dbz_file("input.nc", variable="reflectivity")) == "raster-result"
    assert engine.read_args == ("input.nc", "reflectivity", None)
    result = asyncio.run(
        session.write_raster(
            "raster-result",
            output_name="out/file.nc",
            format="netcdf",
            options_json="{}",
            overwrite=True,
            output_root=tmp_path,
        )
    )
    assert result["status"] == "written"
    assert engine.write_args[:5] == (
        "raster-result",
        "out/file.nc",
        "netcdf",
        "{}",
        True,
    )
    assert engine.write_args[5] == str(tmp_path)
    assert engine.write_args[6] is None


def test_old_extension_reports_numeric_read_write_capability_as_unsupported():
    class OldEngine:
        pass

    session = _session(OldEngine())
    with pytest.raises(UnsupportedQueryError, match="receipt-bound numeric dBZ reads"):
        asyncio.run(session.read_dbz_file("input.nc"))
    with pytest.raises(UnsupportedQueryError, match="receipt-bound raster writes"):
        asyncio.run(
            session.write_raster(
                object(),
                output_name="out/file.nc",
                format="netcdf",
                options_json="{}",
                overwrite=False,
            )
        )


def test_stream_uses_gray_result_when_raster_result_is_empty():
    frame = FrameRef(
        "ca",
        "rain",
        datetime(2026, 10, 3, tzinfo=timezone.utc),
        station="CASFT",
    )
    gray = object()

    class NativeItem:
        status = "success"
        error = None

        def frame(self):
            return frame

        def error_details(self):
            return None

        def raster_result(self):
            return None

        def gray_result(self):
            return gray

    result = _fetch_item_result(NativeItem())

    assert result.status == "success"
    assert result.error is None
    assert result.data is gray


def test_default_raster_name_includes_selected_input_and_frame(tmp_path):
    class Session:
        def __init__(self):
            self.output_names = []

        async def write_raster(self, _result, **kwargs):
            self.output_names.append(kwargs["output_name"])
            return {"status": "written"}

    client = Client.__new__(Client)
    client._session = Session()
    content_digest = "a" * 64

    async def write(selection, frame_index):
        input_value = {
            "kind": "numeric_file",
            "identity": {
                "content_digest": content_digest,
                "selection": selection,
            },
            "read_receipt": {
                "content_digest": content_digest,
                "selection": selection,
                "frame_index": frame_index,
            },
        }
        processing = {"steps": [{"method": "gray_to_dbz"}]}
        result = SimpleNamespace(
            input_json=json.dumps(input_value),
            processing_json=json.dumps(processing),
            mode_info_json=json.dumps({"variable": "reflectivity"}),
        )
        await Client._awrite_raster_result(
            client,
            result,
            output=tmp_path,
            format="netcdf",
            overwrite=False,
            ref=None,
            options={},
        )

    common_input_selection = {"variable": "reflectivity", "valid_time": "2026-10-03T01:00:00Z"}
    asyncio.run(write(common_input_selection, 0))
    asyncio.run(write(common_input_selection, 1))
    asyncio.run(
        write({"variable": "rain_rate", "valid_time": "2026-10-03T01:00:00Z"}, 0)
    )

    assert len(client._session.output_names) == 3
    assert len(set(client._session.output_names)) == 3


def test_source_dbz_download_forwards_raw_format_and_on_error_to_rust(monkeypatch, tmp_path):
    class DownloadEngine:
        def __init__(self):
            self.args = None

        async def download_dbz_mode(self, *args):
            self.args = args
            return "download-report"

    engine = DownloadEngine()
    session = _session(engine)
    monkeypatch.setattr(_bridge, "_native_frames", lambda frames: list(frames))
    result = asyncio.run(
        session.download_dbz_mode(
            ["frame"],
            on_error="stop",
            overwrite=True,
            output_root=tmp_path,
            format="zarr",
            include_raw=True,
        )
    )
    assert result == "download-report"
    assert engine.args == (["frame"], "stop", False, True, str(tmp_path), "zarr", True)


def test_old_extension_reports_source_dbz_download_as_unsupported(monkeypatch):
    monkeypatch.setattr(_bridge, "_native_frames", lambda frames: list(frames))
    session = _session(object())
    with pytest.raises(UnsupportedQueryError, match="does not support source dBZ downloads"):
        asyncio.run(session.download_dbz_mode(["frame"]))


def test_client_download_mode_dbz_uses_the_source_mode_writer(monkeypatch, tmp_path):
    from radiust.rust_client import Client

    class NativeItem:
        status = "written"
        output_uri = str(tmp_path / "reflectivity.nc")

        def frame(self):
            return SimpleNamespace(
                source="ca",
                product="rain",
                station="CASFT",
                valid_time="2026-10-03T01:02:03Z",
                logical_id="logical-id",
            )

        def error_json(self):
            return ""

    class NativeReport:
        total = 1
        interrupted = False

        def item(self, _index):
            return NativeItem()

    class Session:
        def __init__(self):
            self.calls = []

        async def download_dbz_mode(self, *args, **kwargs):
            self.calls.append((args, kwargs))
            return NativeReport()

    session = Session()
    client = Client.__new__(Client)
    client._session = session
    monkeypatch.setattr(
        "radiust.rust_client._resolve_frames",
        lambda *_args, **_kwargs: asyncio.sleep(0, result=["frame"]),
    )
    report = asyncio.run(
        Client._adownload_inner(
            client,
            {"source": "ca"},
            output=tmp_path,
            format="netcdf",
            mode="dbz",
            raw=True,
            raw_only=False,
            overwrite=True,
            output_template=None,
            on_error="collect",
            progress=None,
            processing={"variable": "reflectivity"},
            events=None,
        )
    )
    assert report.counts["written"] == 1
    assert session.calls == [
        (
            ( ["frame"],),
            {
                "on_error": "collect",
                "overwrite": True,
                "output_root": str(tmp_path),
                "format": "netcdf",
                "include_raw": True,
            },
        )
    ]
