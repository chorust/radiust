#!/usr/bin/env python3
"""Compare the legacy buffered HTTP path with Rust streaming on loopback artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import subprocess
import sys
import tempfile
import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUTPUT = ROOT / "validation-results/rust-migration-large-artifact.json"
PYTHON_WORKER = r'''
import concurrent.futures, hashlib, json, shutil, sys, threading
from pathlib import Path
from radiust.transport import HTTPTransport

base, frames, size, temp, sink = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), Path(sys.argv[4]), Path(sys.argv[5])
temp.mkdir(parents=True, exist_ok=True)
sink.mkdir(parents=True, exist_ok=True)
transport = HTTPTransport(allow_network=False, max_bytes=size, request_concurrency=frames, host_concurrency=frames)
temp_lock = threading.Lock()
peak_temp = [0]

def digest_file(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(64 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()

def fetch(index):
    body = transport.get_sync(f'{base}/artifact/{index}')
    digest = hashlib.sha256(body).hexdigest()
    stage = temp / f'artifact-{index}.part'
    stage.write_bytes(body)
    with temp_lock:
        peak_temp[0] = max(peak_temp[0], sum(path.stat().st_size for path in temp.iterdir() if path.is_file()))
    del body
    destination = sink / 'generation' / f'artifact-{index}.bin'
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(stage, destination)
    stage.unlink()
    if destination.stat().st_size != size or digest_file(destination) != digest:
        raise RuntimeError('artifact sink readback differs from downloaded payload')
    return digest

with concurrent.futures.ThreadPoolExecutor(max_workers=frames) as pool:
    fingerprints = list(pool.map(fetch, range(frames)))
if list(temp.iterdir()):
    raise RuntimeError('temporary directory contains residual entries')
print(json.dumps({'frames': frames, 'bytes_per_artifact': size, 'integrity_ok': True, 'temporary_entries': 0, 'worker_peak_temporary_bytes': peak_temp[0], 'fingerprints': fingerprints}))
'''
RUSAGE_WRAPPER = r'''
import json, resource, subprocess, sys
process = subprocess.Popen(sys.argv[1:], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
stdout, stderr = process.communicate()
sys.stdout.write(stdout)
sys.stderr.write(stderr)
peak = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
peak_bytes = int(peak if sys.platform == 'darwin' else peak * 1024)
sys.stderr.write('\nRADIUST_WAIT4_JSON=' + json.dumps({'peak_rss_bytes': peak_bytes}) + '\n')
sys.exit(process.returncode)
'''


class FixtureState:
    def __init__(self, body_bytes: int) -> None:
        self.body_bytes = body_bytes
        self.lock = threading.Lock()
        self.paths: list[str] = []


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    position = (len(ordered) - 1) * fraction
    lower, upper = math.floor(position), math.ceil(position)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower)


def directory_usage(root: Path) -> tuple[int, int]:
    size = count = 0
    for path in root.rglob("*"):
        try:
            if path.is_file() and not path.is_symlink():
                size += path.stat().st_size
                count += 1
        except OSError:
            pass
    return size, count


def run_child(command: list[str], *, temp_root: Path, output_root: Path, timeout: float) -> dict[str, Any]:
    temp_root.mkdir(parents=True, exist_ok=True)
    output_root.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    for key in tuple(env):
        if key.lower() in {"http_proxy", "https_proxy", "all_proxy", "ftp_proxy"}:
            env.pop(key, None)
    env.pop("RADIUST_TEST_ALLOW_LIVE", None)
    env.update(
        {
            "PYTHONPATH": str(ROOT / "python"),
            "NO_PROXY": "*",
            "no_proxy": "*",
            "TMPDIR": str(temp_root),
            "TMP": str(temp_root),
            "TEMP": str(temp_root),
            "CARGO_NET_OFFLINE": "true",
        }
    )
    started = time.perf_counter()
    process = subprocess.Popen(
        [sys.executable, "-c", RUSAGE_WRAPPER, *command],
        cwd=ROOT,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    temp_peak = temp_files_peak = 0
    deadline = started + timeout
    while process.poll() is None:
        temp_size, temp_files = directory_usage(temp_root)
        temp_peak = max(temp_peak, temp_size)
        temp_files_peak = max(temp_files_peak, temp_files)
        if time.perf_counter() > deadline:
            process.kill()
            break
        time.sleep(0.01)
    stdout, stderr = process.communicate()
    elapsed = time.perf_counter() - started
    if process.returncode != 0:
        raise RuntimeError(f"benchmark worker failed ({process.returncode}): {stderr[-2000:]}")
    usage_lines = [line for line in stderr.splitlines() if line.startswith("RADIUST_WAIT4_JSON=")]
    if not usage_lines:
        raise RuntimeError("wait4 did not return child peak RSS")
    usage = json.loads(usage_lines[-1].split("=", 1)[1])
    peak_rss = int(usage.get("peak_rss_bytes", 0))
    if peak_rss <= 0:
        raise RuntimeError("wait4 returned an invalid peak RSS value")
    lines = [line for line in stdout.splitlines() if line.strip()]
    result = json.loads(lines[-1])
    if result.get("integrity_ok") is not True or result.get("temporary_entries") != 0:
        raise RuntimeError(f"benchmark worker failed its integrity check: {result}")
    if directory_usage(temp_root)[0] != 0:
        raise RuntimeError("worker left temporary files after exit")
    return {
        "elapsed_seconds": elapsed,
        "wait4_child_peak_rss_bytes": peak_rss,
        "sampled_peak_temporary_bytes": temp_peak,
        "worker_peak_temporary_bytes": int(result.get("worker_peak_temporary_bytes", 0)),
        "peak_temporary_bytes": max(temp_peak, int(result.get("worker_peak_temporary_bytes", 0))),
        "sampled_peak_temporary_file_count": temp_files_peak,
        "request_count": len(result.get("fingerprints", [])),
        "fingerprints": result["fingerprints"],
        "integrity_ok": True,
        "temp_clean_after_exit": True,
    }


def build_rust_example(timeout: float) -> Path:
    command = [
        "cargo", "build", "--release", "--offline", "--locked", "-p", "radiust-core",
        "--example", "streaming_artifact_benchmark",
    ]
    completed = subprocess.run(
        command,
        cwd=ROOT,
        env={**os.environ, "CARGO_NET_OFFLINE": "true"},
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        timeout=timeout,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"Rust benchmark example failed to build: {completed.stderr[-3000:]}")
    executable = ROOT / "target/release/examples/streaming_artifact_benchmark"
    if not executable.is_file():
        raise RuntimeError("Cargo build did not produce the streaming benchmark executable")
    return executable


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-mib", type=int, default=8)
    parser.add_argument("--frames", type=int, default=4)
    parser.add_argument("--pairs", type=int, default=10)
    parser.add_argument("--sample-interval-seconds", type=float, default=0.01)
    parser.add_argument("--timeout-seconds", type=float, default=180)
    parser.add_argument("--build-timeout-seconds", type=float, default=1800)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args()
    if not (1 <= args.artifact_mib <= 256 and 1 <= args.frames <= 32 and 5 <= args.pairs <= 30):
        parser.error("artifact-mib must be 1..256, frames 1..32 and pairs 5..30")
    if args.sample_interval_seconds <= 0 or args.timeout_seconds <= 0 or args.build_timeout_seconds <= 0:
        parser.error("timeouts and sample interval must be positive")
    output_path = args.output if args.output.is_absolute() else ROOT / args.output
    if output_path.exists():
        parser.error(f"result already exists; refusing to overwrite: {output_path}")

    body_bytes = args.artifact_mib * 1024 * 1024
    state = FixtureState(body_bytes)

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802
            pieces = self.path.strip("/").split("/")
            if len(pieces) != 2 or pieces[0] != "artifact":
                self.send_error(404)
                return
            try:
                index = int(pieces[1])
            except ValueError:
                self.send_error(404)
                return
            if index < 0 or index >= args.frames:
                self.send_error(404)
                return
            with state.lock:
                state.paths.append(self.path)
            self.send_response(200)
            self.send_header("Content-Length", str(state.body_bytes))
            self.send_header("Connection", "close")
            self.end_headers()
            block = bytes([(index + 17) & 0xFF]) * (64 * 1024)
            remaining = state.body_bytes
            try:
                while remaining:
                    size = min(remaining, len(block))
                    self.wfile.write(block[:size])
                    remaining -= size
            except (BrokenPipeError, ConnectionResetError):
                pass
            self.close_connection = True

        def log_message(self, _format: str, *_args: object) -> None:
            return

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    host, port = server.server_address
    if host != "127.0.0.1":
        raise RuntimeError("fixture server did not bind to loopback")
    base_url = f"http://{host}:{port}"
    rust_executable = build_rust_example(args.build_timeout_seconds)
    samples: dict[str, list[dict[str, Any]]] = {"python_buffered": [], "rust_streamed": []}
    fingerprint_match = True

    try:
        for pair in range(args.pairs):
            order = ("python_buffered", "rust_streamed") if pair % 2 == 0 else ("rust_streamed", "python_buffered")
            for runtime in order:
                with state.lock:
                    state.paths.clear()
                with tempfile.TemporaryDirectory(prefix=f"radiust-large-{runtime}-") as directory:
                    root = Path(directory)
                    temp_root, output_root = root / "tmp", root / "sink"
                    if runtime == "python_buffered":
                        command = [
                            sys.executable,
                            "-c",
                            PYTHON_WORKER,
                            base_url,
                            str(args.frames),
                            str(body_bytes),
                            str(temp_root),
                            str(output_root),
                        ]
                    else:
                        command = [
                            str(rust_executable),
                            base_url,
                            str(args.frames),
                            str(body_bytes),
                            str(temp_root),
                            str(output_root),
                        ]
                    result = run_child(
                        command,
                        temp_root=temp_root,
                        output_root=output_root,
                        timeout=args.timeout_seconds,
                    )
                    with state.lock:
                        paths = list(state.paths)
                    if len(paths) != args.frames or sorted(paths) != sorted(
                        f"/artifact/{index}" for index in range(args.frames)
                    ):
                        raise RuntimeError(f"loopback request set is incomplete or duplicated: {paths}")
                    result["fixture_request_count"] = len(paths)
                    result["fixture_host"] = "127.0.0.1"
                    samples[runtime].append(result)
            fingerprint_match = fingerprint_match and samples["python_buffered"][-1]["fingerprints"] == samples["rust_streamed"][-1]["fingerprints"]
    finally:
        server.shutdown()
        server.server_close()

    def summary(rows: list[dict[str, Any]]) -> dict[str, Any]:
        timings = [row["elapsed_seconds"] for row in rows]
        rss = [row["wait4_child_peak_rss_bytes"] for row in rows]
        temp = [row["peak_temporary_bytes"] for row in rows]
        return {
            "sample_count": len(rows),
            "elapsed_p50_seconds": percentile(timings, 0.50),
            "elapsed_p95_seconds": percentile(timings, 0.95),
            "wait4_child_peak_rss_p50_bytes": percentile(rss, 0.50),
            "wait4_child_peak_rss_max_bytes": max(rss),
            "sampled_peak_temp_p50_bytes": percentile(temp, 0.50),
            "sampled_peak_temp_max_bytes": max(temp),
            "worker_observed_peak_temp_p50_bytes": percentile([row["worker_peak_temporary_bytes"] for row in rows], 0.50),
            "request_count_per_sample": args.frames,
            "all_integrity_checks_passed": all(row["integrity_ok"] for row in rows),
            "all_temp_roots_empty_after_exit": all(row["temp_clean_after_exit"] for row in rows),
        }

    summaries = {name: summary(rows) for name, rows in samples.items()}
    rss_not_above = summaries["rust_streamed"]["wait4_child_peak_rss_max_bytes"] <= summaries["python_buffered"]["wait4_child_peak_rss_max_bytes"]
    temp_not_above = summaries["rust_streamed"]["sampled_peak_temp_max_bytes"] <= summaries["python_buffered"]["sampled_peak_temp_max_bytes"]
    report = {
        "schema_version": 1,
        "benchmark": "radiust-large-artifact-loopback-streaming",
        "task": "T059",
        "status": "passed" if fingerprint_match and rss_not_above and temp_not_above else "failed_or_incomplete",
        "captured_at_utc": datetime.now(timezone.utc).isoformat(),
        "scope": {
            "provider": "synthetic loopback HTTP plus local filesystem object sink",
            "artifact_bytes": body_bytes,
            "frames_per_sample": args.frames,
            "paired_samples": args.pairs,
            "configured_concurrency": args.frames,
            "network_policy": {"bind": "127.0.0.1", "public_network_used": False, "proxy_environment_removed": True},
            "python_path": "legacy HTTPTransport.get_sync buffers response chunks into bytes before writing a staged file and local sink",
            "rust_path": "HttpTransport.get_to_path streams response to a staged file; ObjectStore.write_path_cancellable streams to OpenDAL filesystem sink",
            "measurement": "wait4 RUSAGE_CHILDREN ru_maxrss for the single benchmark worker plus directory polling and worker-observed staged-file byte peaks; target worker spawns no child processes",
        },
        "environment": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": sys.version.split()[0],
            "rustc": subprocess.run(["rustc", "--version"], cwd=ROOT, capture_output=True, text=True, check=False).stdout.strip(),
            "git_revision": subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=False).stdout.strip(),
            "rust_example_sha256": sha256_file(ROOT / "crates/radiust-core/examples/streaming_artifact_benchmark.rs"),
            "python_transport_sha256": sha256_file(ROOT / "python/radiust/transport.py"),
            "rust_executable_sha256": sha256_file(rust_executable),
        },
        "method": {"sample_interval_seconds": args.sample_interval_seconds, "pair_order": "alternates which runtime runs first", "percentile": "linear interpolation at index (n - 1) * p"},
        "measurements": summaries,
        "acceptance": {
            "semantic_fingerprints_match": fingerprint_match,
            "rust_peak_rss_not_above_python_buffered_baseline": rss_not_above,
            "rust_peak_temp_not_above_python_buffered_baseline": temp_not_above,
            "all_requests_loopback_only": all(row["fixture_host"] == "127.0.0.1" for rows in samples.values() for row in rows),
        },
        "samples": samples,
    }
    report["status"] = "passed" if all(report["acceptance"].values()) else "failed_or_incomplete"
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(report, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")
    print(json.dumps({"status": report["status"], "report": str(output_path), "measurements": summaries, "acceptance": report["acceptance"]}, indent=2))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
