"""Offline source benchmark with explicit canonical and synthetic-only results.

Run from the repository root with ``uv run python scripts/validation/benchmark_sources.py
--offline --output validation-results/benchmarks.json``. A subprocess per
measurement gives independent RSS high-water marks while sharing a private
cache between the cold and warm trials. No external network is opened.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import resource
import subprocess
import sys
import tempfile
import threading
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any
from urllib.parse import parse_qs, urlparse

SOURCES = ("my", "id_sidarma", "rainviewer", "au", "ph", "fr")
SYNTHETIC_SOURCES = ("my", "id_sidarma", "ph")
ROOT = Path(__file__).resolve().parents[2]


def _fixture(source: str) -> tuple[Path, dict[str, Any]]:
    path = ROOT / "tests" / "fixtures" / "sources" / source / "fixture.json"
    return path, json.loads(path.read_text(encoding="utf-8"))


def _synthetic_case(source: str) -> dict[str, Any]:
    """Build deterministic request/response data for adapter-path measurements only."""
    if source not in SYNTHETIC_SOURCES:
        raise ValueError(f"no synthetic benchmark case for {source}")

    from io import BytesIO

    from PIL import Image

    output = BytesIO()
    Image.new("RGBA", (4, 3), (10, 20, 30, 255)).save(output, format="PNG")
    raw_bytes = output.getvalue()
    raw_hash = hashlib.sha256(raw_bytes).hexdigest()
    valid_time = "2026-09-22T00:00:00Z"
    head_responses: dict[str, dict[str, str]] = {}
    if source == "my":
        from radiust.sources.my import MySource

        station = "east"
        raw_url = MySource.STATIONS[station]
        head_responses = {
            MySource.STATIONS["peninsular"]: {
                "content-type": "image/gif",
                "last-modified": "Tue, 22 Sep 2026 00:11:00 GMT",
            },
            raw_url: {
                "content-type": "image/gif",
                "last-modified": "Tue, 22 Sep 2026 00:09:00 GMT",
            },
        }
    else:
        station = "CGK" if source == "id_sidarma" else "PHCOMP4"
        raw_url = f"https://benchmark.invalid/{source}.png"
    frame = {
        "product": None,
        "station": station,
        "valid_time": valid_time,
        "artifacts": [{"name": f"{source}.png", "sha256": raw_hash}],
    }

    if source == "my":
        responses = {raw_url: raw_bytes}
        source_config = {}
    elif source == "id_sidarma":
        from radiust.sources.id_sidarma import IdSidarmaSource

        api_url = IdSidarmaSource.API_URL_TEMPLATE.format(radar_id="CGK")
        provider_time = "2026-09-22 00:00 UTC"
        discovery = json.dumps(
            {
                "CMAX": {
                    "LastOneHour": {"file": raw_url, "timeUTC": provider_time},
                    "Latest": {"file": raw_url, "timeUTC": provider_time},
                }
            }
        ).encode("utf-8")
        responses = {api_url: discovery, raw_url: raw_bytes}
        source_config = {"radar_ids": ["CGK"], "api_key": "benchmark-only-key"}
    else:
        from urllib.parse import urlencode

        from radiust.sources.ph import PhSource

        token = "benchmark-only-token"
        timeline_url = f"{PhSource.TIMELINE_URL}?{urlencode({'token': token})}"
        timeline = json.dumps(
            {
                "data": {
                    "timeline": [
                        {
                            "observed_at": "2026-09-22 08:00:00",
                            "image_url": raw_url,
                        }
                    ]
                }
            }
        ).encode("utf-8")
        responses = {
            PhSource.BASE_URL: b'<meta name="csrf-token" content="synthetic-csrf">',
            timeline_url: timeline,
            raw_url: raw_bytes,
        }
        source_config = {"timeline_token": token}

    fingerprint_doc = {
        "source": source,
        "frame": frame,
        "raw_sha256": raw_hash,
        "responses_sha256": sorted(hashlib.sha256(value).hexdigest() for value in responses.values()),
        "head_responses": head_responses,
        "provenance": "generated synthetic replay, not provider data",
    }
    fingerprint = hashlib.sha256(
        json.dumps(fingerprint_doc, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()
    return {
        "frame": frame,
        "responses": responses,
        "head_responses": head_responses,
        "source_config": source_config,
        "payloads": {raw_hash: raw_bytes},
        "fingerprint": fingerprint,
    }


def _bytes_on_disk(path: Path) -> int:
    return sum(item.stat().st_size for item in path.rglob("*") if item.is_file())


def _rss_bytes() -> int:
    peak = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    # Darwin reports bytes; Linux reports KiB.
    return int(peak if sys.platform == "darwin" else peak * 1024)


def _measure(source: str, cache_dir: Path) -> dict[str, Any]:
    # The installed package is loaded only by the worker, keeping startup
    # costs out of the operation timing and giving each run its own RSS peak.
    import radiust.pipeline as pipeline
    from radiust import Client, Query, _core
    from radiust.identity import artifact_bytes
    from radiust.registry import get_source
    from radiust.sources.au import AuSource
    from radiust.sources.base import FixtureSource
    from radiust.sources.fr import FrSource

    fixture_path, fixture = _fixture(source)
    synthetic = _synthetic_case(source) if source in SYNTHETIC_SOURCES else None
    frame = synthetic["frame"] if synthetic is not None else fixture["frames"][0]
    at = datetime.fromisoformat(frame["valid_time"].replace("Z", "+00:00"))
    query = Query(
        source, product=frame["product"],
        stations=(frame["station"],) if frame.get("station") else (),
        latest=source == "fr" or synthetic is not None,
        at=None if source == "fr" or synthetic is not None else at,
    )
    request_urls: list[str] = []
    contexts: list[Any] = []
    actual_get_source = pipeline.get_source
    actual_context = pipeline.Client._context
    original_ftp_nlst = _core.ftp_nlst
    original_ftp_read = _core.ftp_read
    original_frame_time = FrSource.__dict__["_current_frame_time"]
    expected_bytes = (
        synthetic["payloads"]
        if synthetic is not None
        else {
            artifact["sha256"]: (fixture_path.parent / artifact["path"]).read_bytes()
            for artifact in frame["artifacts"]
        }
    )

    def context(client: Client, source_id: str):
        result = actual_context(client, source_id)
        if source_id == source:
            contexts.append(result)
        return result

    if source == "rainviewer":
        discovery = fixture["discovery"]
        host = frame["locator"]["host"]
        path = frame["locator"]["path"]
        manifest = {
            "version": discovery["version"], "generated": discovery["generated"],
            "host": host,
            "radar": {"past": [{"time": int(at.timestamp()), "path": path}], "nowcast": []},
        }
        responses = {discovery["api_url"]: json.dumps(manifest).encode("utf-8")}
        for artifact in frame["artifacts"]:
            name = artifact["name"].removesuffix(".png").split("-")
            x, y = int(name[-2][1:]), int(name[-1][1:])
            url = f"{host}{path}/512/1/{x}/{y}/2/0_0.png"
            responses[url] = (fixture_path.parent / artifact["path"]).read_bytes()

        class ReplayTransport:
            async def get(self, url: str) -> bytes:
                request_urls.append(url)
                if url not in responses:
                    raise AssertionError("unexpected replay request")
                return responses[url]

        def context(client: Client, source_id: str):
            result = actual_context(client, source_id)
            if source_id == source:
                result.transport = ReplayTransport()
                contexts.append(result)
            return result

        pipeline.Client._context = context
        scope = "registered_adapter_discover_acquire_decode_official_replay"
    elif source == "au":
        filename = frame["artifacts"][0]["name"]

        async def ftp_nlst(address: str, *args: Any) -> list[str]:
            request_urls.append(address)
            if address != AuSource.FTP_ROOT:
                raise AssertionError("unexpected FTP listing address")
            return [filename]

        async def ftp_read(address: str, *args: Any) -> bytes:
            request_urls.append(address)
            if address != AuSource.FTP_ROOT + filename:
                raise AssertionError("unexpected FTP artifact address")
            return next(iter(expected_bytes.values()))

        _core.ftp_nlst = ftp_nlst
        _core.ftp_read = ftp_read
        pipeline.Client._context = context
        scope = "registered_adapter_discover_acquire_official_ftp_raw_replay_no_scientific_decode"
    elif source == "fr":
        from types import SimpleNamespace

        class ReplayTransport:
            async def get_response(self, url: str, *, headers: Any = None) -> Any:
                request_urls.append(url)
                if url != FrSource.PAGE_URL:
                    raise AssertionError("unexpected WMS discovery address")
                return SimpleNamespace(headers={"set-cookie": "mfsession=BenchReplay123; Path=/; Secure"}, status=200, url=url)

            async def get(self, url: str, *, headers: Any = None) -> bytes:
                request_urls.append(url)
                if not url.startswith(FrSource.WMS_BASE_URL + "?") or parse_qs(urlparse(url).query).get("time") != [at.strftime("%Y-%m-%dT%H:%M:%SZ")]:
                    raise AssertionError("unexpected WMS artifact address/time")
                return next(iter(expected_bytes.values()))

        def context(client: Client, source_id: str):
            result = actual_context(client, source_id)
            if source_id == source:
                result.transport = ReplayTransport()
                contexts.append(result)
            return result

        FrSource._current_frame_time = staticmethod(lambda now=None: at)
        pipeline.Client._context = context
        scope = "registered_adapter_discover_acquire_historical_wms_raw_replay_no_scientific_decode"
    elif synthetic is not None:
        from types import SimpleNamespace

        class ReplayTransport:
            async def head_response(self, url: str, *, headers: Any = None) -> Any:
                request_urls.append(url)
                response_headers = synthetic["head_responses"].get(url)
                if response_headers is None:
                    raise AssertionError("unexpected synthetic HEAD request")
                return SimpleNamespace(status=200, url=url, headers=response_headers)

            async def get(self, url: str, *, headers: Any = None) -> bytes:
                request_urls.append(url)
                if url not in synthetic["responses"]:
                    raise AssertionError("unexpected synthetic replay request")
                return synthetic["responses"][url]

        def context(client: Client, source_id: str):
            result = actual_context(client, source_id)
            if source_id == source:
                result.transport = ReplayTransport()
                contexts.append(result)
            return result

        pipeline.Client._context = context
        scope = "registered_adapter_synthetic_replay_raw_acquisition_no_scientific_decode"
    else:
        # Unverified palettes/geometries cannot produce a scientific benchmark.
        # FixtureSource exercises raw acquisition only, using recorded bytes.
        info = get_source(source).info
        fixture_source = FixtureSource(info, fixture_path)
        pipeline.get_source = lambda name: fixture_source if name == source else actual_get_source(name)
        pipeline.Client._context = context
        scope = "fixture_raw_acquisition_only_not_source_adapter_or_scientific_decode"

    with tempfile.TemporaryDirectory(prefix="radiust-bench-worker-") as scratch:
        scratch_path = Path(scratch)
        old_tempdir = tempfile.tempdir
        tempfile.tempdir = scratch
        peak_tmp = [0]
        finish = threading.Event()

        def sample_tmp() -> None:
            while not finish.wait(0.01):
                peak_tmp[0] = max(peak_tmp[0], _bytes_on_disk(scratch_path))

        sampler = threading.Thread(target=sample_tmp, daemon=True)
        sampler.start()
        try:
            config: dict[str, Any] = {
                "runtime": {"allow_network": False},
                "cache": {"dir": str(cache_dir), "enabled": True},
            }
            if synthetic is not None:
                config["sources"] = {source: synthetic["source_config"]}
            with Client(config=config) as client:
                start = time.perf_counter_ns()
                if source == "rainviewer":
                    field = client.fetch(query)
                    shape = list(field.data.shape)
                else:
                    refs = client.discover(query)
                    if len(refs) != 1:
                        raise AssertionError("fixture query did not resolve exactly one frame")
                    with client.acquire(refs[0]) as raw:
                        if len(raw.artifacts) != len(frame["artifacts"]):
                            raise AssertionError("fixture artifact count differs")
                        if sorted(hashlib.sha256(artifact_bytes(item)).hexdigest() for item in raw.artifacts) != sorted(expected_bytes):
                            raise AssertionError("acquired artifacts differ from recorded fixture")
                    shape = None
                elapsed = time.perf_counter_ns() - start
        finally:
            finish.set()
            sampler.join()
            peak_tmp[0] = max(peak_tmp[0], _bytes_on_disk(scratch_path))
            tempfile.tempdir = old_tempdir
            pipeline.Client._context = actual_context
            pipeline.get_source = actual_get_source
            _core.ftp_nlst = original_ftp_nlst
            _core.ftp_read = original_ftp_read
            FrSource._current_frame_time = original_frame_time

    input_bytes = (
        sum(len(payload) for payload in expected_bytes.values())
        if synthetic is not None
        else sum((fixture_path.parent / artifact["path"]).stat().st_size for artifact in frame["artifacts"])
    )
    return {
        "scope": scope, "elapsed_seconds": elapsed / 1e9,
        "frames_per_second": 1e9 / elapsed,
        "input_bytes": input_bytes, "input_bytes_per_second": input_bytes * 1e9 / elapsed,
        "peak_rss_bytes": _rss_bytes(), "peak_temporary_bytes_sampled": peak_tmp[0],
        "request_count": len(request_urls), "cache_hits_observed": sum(bool(ctx.metadata.get("cache_hit")) for ctx in contexts),
        "decoded_shape": shape,
    }


def _run_offline() -> dict[str, Any]:
    results: dict[str, Any] = {}
    with tempfile.TemporaryDirectory(prefix="radiust-bench-cache-") as root:
        for source in SOURCES:
            fixture_path, fixture = _fixture(source)
            synthetic = _synthetic_case(source) if source in SYNTHETIC_SOURCES else None
            if (fixture.get("status") == "blocked" or not fixture.get("frames")) and synthetic is None:
                results[source] = {"status": "unavailable", "reason": "no usable canonical fixture frame", "cold": None, "warm": None}
                continue
            frame = synthetic["frame"] if synthetic is not None else fixture["frames"][0]
            if synthetic is None and not all((fixture_path.parent / artifact["path"]).is_file() for artifact in frame.get("artifacts", [])):
                results[source] = {"status": "unavailable", "reason": "recorded raw artifacts missing", "cold": None, "warm": None}
                continue
            cache_dir = Path(root) / source
            trials: dict[str, Any] = {}
            for phase in ("cold", "warm"):
                command = [sys.executable, str(Path(__file__).resolve()), "--_worker", source, "--_cache-dir", str(cache_dir)]
                run = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=False)
                if run.returncode:
                    trials[phase] = {"status": "failed", "reason": run.stderr[-1500:] or f"worker exit {run.returncode}"}
                    break
                trials[phase] = json.loads(run.stdout)
            if synthetic is not None:
                results[source] = {
                    "status": "synthetic_replay_only" if len(trials) == 2 else "failed",
                    "canonical_fixture_status": "unavailable",
                    "synthetic_input_sha256": synthetic["fingerprint"],
                    "fixture_origin": "generated deterministic responses; not provider bytes",
                    "scientific_reference_status": "not applicable: synthetic raw payload cannot validate physical values or geometry",
                    **trials,
                }
            else:
                results[source] = {
                    "status": "measured" if len(trials) == 2 else "failed",
                    "fixture_sha256": hashlib.sha256(fixture_path.read_bytes()).hexdigest(),
                    "fixture_origin": fixture.get("origin"),
                    "scientific_reference_status": frame.get("metadata", {}).get("scientific_reference_status"),
                    **trials,
                }
    return {
        "schema_version": 1, "task": "T148", "task_status": "partial",
        "mode": "offline", "measured_at": datetime.now(timezone.utc).isoformat(),
        "environment": {
            "python": platform.python_version(), "platform": platform.platform(),
            "architecture": platform.machine(), "processor": platform.processor(),
        },
        "sources": results,
        "measurement_notes": [
            "Trials run in separate processes with a shared private cache per source; RSS is the process peak including imports.",
            "Temporary bytes are sampled at 10 ms intervals and may miss shorter peaks; cache contents are excluded.",
            "MY, SIDARMA, and PAGASA use deterministic synthetic request/response replays through their registered adapters because no canonical permitted raw fixture is available; these measurements do not use provider bytes or credentials and do not verify scientific values or geometry.",
            "Historical FR replay pins the discovery clock to its recorded frame time; live latest behavior is excluded.",
            "RainViewer replays official tiles through the registered adapter; its tile cache may intentionally have no warm cache hit.",
            "No comparable old-chain benchmark under identical inputs and environment has been run; acceleration is unverified.",
            "T148 remains partial because canonical raw evidence is unavailable for my, id_sidarma, and ph, and no matched old-chain baseline exists; synthetic replay timings are not source-performance substitutes.",
        ],
        "old_chain_comparison": {"status": "unavailable", "reason": "no matched source/fixture/environment baseline"},
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Run recorded fixture benchmark without external network")
    parser.add_argument("--output", type=Path, default=ROOT / "validation-results/benchmarks.json")
    parser.add_argument("--_worker", choices=SOURCES, help=argparse.SUPPRESS)
    parser.add_argument("--_cache-dir", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args._worker:
        if args._cache_dir is None:
            parser.error("internal worker requires a cache directory")
        print(json.dumps(_measure(args._worker, args._cache_dir)))
        return 0
    if not args.offline:
        parser.error("explicit --offline is required; live benchmarking is not configured")
    report = _run_offline()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    canonical = sum(item["status"] == "measured" for item in report["sources"].values())
    synthetic = sum(item["status"] == "synthetic_replay_only" for item in report["sources"].values())
    print(
        f"Wrote {args.output} ({canonical}/{len(SOURCES)} canonical measured; "
        f"{synthetic} synthetic-only adapter replays)"
    )
    return 0 if all(item["status"] != "failed" for item in report["sources"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
