#!/usr/bin/env python3
"""Loopback fixture server and legacy-Python worker for the migration benchmark.

This helper intentionally defines synthetic adapters in the benchmark process.
It never imports or patches Radiust product files, and its only request URLs
are assembled from the supplied 127.0.0.1 fixture-server address.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import threading
import time
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlsplit

SOURCES = ("au", "es", "fr", "vn")
PRODUCTS = {"au": "composite", "es": "composite", "fr": "composite", "vn": "cmax"}
VALID_TIME = "2026-09-18T05:11:00Z"
DELAYS_SECONDS = {"au": 0.008, "es": 0.012, "fr": 0.016, "vn": 0.020}


class LoopbackFixtureServer:
    """Serve immutable fixture bytes with deterministic per-source delay."""

    def __init__(self, artifact: bytes) -> None:
        self.artifact = artifact
        self._counts: Counter[tuple[str, str, str, str, str]] = Counter()
        self._active: Counter[tuple[str, str, str, str]] = Counter()
        self._peak_active: Counter[tuple[str, str, str, str]] = Counter()
        self._lock = threading.Lock()
        owner = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
                parts = [unquote(part) for part in self.path.split("?")[0].split("/") if part]
                if len(parts) != 5:
                    self.send_error(404)
                    return
                token, runtime, phase, kind, source = parts
                if runtime not in {"python", "native"} or phase not in {"cold", "warm", "batch"}:
                    self.send_error(404)
                    return
                if source not in SOURCES or kind not in {"discover", "artifact"}:
                    self.send_error(404)
                    return
                with owner._lock:
                    owner._counts[(token, runtime, phase, kind, source)] += 1
                    activity_key = (token, runtime, phase, kind)
                    owner._active[activity_key] += 1
                    owner._peak_active[activity_key] = max(
                        owner._peak_active[activity_key], owner._active[activity_key]
                    )
                try:
                    if kind == "discover":
                        time.sleep(DELAYS_SECONDS[source])
                        payload = json.dumps(
                            {
                                "source": source,
                                "product": PRODUCTS[source],
                                "station": None,
                                "valid_time": VALID_TIME,
                                "revision": "fixed-fixture-r1",
                            },
                            sort_keys=True,
                            separators=(",", ":"),
                        ).encode("utf-8")
                        content_type = "application/json"
                    else:
                        payload = owner.artifact
                        content_type = "image/png"
                    self.send_response(200)
                    self.send_header("Content-Type", content_type)
                    self.send_header("Content-Length", str(len(payload)))
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(payload)
                finally:
                    with owner._lock:
                        owner._active[activity_key] -= 1

            def log_message(self, _format: str, *_args: object) -> None:
                return

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        self._thread = threading.Thread(target=self._server.serve_forever, name="radiust-loopback-fixture", daemon=True)

    @property
    def base_url(self) -> str:
        host, port = self._server.server_address
        if host != "127.0.0.1":
            raise RuntimeError("fixture server did not bind to IPv4 loopback")
        return f"http://127.0.0.1:{port}"

    def start(self) -> None:
        self._thread.start()

    def close(self) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=2)

    def counts(self, token: str) -> dict[str, Any]:
        with self._lock:
            selected = {
                (runtime, phase, kind, source): count
                for (run_token, runtime, phase, kind, source), count in self._counts.items()
                if run_token == token
            }
        by_source = {
            f"{runtime}.{phase}.{kind}.{source}": count
            for (runtime, phase, kind, source), count in sorted(selected.items())
        }
        by_operation: Counter[tuple[str, str, str]] = Counter()
        for (runtime, phase, kind, _source), count in selected.items():
            by_operation[(runtime, phase, kind)] += count
        with self._lock:
            peaks = {
                (runtime, phase, kind): count
                for (run_token, runtime, phase, kind), count in self._peak_active.items()
                if run_token == token
            }
        return {
            "by_runtime_phase_kind": {
                f"{runtime}.{phase}.{kind}": count
                for (runtime, phase, kind), count in sorted(by_operation.items())
            },
            "by_runtime_phase_kind_source": by_source,
            "peak_concurrent_by_runtime_phase_kind": {
                f"{runtime}.{phase}.{kind}": count
                for (runtime, phase, kind), count in sorted(peaks.items())
            },
            "total": sum(by_source.values()),
        }


def _run_python_helper(args: argparse.Namespace) -> int:
    # Imports are intentionally deferred: the parent imports this helper while
    # running the current benchmark, but the worker must resolve Radiust from
    # the archived pre-migration source tree supplied in PYTHONPATH.
    from radiust.config import load_config
    from radiust.discovery import discover_all
    from radiust.models import Artifact, FrameRef, Query
    from radiust.pipeline import AsyncClient
    from radiust.raw import RawFrame
    from radiust.registry import SourceRegistry, registry
    from radiust.sources.base import Source

    helper_started = time.perf_counter()
    runtime_root = Path(args.runtime_root).resolve()
    output_root = runtime_root / "output"
    cache_root = runtime_root / "cache"
    temp_root = runtime_root / "tmp"
    for path in (output_root, cache_root, temp_root):
        path.mkdir(parents=True, exist_ok=True)
    offline_config = load_config(
        {
            "runtime": {"allow_network": False, "temp_root": str(temp_root / "offline")},
            "cache": {"dir": str(cache_root / "offline"), "enabled": False},
            "storage": {"output": str(output_root / "offline")},
        }
    )
    fixture_config = load_config(
        {
            "runtime": {
                "allow_network": True,
                "frame_concurrency": 4,
                "request_concurrency": 8,
                "host_concurrency": 4,
                "temp_root": str(temp_root / "loopback"),
            },
            "cache": {"dir": str(cache_root / "loopback"), "enabled": False},
            "storage": {"output": str(output_root / "loopback")},
        }
    )
    base_url = args.server
    parsed_server = urlsplit(base_url)
    if parsed_server.scheme != "http" or parsed_server.hostname != "127.0.0.1" or parsed_server.port is None:
        raise ValueError("fixture server URL must be an explicit http://127.0.0.1:<port> address")
    phase = {"value": "cold"}

    class ReplaySource(Source):
        def __init__(self, info: Any) -> None:
            self.info = info

        def _url(self, kind: str) -> str:
            return f"{base_url}/{args.token}/python/{phase['value']}/{kind}/{self.info.id}"

        def _artifact_url(self) -> str:
            return f"{base_url}/{args.token}/python/batch/artifact/{self.info.id}"

        async def discover(self, query: Query, context: Any) -> list[FrameRef]:
            body = await context.transport.get(self._url("discover"))
            row = json.loads(body)
            return [
                FrameRef(
                    source=self.info.id,
                    product=str(row["product"]),
                    station=row.get("station"),
                    valid_time=__import__("datetime").datetime.fromisoformat(
                        str(row["valid_time"]).replace("Z", "+00:00")
                    ),
                    uri=None,
                    locator={
                        "url": self._artifact_url(),
                        "name": "fixed-frame.png",
                        "media_type": "image/png",
                    },
                    locator_version="1",
                    revision=str(row["revision"]),
                )
            ]

        async def download(self, ref: FrameRef, context: Any) -> RawFrame:
            payload = await context.transport.get(str(ref.locator["url"]))
            artifact = Artifact(
                name="fixed-frame.png",
                role="data",
                media_type="image/png",
                payload=payload,
                size_bytes=len(payload),
                sha256=hashlib.sha256(payload).hexdigest(),
            )
            return RawFrame(ref, (artifact,))

        def decode(self, raw: RawFrame, context: Any) -> dict[str, Any]:
            return {
                "artifacts": [
                    {
                        "name": artifact.name,
                        "size_bytes": len(artifact.payload)
                        if isinstance(artifact.payload, bytes)
                        else Path(artifact.payload).stat().st_size,
                        "sha256": artifact.sha256
                        or hashlib.sha256(
                            artifact.payload
                            if isinstance(artifact.payload, bytes)
                            else Path(artifact.payload).read_bytes()
                        ).hexdigest(),
                    }
                    for artifact in raw.artifacts
                ]
            }

    for source_id in SOURCES:
        registry._instances.pop(source_id, None)
        registry._factories[source_id] = lambda info: ReplaySource(info)

    async def run() -> dict[str, Any]:
        client = AsyncClient(config=fixture_config)
        try:
            timings: dict[str, Any] = {}
            source_catalog: SourceRegistry | None = None

            for name in ("list_sources", "discover_all"):
                values: list[dict[str, Any]] = []
                for mode in ("cold", "warm"):
                    started = time.perf_counter()
                    if name == "list_sources":
                        if source_catalog is None:
                            source_catalog = SourceRegistry()
                        source_infos = source_catalog.infos()
                        semantic = {
                            "sources": [
                                {
                                    "id": info.id,
                                    "description": info.description,
                                    "availability": info.availability,
                                    "products": sorted(product.id for product in info.products),
                                }
                                for info in source_infos
                            ]
                        }
                        semantic["sources"].sort(key=lambda item: item["id"])
                    else:
                        report = discover_all(offline_config)
                        semantic = {"command": "discover", "items": report.get("items", [])}
                    values.append(
                        {
                            "mode": mode,
                            "elapsed_seconds": time.perf_counter() - started,
                            "semantic": semantic,
                        }
                    )
                timings[name] = values

            refs_by_source: dict[str, FrameRef] = {}
            for mode in ("cold", "warm"):
                phase["value"] = mode
                started = time.perf_counter()
                per_source = []
                for source_id in SOURCES:
                    per_source.append(await client.discover(Query(source_id, latest=True)))
                elapsed = time.perf_counter() - started
                rows: list[dict[str, Any]] = []
                for refs in per_source:
                    for ref in refs:
                        refs_by_source[ref.source] = ref
                        rows.append(
                            {
                                "source": ref.source,
                                "product": ref.product,
                                "station": ref.station,
                                "status": "success",
                                "valid_time": ref.valid_time.isoformat().replace("+00:00", "Z"),
                                "logical_id": ref.logical_id,
                            }
                        )
                rows.sort(key=lambda row: row["source"])
                timings.setdefault("four_delayed_sources", []).append(
                    {"mode": mode, "elapsed_seconds": elapsed, "semantic": {"items": rows}}
                )

            frames = [refs_by_source[source_id] for source_id in SOURCES]
            started = time.perf_counter()
            semaphore = asyncio.Semaphore(4)

            async def acquire_one(ref: FrameRef) -> dict[str, Any]:
                async with semaphore, client.acquire(ref) as raw:
                    return {
                        "source": ref.source,
                        "logical_id": ref.logical_id,
                        "status": "success",
                        "artifacts": [
                            {
                                "name": artifact.name,
                                "size_bytes": artifact.size_bytes,
                                "sha256": artifact.sha256,
                            }
                            for artifact in raw.artifacts
                        ],
                    }

            batch_rows = await asyncio.gather(*(acquire_one(ref) for ref in frames))
            batch_rows.sort(key=lambda row: row["source"])
            timings["batch_acquisition"] = {
                "mode": "warm_engine_raw_acquisition",
                "elapsed_seconds": time.perf_counter() - started,
                "semantic": {"items": batch_rows},
            }
            await client.aclose()
            return {
                "runtime": "python",
                "setup_seconds": time.perf_counter() - helper_started,
                "operations": timings,
            }
        finally:
            if not client._closed:
                await client.aclose()

    result = asyncio.run(run())
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", choices=("python",), required=True)
    parser.add_argument("--server", required=True)
    parser.add_argument("--token", required=True)
    parser.add_argument("--runtime-root", required=True)
    args = parser.parse_args(argv)
    parsed_server = urlsplit(args.server)
    if parsed_server.scheme != "http" or parsed_server.hostname != "127.0.0.1" or parsed_server.port is None:
        parser.error("--server must be an explicit http://127.0.0.1:<port> fixture URL")
    return _run_python_helper(args)


if __name__ == "__main__":
    raise SystemExit(main())
