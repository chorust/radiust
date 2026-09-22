from __future__ import annotations

import json
from datetime import datetime, timezone
from hashlib import sha256
from importlib import import_module
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import numpy as np
import pytest
import radiust.pipeline as pipeline
from click.testing import CliRunner
from radiust import Client, Query, _core
from radiust.cli.main import main
from radiust.identity import artifact_bytes
from radiust.registry import registry
from radiust.sources.au import AuSource
from radiust.sources.base import FixtureSource
from radiust.sources.ca import CaSource
from radiust.sources.es import EsSource
from radiust.sources.fr import FrSource
from radiust.sources.kr import KrSource
from radiust.sources.nz import NzSource
from radiust.sources.pt import PtSource
from radiust.sources.sg import SgSource
from radiust.sources.th import ThSource
from radiust.sources.th_royalrain import ThRoyalRainSource
from radiust.sources.tw import TwSource
from radiust.sources.tw_http import TwHttpSource
from radiust.sources.vn import VnSource
from radiust.sources.windy import WindySource

ROOT = Path(__file__).parents[2]
INVENTORY = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))


def _raw_fixture_cases() -> list[tuple[str, Path, dict[str, Any]]]:
    cases = []
    for item in INVENTORY["sources"]:
        if item["id"] == "rainviewer":
            continue  # exercised below through its registered adapter and replay transport
        fixture_path = ROOT / item["fixture"]
        document = json.loads(fixture_path.read_text(encoding="utf-8"))
        if document.get("status") == "blocked":
            continue
        frames = document.get("frames", [])
        assert frames, f"{item['id']} fixture is not blocked and must contain a frame"
        cases.append((item["id"], fixture_path, frames[0]))
    return cases


RAW_CASES = _raw_fixture_cases()


class ReplayTransport:
    def __init__(self, responses: dict[str, bytes]) -> None:
        self.responses = responses
        self.urls: list[str] = []

    async def get(self, url: str) -> bytes:
        self.urls.append(url)
        try:
            return self.responses[url]
        except KeyError as exc:
            raise AssertionError(f"unexpected replay request: {url}") from exc


def _rainviewer_replay(fixture_path: Path, frame: dict[str, Any]) -> ReplayTransport:
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    discovery = fixture["discovery"]
    path = frame["locator"]["path"]
    host = frame["locator"]["host"]
    timestamp = int(datetime.fromisoformat(frame["valid_time"].replace("Z", "+00:00")).timestamp())
    manifest = {
        "version": discovery["version"],
        "generated": discovery["generated"],
        "host": host,
        "radar": {"past": [{"time": timestamp, "path": path}], "nowcast": []},
    }
    responses = {discovery["api_url"]: json.dumps(manifest).encode("utf-8")}
    for artifact in frame["artifacts"]:
        name = artifact["name"].removesuffix(".png").split("-")
        x = int(name[-2][1:])
        y = int(name[-1][1:])
        url = f"{host}{path}/512/1/{x}/{y}/2/0_0.png"
        responses[url] = (fixture_path.parent / artifact["path"]).read_bytes()
    return ReplayTransport(responses)


def _query(source_id: str, frame: dict[str, Any]) -> Query:
    valid_time = datetime.fromisoformat(frame["valid_time"].replace("Z", "+00:00"))
    stations = (frame["station"],) if frame.get("station") else ()
    return Query(source_id, product=frame["product"], stations=stations, at=valid_time)


def test_source_cli_list_covers_the_head_inventory() -> None:
    result = CliRunner().invoke(main, ["list", "sources", "--json"])

    assert result.exit_code == 0, result.output
    report = json.loads(result.output)
    catalog_ids = {item["id"] for item in report["items"]}
    inventory_ids = {item["id"] for item in INVENTORY["sources"]}
    assert catalog_ids == inventory_ids
    assert len(catalog_ids) == 24

    raw_fixture_ids = set()
    for item in INVENTORY["sources"]:
        manifest = json.loads((ROOT / item["fixture"]).read_text(encoding="utf-8"))
        if manifest.get("status") != "blocked":
            raw_fixture_ids.add(item["id"])
    matrix_ids = {source_id for source_id, _fixture_path, _frame in RAW_CASES} | {"rainviewer"}
    assert matrix_ids == raw_fixture_ids


def test_rainviewer_adapter_flows_through_sdk_cli_download_and_cat(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """Replay official RainViewer bytes through the registered adapter, without network access."""

    fixture_path = ROOT / "tests/fixtures/sources/rainviewer/fixture.json"
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    frame = fixture["frames"][0]
    transport = _rainviewer_replay(fixture_path, frame)
    original_context = pipeline.Client._context

    def replay_context(client: Client, source_id: str):
        context = original_context(client, source_id)
        if source_id == "rainviewer":
            context.transport = transport
        return context

    monkeypatch.setattr(pipeline.Client, "_context", replay_context)
    query = _query("rainviewer", frame)

    with Client(config={"runtime": {"allow_network": False}, "cache": {"enabled": False}}) as client:
        field = client.fetch(query)

    assert field.provenance["decoder"] == "rainviewer-universal-blue-v1"
    assert field.data.shape == tuple(frame["metadata"]["reference_shape"])
    assert str(field.data.dtype) == frame["metadata"]["reference_dtype"]
    assert field.data.attrs["units"] == frame["metadata"]["reference_units"]
    assert field.grid.crs == frame["metadata"]["reference_grid"]["crs"]
    assert str(field.quality.dtype) == frame["metadata"]["reference_quality"]["dtype"]
    row, column = map(int, next(iter(frame["metadata"]["reference_pixels"])).split(","))
    assert float(field.data.values[row, column]) == frame["metadata"]["reference_pixels"][f"{row},{column}"]
    assert int(field.quality.values[row, column]) == frame["metadata"]["reference_quality"]["valid_code"]

    output = tmp_path / "rainviewer-raw"
    downloaded = CliRunner().invoke(
        main,
        [
            "download",
            "rainviewer",
            "--product",
            frame["product"],
            "--at",
            frame["valid_time"],
            "--output",
            str(output),
            "--raw-only",
            "--no-cache",
            "--json",
        ],
    )
    assert downloaded.exit_code == 0, downloaded.output
    report = json.loads(downloaded.output)
    assert report["counts"]["written"] == 1
    assert len(list(output.rglob("*.png"))) == 4

    preview = CliRunner().invoke(
        main,
        ["cat", "rainviewer", "--renderer", "text", "--at", frame["valid_time"]],
    )
    assert preview.exit_code == 0, preview.output
    assert "source=rainviewer" in preview.output
    assert "shape=(1024, 1024)" in preview.output
    assert "units=dBZ" in preview.output
    assert transport.urls.count(fixture["discovery"]["api_url"]) == 3
    assert len(transport.urls) == 15


def test_tw_numeric_adapter_flows_through_sdk_cli_download_and_cat(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """Replay official CWA numeric bytes through the registered adapter and NetCDF writer."""
    fixture_path = ROOT / "tests/fixtures/sources/tw/fixture.json"
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    frame = next(item for item in fixture["frames"] if item["product"] == "grid")
    payload = (fixture_path.parent / frame["artifacts"][0]["path"]).read_bytes()
    url = f"{TwSource.BUCKET_BASE}/{TwSource.GRID_KEY}.json"
    transport = ReplayTransport({url: payload})
    original_context = pipeline.Client._context

    def replay_context(client: Client, source_id: str):
        context = original_context(client, source_id)
        if source_id == "tw":
            context.transport = transport
        return context

    monkeypatch.setattr(pipeline.Client, "_context", replay_context)
    query = _query("tw", frame)

    with Client(config={"runtime": {"allow_network": False}, "cache": {"enabled": False}}) as client:
        field = client.fetch(query)

    reference = frame["metadata"]
    assert field.grid.crs == reference["reference_grid"]["crs"]
    assert field.data.shape == tuple(reference["reference_shape"])
    assert field.data.attrs["units"] == reference["reference_units"]
    assert int(np.isfinite(field.data.values).sum()) == reference["reference_quality"]["finite"]
    pixel = reference["reference_pixels"][0]
    assert field.data.values[pixel["row"], pixel["column"]] == pixel["value"]

    output = tmp_path / "tw-grid"
    downloaded = CliRunner().invoke(main, [
        "download", "tw", "--product", "grid", "--at", frame["valid_time"],
        "--output", str(output), "--no-cache", "--json",
    ])
    assert downloaded.exit_code == 0, downloaded.output
    report = json.loads(downloaded.output)
    assert report["counts"]["written"] == 1
    files = list(output.rglob("*.nc"))
    assert len(files) == 1

    import xarray as xr

    with xr.open_dataset(files[0]) as dataset:
        assert dataset.reflectivity.shape == tuple(reference["reference_shape"])
        assert dataset.crs.attrs["spatial_ref"] == "EPSG:3821"
        assert dataset.reflectivity.values[pixel["row"], pixel["column"]] == pixel["value"]

    preview = CliRunner().invoke(main, [
        "cat", "tw", "--product", "grid", "--renderer", "text", "--at", frame["valid_time"],
    ])
    assert preview.exit_code == 0, preview.output
    assert "source=tw product=grid" in preview.output
    assert "shape=(881, 921)" in preview.output
    assert "units=dBZ" in preview.output
    assert "\x1b[" not in preview.output
    assert len(transport.urls) >= 3
    assert set(transport.urls) == {url}


def test_tw_png_raw_only_cli_replays_registered_adapter(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """Preserve both official PNG and metadata via the production source's raw-only path."""
    fixture_path = ROOT / "tests/fixtures/sources/tw/fixture.json"
    frame = next(item for item in json.loads(fixture_path.read_text())["frames"] if item["product"] == "observation")
    responses = {
        f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.{suffix}": (
            fixture_path.parent / f"raw/O-A0058-005.{suffix}"
        ).read_bytes()
        for suffix in ("json", "png")
    }
    transport = ReplayTransport(responses)
    original_context = pipeline.Client._context

    def replay_context(client: Client, source_id: str):
        context = original_context(client, source_id)
        if source_id == "tw":
            context.transport = transport
        return context

    monkeypatch.setattr(pipeline.Client, "_context", replay_context)
    output = tmp_path / "tw-png-raw"
    downloaded = CliRunner().invoke(main, [
        "download", "tw", "--product", "observation", "--latest", "--raw-only",
        "--no-cache", "--output", str(output), "--json",
    ])
    assert downloaded.exit_code == 0, downloaded.output
    report = json.loads(downloaded.output)
    assert report["counts"]["written"] == 1
    assert report["items"][0]["valid_time"].startswith(frame["valid_time"].removesuffix("Z"))
    for suffix in ("json", "png"):
        retained = list(output.rglob(f"O-A0058-005.{suffix}"))
        assert len(retained) == 1
        assert retained[0].read_bytes() == responses[f"{TwSource.BUCKET_BASE}/{TwSource.PRODUCT_KEY}.{suffix}"]
    assert len(transport.urls) >= 3
    assert set(transport.urls) == set(responses)


@pytest.mark.parametrize("source_id", ["au", "fr"])
def test_registered_legacy_adapter_raw_replay_via_sdk_and_cli(
    source_id: str,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """Preserve historical raw bytes through the actual provider adapter."""
    fixture_path = ROOT / "tests/fixtures/sources" / source_id / "fixture.json"
    frame = json.loads(fixture_path.read_text(encoding="utf-8"))["frames"][0]
    expected = (fixture_path.parent / frame["artifacts"][0]["path"]).read_bytes()
    observed: list[str] = []

    if source_id == "au":
        filename = frame["artifacts"][0]["name"]

        async def ftp_nlst(url: str, *_args):
            observed.append(url)
            assert url == AuSource.FTP_ROOT
            return [filename]

        async def ftp_read(url: str, *_args):
            observed.append(url)
            assert url == AuSource.FTP_ROOT + filename
            return expected

        monkeypatch.setattr(_core, "ftp_nlst", ftp_nlst)
        monkeypatch.setattr(_core, "ftp_read", ftp_read)
    else:
        at = datetime.fromisoformat(frame["valid_time"].replace("Z", "+00:00"))
        monkeypatch.setattr(FrSource, "_current_frame_time", staticmethod(lambda now=None: at))

        class WmsReplay:
            async def get_response(self, url: str, *, headers=None):
                observed.append(url)
                assert url == FrSource.PAGE_URL
                return SimpleNamespace(headers={"set-cookie": "mfsession=ReplayOnly; Path=/; Secure"}, status=200, url=url)

            async def get(self, url: str, *, headers=None):
                observed.append(url)
                assert url.startswith(FrSource.WMS_BASE_URL + "?")
                assert "time=2026-09-18T02%3A45%3A00Z" in url
                return expected

        old_context = pipeline.Client._context

        def replay_context(client: Client, requested: str):
            context = old_context(client, requested)
            if requested == "fr":
                context.transport = WmsReplay()
            return context

        monkeypatch.setattr(pipeline.Client, "_context", replay_context)

    query = Query(source_id, latest=True)
    with Client(config={"runtime": {"allow_network": False}, "cache": {"enabled": False}}) as client:
        refs = client.discover(query)
        assert len(refs) == 1
        with client.acquire(refs[0]) as raw:
            assert len(raw.artifacts) == 1
            assert artifact_bytes(raw.artifacts[0]) == expected

    output = tmp_path / "registered-raw"
    downloaded = CliRunner().invoke(
        main,
        ["download", source_id, "--latest", "--raw-only", "--no-cache", "--output", str(output), "--json"],
    )
    assert downloaded.exit_code == 0, downloaded.output
    report = json.loads(downloaded.output)
    assert report["counts"]["written"] == 1
    assert any(file.read_bytes() == expected for file in output.rglob("*") if file.is_file())
    assert len(observed) == 4

    # Neither adapter currently has an independently verified physical palette;
    # the text renderer must not present their raw display pixels as dBZ.
    preview = CliRunner().invoke(main, ["cat", source_id, "--latest", "--renderer", "text"])
    assert preview.exit_code != 0
    assert "verified scientific decoder" in preview.output


def test_tw_http_registered_adapter_replays_provider_timeline_and_raw(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    fixture_path = ROOT / "tests/fixtures/sources/tw-http/fixture.json"
    frame = json.loads(fixture_path.read_text(encoding="utf-8"))["frames"][0]
    expected = (fixture_path.parent / frame["artifacts"][0]["path"]).read_bytes()
    url = frame["uri"]
    js = b'''0:{"img":'CV1_3600_202609181050.png', 'text':'2026/09/18 10:50'}'''
    requests: list[str] = []

    class Replay:
        async def get(self, address: str, *, headers=None):
            requests.append(address)
            if address == TwHttpSource.JS_URL:
                return js
            if address == url:
                return expected
            raise AssertionError("unexpected TW HTTP replay request")

    old_context = pipeline.Client._context

    def context(client: Client, source_id: str):
        result = old_context(client, source_id)
        if source_id == "tw-http":
            result.transport = Replay()
        return result

    monkeypatch.setattr(pipeline.Client, "_context", context)
    with Client(config={"runtime": {"allow_network": False}, "cache": {"enabled": False}}) as client:
        refs = client.discover(Query("tw-http", latest=True))
        assert len(refs) == 1
        assert refs[0].valid_time.isoformat().startswith("2026-09-18T02:50:00")
        with client.acquire(refs[0]) as raw:
            assert artifact_bytes(raw.artifacts[0]) == expected

    output = tmp_path / "tw-http-raw"
    run = CliRunner().invoke(main, [
        "download", "tw-http", "--latest", "--raw-only", "--no-cache", "--output", str(output), "--json",
    ])
    assert run.exit_code == 0, run.output
    assert json.loads(run.output)["counts"]["written"] == 1
    assert any(path.read_bytes() == expected for path in output.rglob("*") if path.is_file())
    assert requests == [TwHttpSource.JS_URL, url, TwHttpSource.JS_URL, url]

    preview = CliRunner().invoke(main, ["cat", "tw-http", "--latest", "--renderer", "text"])
    assert preview.exit_code != 0
    assert "verified scientific decoder" in preview.output


def test_kr_registered_adapter_replays_station_discovery_and_original_image(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """Test the CGI POST and image GET through SDK and CLI with retained provider bytes."""
    fixture_path = ROOT / "tests/fixtures/sources/kr/fixture.json"
    frame = json.loads(fixture_path.read_text(encoding="utf-8"))["frames"][0]
    expected = (fixture_path.parent / frame["artifacts"][0]["path"]).read_bytes()
    posts: list[dict[str, str]] = []
    gets: list[str] = []
    monkeypatch.setattr(KrSource, "STATIONS", (frame["station"],))

    class Replay:
        async def post(self, url: str, body: dict[str, str], *, headers=None) -> bytes:
            assert url == KrSource.DISCOVERY_URL
            assert body["siteCd"] == frame["station"]
            assert body["cgiId"] == "STN"
            posts.append(body)
            return b'[{"result":1,"recDate":"202609181250"}]'

        async def get(self, url: str, *, headers=None) -> bytes:
            gets.append(url)
            assert url.startswith(KrSource.IMAGE_URL + "?")
            assert "tm=202609181250" in url
            assert "stn=KWK" in url
            return expected

    old_context = pipeline.Client._context

    def context(client: Client, source_id: str):
        result = old_context(client, source_id)
        if source_id == "kr":
            result.transport = Replay()
        return result

    monkeypatch.setattr(pipeline.Client, "_context", context)
    with Client(config={"runtime": {"allow_network": False}, "cache": {"enabled": False}}) as client:
        refs = client.discover(Query("kr", latest=True, stations=("KWK",)))
        assert len(refs) == 1
        assert refs[0].valid_time.isoformat() == "2026-09-18T03:50:00+00:00"
        with client.acquire(refs[0]) as raw:
            assert artifact_bytes(raw.artifacts[0]) == expected

    output = tmp_path / "kr-raw"
    run = CliRunner().invoke(main, [
        "download", "kr", "--latest", "--station", "KWK", "--raw-only",
        "--no-cache", "--output", str(output), "--json",
    ])
    assert run.exit_code == 0, run.output
    assert json.loads(run.output)["counts"]["written"] == 1
    assert any(path.read_bytes() == expected for path in output.rglob("*") if path.is_file())
    assert len(posts) == 10  # Five bounded lookbacks for each SDK/CLI discovery.
    assert len(gets) == 2

    preview = CliRunner().invoke(main, ["cat", "kr", "--latest", "--renderer", "text"])
    assert preview.exit_code != 0
    assert "verified scientific decoder" in preview.output


@pytest.mark.parametrize("source_id", ("es", "vn", "ca", "sg", "pt", "nz"))
def test_image_scraper_registered_adapter_replays_discovery_and_raw_cli(
    source_id: str,
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """Exercise actual provider timeline/directory parsing and original raw bytes."""
    fixture_path = ROOT / "tests/fixtures/sources" / source_id / "fixture.json"
    frame = json.loads(fixture_path.read_text(encoding="utf-8"))["frames"][0]
    expected = (fixture_path.parent / frame["artifacts"][0]["path"]).read_bytes()
    requests: list[str] = []
    if source_id == "es":
        discovery = json.dumps([{"Elementos": [{
            "Fecha": "2026-09-17T16:40:00+02:00",
            "Nombre fichero": "radw202609171440_3857.png",
        }]}]).encode()
        responses = {EsSource.TIMELINE_URL: discovery, frame["uri"]: expected}
    elif source_id == "vn":
        monkeypatch.setattr(VnSource, "STATIONS", ("PLI",))
        responses = {
            VnSource.BASE_URL + "/radar/PLI": b'<script>tentimesett[0] = "202609180350";</script>',
            frame["uri"]: expected,
        }
    elif source_id == "sg":
        responses = {
            SgSource.PAGE_URL: f'<script>slideshowimages("{frame["uri"]}");</script>'.encode(),
            frame["uri"]: expected,
        }
    elif source_id == "pt":
        responses = {
            PtSource.INDEX_URL: json.dumps({"Madeira": [{
                "date": "2026-09-18 02:50", "path": "pcr_pst-2026-09-18T0250.png",
            }]}).encode(),
            frame["uri"]: expected,
        }
    elif source_id == "nz":
        monkeypatch.setattr(NzSource, "STATIONS", {"NZAU2": "Kumeu"})
        responses = {
            NzSource.BASE_URL + "/publicData/mobileRainRadar_rural_Kumeu": json.dumps({
                "imageList": [{"url": "/radarImage", "dateTimeISO": "2026-09-18T15:50:00+12:00"}],
            }).encode(),
            frame["uri"]: expected,
        }
    else:
        class FixedDateTime(datetime):
            @classmethod
            def now(cls, tz=None):
                return cls(2026, 9, 18, 4, 0, tzinfo=tz or timezone.utc)

        monkeypatch.setattr(import_module("radiust.sources.ca"), "datetime", FixedDateTime)
        root = CaSource.BASE_URL.format(date="20260918")
        responses = {
            root: b'<a href="CASFT/">CASFT/</a>',
            root + "CASFT/": b'<a href="1.5/">1.5/</a>',
            root + "CASFT/1.5/": b'<a href="202609180354_CASFT_CAPPI_1.5_RAIN.gif">RAIN</a>',
            frame["uri"]: expected,
        }

    class Replay:
        async def get(self, url: str, *, headers=None) -> bytes:
            requests.append(url)
            assert url in responses, f"unexpected {source_id} request"
            return responses[url]

    old_context = pipeline.Client._context

    def context(client: Client, requested: str):
        result = old_context(client, requested)
        if requested == source_id:
            result.transport = Replay()
        return result

    monkeypatch.setattr(pipeline.Client, "_context", context)
    with Client(config={"runtime": {"allow_network": False}, "cache": {"enabled": False}}) as client:
        refs = client.discover(Query(source_id, latest=True, stations=(frame["station"],)))
        assert len(refs) == 1
        assert refs[0].valid_time.isoformat().replace("+00:00", "Z") == frame["valid_time"]
        with client.acquire(refs[0]) as raw:
            assert artifact_bytes(raw.artifacts[0]) == expected

    output = tmp_path / f"{source_id}-raw"
    download = CliRunner().invoke(main, [
        "download", source_id, "--latest", "--station", frame["station"],
        "--raw-only", "--no-cache", "--output", str(output), "--json",
    ])
    assert download.exit_code == 0, download.output
    assert json.loads(download.output)["counts"]["written"] == 1
    assert any(path.read_bytes() == expected for path in output.rglob("*") if path.is_file())
    assert requests.count(frame["uri"]) == 2

    preview = CliRunner().invoke(main, ["cat", source_id, "--latest", "--renderer", "text"])
    if source_id == "sg":
        assert preview.exit_code == 0, preview.output
        assert "source=sg" in preview.output
        assert "variable=rain_intensity" in preview.output
        assert "shape=(480, 480)" in preview.output
        assert "units=1" in preview.output
    elif source_id == "pt":
        assert preview.exit_code == 0, preview.output
        assert "source=pt" in preview.output
        assert "variable=rain_intensity" in preview.output
        assert "shape=(1526, 1500)" in preview.output
        assert "units=1" in preview.output
    else:
        assert preview.exit_code != 0
        assert "verified scientific decoder" in preview.output


@pytest.mark.parametrize(
    ("source_id", "station"),
    (("th", "cmp1"), ("th", "kkn240Loop"), ("th_royalrain", "takhli")),
)
def test_thai_registered_adapters_replay_raw_cli(
    source_id: str, station: str, monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    fixture_path = ROOT / "tests/fixtures/sources" / source_id / "fixture.json"
    frames = json.loads(fixture_path.read_text(encoding="utf-8"))["frames"]
    frame = next(item for item in frames if item["station"] == station)
    expected = (fixture_path.parent / frame["artifacts"][0]["path"]).read_bytes()
    if source_id == "th":
        monkeypatch.setattr(ThSource, "RADARS", {
            frame["station"]: (frame["uri"], "https://weather.tmd.go.th/cmpLoop.php"),
        })
        responses = {frame["uri"]: expected}
    else:
        monkeypatch.setattr(import_module("radiust.sources.th_royalrain"), "STATIONS", ("takhli",))
        page = ThRoyalRainSource.BASE_URL + "/?station=takhli"
        responses = {
            page: b'<img src="/opendata/radar_data/cappi/takhli/2026091803420400dBZ.cappi.png">',
            frame["uri"]: expected,
        }
    requested: list[str] = []

    class Replay:
        async def get(self, url: str, *, headers=None) -> bytes:
            requested.append(url)
            assert url in responses
            return responses[url]

    original_context = pipeline.Client._context

    def replay_context(client: Client, requested_source: str):
        context = original_context(client, requested_source)
        if requested_source == source_id:
            context.transport = Replay()
        return context

    monkeypatch.setattr(pipeline.Client, "_context", replay_context)
    with Client(config={"runtime": {"allow_network": False}, "cache": {"enabled": False}}) as client:
        refs = client.discover(Query(source_id, latest=True, stations=(frame["station"],)))
        assert len(refs) == 1
        if source_id == "th_royalrain":
            assert refs[0].valid_time.isoformat().replace("+00:00", "Z") == frame["valid_time"]
        else:
            assert refs[0].valid_time.isoformat(timespec="seconds").replace("+00:00", "Z") == frame["valid_time"]
            assert refs[0].metadata["time_semantics"] == "rendered_footer_ocr_utc"
        with client.acquire(refs[0]) as raw:
            assert artifact_bytes(raw.artifacts[0]) == expected

    output = tmp_path / f"{source_id}-raw"
    run = CliRunner().invoke(main, [
        "download", source_id, "--latest", "--station", frame["station"],
        "--raw-only", "--no-cache", "--output", str(output), "--json",
    ])
    assert run.exit_code == 0, run.output
    assert json.loads(run.output)["counts"]["written"] == 1
    assert any(path.read_bytes() == expected for path in output.rglob("*") if path.is_file())
    # The two standalone SDK calls each fetch once; query-driven CLI download
    # should share discovery bytes with acquisition and fetch only once more.
    assert requested.count(frame["uri"]) == (3 if source_id == "th" else 2)
    preview = CliRunner().invoke(main, ["cat", source_id, "--latest", "--renderer", "text"])
    assert preview.exit_code != 0
    assert "verified scientific decoder" in preview.output


def test_th_latest_cli_download_reuses_both_timestamp_bound_gifs(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    fixture_path = ROOT / "tests/fixtures/sources/th/fixture.json"
    frames = json.loads(fixture_path.read_text(encoding="utf-8"))["frames"]
    raw_root = fixture_path.parent
    responses = {
        frame["uri"]: (raw_root / frame["artifacts"][0]["path"]).read_bytes()
        for frame in frames
    }
    monkeypatch.setattr(
        ThSource,
        "RADARS",
        {frame["station"]: (frame["uri"], "https://weather.tmd.go.th/cmpLoop.php") for frame in frames},
    )
    requested: list[str] = []

    class Replay:
        async def get(self, url: str, *, headers=None) -> bytes:
            requested.append(url)
            return responses[url]

    original_context = pipeline.Client._context

    def replay_context(client: Client, source_id: str):
        context = original_context(client, source_id)
        if source_id == "th":
            context.transport = Replay()
        return context

    monkeypatch.setattr(pipeline.Client, "_context", replay_context)
    output = tmp_path / "th-latest"
    run = CliRunner().invoke(
        main,
        ["download", "th", "--latest", "--raw-only", "--no-cache", "--output", str(output), "--json"],
    )
    assert run.exit_code == 0, run.output
    report = json.loads(run.output)
    assert report["counts"]["written"] == 2
    assert report["counts"]["failed"] == 0
    assert {item["station"]: item["valid_time"] for item in report["items"]} == {
        "cmp1": "2023-04-22T14:45:00.000000Z",
        "kkn240Loop": "2023-04-22T16:00:05.000000Z",
    }
    assert sorted(requested) == sorted(responses)
    actual_hashes = {
        path.name: sha256(path.read_bytes()).hexdigest()
        for path in output.rglob("*.gif")
    }
    assert actual_hashes == {
        frame["artifacts"][0]["name"]: frame["artifacts"][0]["sha256"]
        for frame in frames
    }


def test_windy_registered_adapter_replays_all_four_raw_tiles(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    fixture_path = ROOT / "tests/fixtures/sources/windy/fixture.json"
    frame = json.loads(fixture_path.read_text(encoding="utf-8"))["frames"][0]
    at = datetime.fromisoformat(frame["valid_time"].replace("Z", "+00:00"))
    monkeypatch.setattr(WindySource, "frame_time", lambda self: at)
    source = registry.get("windy")
    responses = {}
    for y in range(2):
        for x in range(2):
            artifact = next(item for item in frame["artifacts"] if item["name"] == f"tile-z1-x{x}-y{y}.png")
            url = source.tile_url(at, x, y, None)
            responses[url] = (fixture_path.parent / artifact["path"]).read_bytes()
    requested: list[str] = []

    class Replay:
        async def get(self, url: str, *, headers=None) -> bytes:
            requested.append(url)
            assert url in responses
            return responses[url]

    original_context = pipeline.Client._context

    def replay_context(client: Client, source_id: str):
        context = original_context(client, source_id)
        if source_id == "windy":
            context.transport = Replay()
        return context

    monkeypatch.setattr(pipeline.Client, "_context", replay_context)
    with Client(config={"runtime": {"allow_network": False}, "cache": {"enabled": False}}) as client:
        refs = client.discover(Query("windy", latest=True))
        assert len(refs) == 1
        assert refs[0].valid_time == at
        with client.acquire(refs[0]) as raw:
            assert [artifact_bytes(artifact) for artifact in raw.artifacts] == [
                (fixture_path.parent / artifact["path"]).read_bytes() for artifact in frame["artifacts"]
            ]

    output = tmp_path / "windy-raw"
    run = CliRunner().invoke(main, [
        "download", "windy", "--latest", "--raw-only", "--no-cache",
        "--output", str(output), "--json",
    ])
    assert run.exit_code == 0, run.output
    assert json.loads(run.output)["counts"]["written"] == 1
    assert len(list(output.rglob("*.png"))) == 4
    assert len(requested) == 8
    preview = CliRunner().invoke(main, ["cat", "windy", "--latest", "--renderer", "text"])
    assert preview.exit_code != 0
    assert "verified scientific decoder" in preview.output


def test_my_registered_adapter_cli_preserves_provider_bytes_without_fixture_fallback(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """Exercise MY's provider adapter in the CLI using a controlled HTTP transcript."""
    from io import BytesIO

    from PIL import Image
    from radiust.sources.my import MySource
    from radiust.transport import HTTPResponse

    image_buffer = BytesIO()
    Image.new("RGB", (3, 2), (40, 80, 120)).save(image_buffer, format="PNG")
    payload = image_buffer.getvalue()
    times = {
        "peninsular": "Mon, 21 Sep 2026 01:09:00 GMT",
        "east": "Mon, 21 Sep 2026 01:11:00 GMT",
    }

    class ReplayTransport:
        def __init__(self) -> None:
            self.head_urls: list[str] = []
            self.get_urls: list[str] = []

        async def head_response(self, url: str, *, headers=None) -> HTTPResponse:
            self.head_urls.append(url)
            station = next(name for name, station_url in MySource.STATIONS.items() if station_url == url)
            return HTTPResponse(b"", 200, url, {"content-type": "image/gif", "last-modified": times[station]})

        async def get(self, url: str, *, headers=None) -> bytes:
            self.get_urls.append(url)
            return payload

    transport = ReplayTransport()
    source = MySource(registry.get_info("my"))
    original_context = pipeline.Client._context
    original_get_source = pipeline.get_source

    def replay_context(client: Client, source_id: str):
        context = original_context(client, source_id)
        if source_id == "my":
            context.transport = transport
        return context

    monkeypatch.setattr(pipeline, "get_source", lambda source_id: source if source_id == "my" else original_get_source(source_id))
    monkeypatch.setattr(pipeline.Client, "_context", replay_context)
    config_path = tmp_path / "network.yml"
    config_path.write_text("runtime:\n  allow_network: true\n", encoding="utf-8")
    output = tmp_path / "my-cli-raw"

    result = CliRunner().invoke(
        main,
        [
            "--conf",
            str(config_path),
            "download",
            "my",
            "--latest",
            "--output",
            str(output),
            "--raw-only",
            "--no-cache",
            "--json",
        ],
    )

    assert result.exit_code == 0, result.output
    report = json.loads(result.output)
    assert report["counts"]["written"] == 2
    assert {item["station"] for item in report["items"]} == {"east", "peninsular"}
    assert {item["valid_time"] for item in report["items"]} == {
        "2026-09-21T01:00:00.000000Z",
        "2026-09-21T01:02:00.000000Z",
    }
    artifacts = list(output.rglob("*.png"))
    assert len(artifacts) == 2
    assert all(path.read_bytes() == payload for path in artifacts)
    raw_manifests = [json.loads(path.read_text(encoding="utf-8")) for path in output.rglob("raw-manifest.json")]
    assert len(raw_manifests) == 2
    for manifest in raw_manifests:
        station = manifest["ref"]["station"]
        assert manifest["artifacts"][0]["media_type"] == "image/png"
        assert manifest["metadata"]["provider_content_type"] == "image/gif"
        assert manifest["metadata"]["payload_media_type"] == "image/png"
        assert manifest["metadata"]["content_type_mismatch"] is True
        assert manifest["metadata"]["provider_last_modified"] == times[station]
    assert set(transport.head_urls) == set(MySource.STATIONS.values())
    assert sorted(transport.get_urls) == sorted(MySource.STATIONS.values())


@pytest.mark.parametrize("source_id,fixture_path,frame", RAW_CASES, ids=[item[0] for item in RAW_CASES])
def test_source_cli_sdk_matrix_uses_hashed_raw_fixtures(
    source_id: str,
    fixture_path: Path,
    frame: dict[str, Any],
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """Exercise shared CLI/SDK plumbing; source-specific tests own science validation."""

    fixture_source = FixtureSource(registry.get_info(source_id), fixture_path)
    original_get_source = pipeline.get_source

    def get_source(requested: str):
        if requested == source_id:
            return fixture_source
        return original_get_source(requested)

    monkeypatch.setattr(pipeline, "get_source", get_source)
    query = _query(source_id, frame)
    config = {"runtime": {"allow_network": False}, "cache": {"enabled": False}}

    with Client(config=config) as client:
        value = client.fetch(query)
        assert value.provenance["source"] == source_id
        assert value.provenance["display_decoded"] is True

    output = tmp_path / "cli-output"
    arguments = [
        "download",
        source_id,
        "--product",
        frame["product"],
        "--at",
        frame["valid_time"],
        "--output",
        str(output),
        "--raw-only",
        "--no-cache",
        "--json",
    ]
    if frame.get("station"):
        arguments.extend(("--station", frame["station"]))
    downloaded = CliRunner().invoke(main, arguments)

    assert downloaded.exit_code == 0, downloaded.output
    report = json.loads(downloaded.output)
    assert report["counts"]["written"] == 1
    assert report["items"][0]["status"] == "written"

    cat_args = ["cat", source_id, "--renderer", "text", "--at", frame["valid_time"]]
    cat_args.extend(("--product", frame["product"]))
    if frame.get("station"):
        cat_args.extend(("--station", frame["station"]))
    preview = CliRunner().invoke(main, cat_args)

    assert preview.exit_code == 0, preview.output
    assert f"source={source_id}" in preview.output
    assert "\x1b[" not in preview.output
