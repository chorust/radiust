#!/usr/bin/env python3
"""Run the manual 30-round, loopback-only Rust raw batch download benchmark."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import signal
import subprocess
import sys
import tempfile
import threading
import time
from collections.abc import Sequence
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
RESULT_PATH = ROOT / "validation-results/rust-migration-batch.json"
TEST_NAME = "batch_download_benchmark_loopback_raw_only_30_rounds"
REQUIRED_ROUNDS = 30
DEFAULT_SAMPLE_INTERVAL = 0.02
DEFAULT_TEST_TIMEOUT = 300.0
DEFAULT_BUILD_TIMEOUT = 1800.0
OUTPUT_PREFIX = "BATCH_BENCHMARK_JSON="


def _percentile(values: Sequence[float], percentile: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = (len(ordered) - 1) * percentile
    lower = math.floor(index)
    upper = math.ceil(index)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (index - lower)


def _tree_file_stats(path: Path) -> tuple[int, int]:
    total_bytes = 0
    file_count = 0
    if not path.exists():
        return total_bytes, file_count
    for directory, subdirs, files in os.walk(path, followlinks=False):
        subdirs[:] = [name for name in subdirs if not (Path(directory) / name).is_symlink()]
        for name in files:
            child = Path(directory) / name
            try:
                if not child.is_symlink() and child.is_file():
                    total_bytes += child.stat().st_size
                    file_count += 1
            except OSError:
                continue
    return total_bytes, file_count


def _parse_process_snapshot(output: str, root_pid: int) -> tuple[int | None, set[int]]:
    parents: dict[int, int] = {}
    rss_kib: dict[int, int] = {}
    for line in output.splitlines():
        fields = line.split()
        if len(fields) != 4:
            continue
        try:
            pid, ppid, rss, _pgid = (int(value) for value in fields)
        except ValueError:
            continue
        parents[pid] = ppid
        rss_kib[pid] = rss
    if root_pid not in parents:
        return None, set()
    members = {root_pid}
    while True:
        expanded = members | {pid for pid, ppid in parents.items() if ppid in members}
        if expanded == members:
            break
        members = expanded
    return sum(rss_kib.get(pid, 0) for pid in members) * 1024, members - {root_pid}


def _sample_process_tree(
    root_pid: int,
    stop: threading.Event,
    peak_bytes: list[int | None],
    status: list[str],
    observed_descendants: set[int],
    interval: float,
) -> None:
    while not stop.is_set():
        try:
            snapshot = subprocess.run(
                ["ps", "-axo", "pid=,ppid=,rss=,pgid="],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                encoding="ascii",
                errors="replace",
                timeout=max(1.0, interval * 10),
                check=False,
            )
            if snapshot.returncode == 0:
                current, descendants = _parse_process_snapshot(snapshot.stdout, root_pid)
                observed_descendants.update(descendants)
                if current is not None and (peak_bytes[0] is None or current > peak_bytes[0]):
                    peak_bytes[0] = current
                    status[0] = "sampled_ps_process_tree"
            else:
                status[0] = "ps_failed"
        except subprocess.TimeoutExpired:
            status[0] = "ps_timeout"
        except OSError as exc:
            status[0] = f"ps_unavailable:{type(exc).__name__}"
        stop.wait(interval)


def _live_process_group_members(process_group: int) -> list[dict[str, int | str]] | None:
    try:
        snapshot = subprocess.run(
            ["ps", "-axo", "pid=,pgid=,stat="],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            encoding="ascii",
            errors="replace",
            timeout=5,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if snapshot.returncode != 0:
        return None
    members: list[dict[str, int | str]] = []
    for line in snapshot.stdout.splitlines():
        fields = line.split()
        if len(fields) != 3:
            continue
        try:
            pid, pgid = int(fields[0]), int(fields[1])
        except ValueError:
            continue
        if pgid == process_group and pid != process_group:
            members.append({"pid": pid, "state": fields[2]})
    return members


def _controlled_environment() -> dict[str, str]:
    env = os.environ.copy()
    for key in tuple(env):
        if key.lower() in {"http_proxy", "https_proxy", "all_proxy", "ftp_proxy"}:
            env.pop(key, None)
    env.update(
        {
            "CARGO_NET_OFFLINE": "true",
            "NO_PROXY": "*",
            "no_proxy": "*",
        }
    )
    return env


def _build_test_binary(timeout: float) -> dict[str, Any]:
    command = [
        "cargo",
        "test",
        "--package",
        "radiust-core",
        "--test",
        "download_batch",
        "--no-run",
        "--offline",
        "--locked",
        "--message-format=json-render-diagnostics",
    ]
    started = time.perf_counter()
    try:
        completed = subprocess.run(
            command,
            cwd=ROOT,
            env=_controlled_environment(),
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
        )
        elapsed = time.perf_counter() - started
    except subprocess.TimeoutExpired as exc:
        return {
            "command": command,
            "exit_code": None,
            "timed_out": True,
            "elapsed_seconds": time.perf_counter() - started,
            "executable": None,
            "stdout_tail": _text_tail(exc.stdout),
            "stderr_tail": _text_tail(exc.stderr),
        }

    executable: str | None = None
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = message.get("target", {})
        if (
            message.get("reason") == "compiler-artifact"
            and target.get("name") == "download_batch"
            and "test" in target.get("kind", [])
            and message.get("executable")
        ):
            executable = message["executable"]
    return {
        "command": command,
        "exit_code": completed.returncode,
        "timed_out": False,
        "elapsed_seconds": elapsed,
        "executable": executable,
        "stdout_tail": _text_tail(completed.stdout),
        "stderr_tail": _text_tail(completed.stderr),
    }


def _text_tail(value: str | bytes | None, lines: int = 30) -> list[str]:
    if value is None:
        return []
    if isinstance(value, bytes):
        value = value.decode("utf-8", errors="replace")
    return value.splitlines()[-lines:]


def _run_exact_test(
    executable: str,
    *,
    timeout: float,
    sample_interval: float,
) -> dict[str, Any]:
    command = [executable, "--exact", TEST_NAME, "--ignored", "--nocapture"]
    with tempfile.TemporaryDirectory(prefix="radiust-rust-batch-bench-") as temporary:
        isolated_tmp = Path(temporary) / "tmp"
        isolated_tmp.mkdir()
        env = _controlled_environment()
        env.update({"TMPDIR": str(isolated_tmp), "TMP": str(isolated_tmp), "TEMP": str(isolated_tmp)})
        started = time.perf_counter()
        try:
            process = subprocess.Popen(
                command,
                cwd=ROOT,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                errors="replace",
                start_new_session=True,
                shell=False,
            )
        except OSError as exc:
            return {
                "command": command,
                "exit_code": None,
                "launch_error": type(exc).__name__,
                "timed_out": False,
                "elapsed_seconds": time.perf_counter() - started,
                "test_output": None,
            }

        peak_tree_rss: list[int | None] = [None]
        rss_status = ["target_not_observed"]
        observed_descendants: set[int] = set()
        rss_stop = threading.Event()
        rss_thread = threading.Thread(
            target=_sample_process_tree,
            args=(
                process.pid,
                rss_stop,
                peak_tree_rss,
                rss_status,
                observed_descendants,
                max(0.05, sample_interval),
            ),
            name="rust-batch-rss-sampler",
            daemon=True,
        )
        rss_thread.start()

        temp_peak_bytes = 0
        temp_peak_file_count = 0
        temp_sample_count = 0
        direct_child_peak_rss: int | None = None
        timed_out = False
        deadline = started + timeout
        return_code: int | None = None
        if hasattr(os, "wait4"):
            while True:
                current_bytes, current_files = _tree_file_stats(isolated_tmp)
                temp_peak_bytes = max(temp_peak_bytes, current_bytes)
                temp_peak_file_count = max(temp_peak_file_count, current_files)
                temp_sample_count += 1
                try:
                    waited_pid, status, usage = os.wait4(process.pid, os.WNOHANG)
                except InterruptedError:
                    continue
                except ChildProcessError:
                    waited_pid, status, usage = process.pid, None, None
                if waited_pid == process.pid:
                    return_code = os.waitstatus_to_exitcode(status) if status is not None else None
                    process.returncode = return_code
                    if usage is not None:
                        direct_child_peak_rss = int(
                            usage.ru_maxrss if sys.platform == "darwin" else usage.ru_maxrss * 1024
                        )
                    break
                if not timed_out and time.monotonic() >= deadline:
                    timed_out = True
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except (ProcessLookupError, PermissionError, OSError):
                        process.kill()
                time.sleep(sample_interval)
        else:
            while process.poll() is None:
                current_bytes, current_files = _tree_file_stats(isolated_tmp)
                temp_peak_bytes = max(temp_peak_bytes, current_bytes)
                temp_peak_file_count = max(temp_peak_file_count, current_files)
                temp_sample_count += 1
                if time.monotonic() >= deadline:
                    timed_out = True
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except (ProcessLookupError, PermissionError, OSError):
                        process.kill()
                    break
                time.sleep(sample_interval)
            return_code = process.wait()

        current_bytes, current_files = _tree_file_stats(isolated_tmp)
        temp_peak_bytes = max(temp_peak_bytes, current_bytes)
        temp_peak_file_count = max(temp_peak_file_count, current_files)
        temp_sample_count += 1
        rss_stop.set()
        rss_thread.join(timeout=2)
        stdout, stderr = process.communicate()
        live_group_members = _live_process_group_members(process.pid)
        elapsed = time.perf_counter() - started
        remaining_entries = sorted(path.name for path in isolated_tmp.iterdir())

        if peak_tree_rss[0] is None and direct_child_peak_rss is not None:
            peak_tree_rss[0] = direct_child_peak_rss
            rss_status[0] = "direct_child_wait4_fallback_target_exited_before_ps_sample"

        parsed_test_output = None
        for line in stdout.splitlines():
            if line.startswith(OUTPUT_PREFIX):
                try:
                    parsed_test_output = json.loads(line[len(OUTPUT_PREFIX) :])
                except json.JSONDecodeError:
                    parsed_test_output = None

        return {
            "command": command,
            "exit_code": return_code,
            "timed_out": timed_out,
            "elapsed_seconds": elapsed,
            "stdout_sha256": hashlib.sha256(stdout.encode("utf-8")).hexdigest(),
            "stderr_sha256": hashlib.sha256(stderr.encode("utf-8")).hexdigest(),
            "stdout_tail": _text_tail(stdout),
            "stderr_tail": _text_tail(stderr),
            "exactly_one_test_started": "running 1 test" in stdout,
            "test_output": parsed_test_output,
            "direct_child_peak_rss_bytes": direct_child_peak_rss,
            "process_tree_peak_rss_bytes": peak_tree_rss[0],
            "process_tree_rss_measurement": {
                "method": "sampled ps RSS sum for the test process and observed descendants",
                "sample_interval_seconds": max(0.05, sample_interval),
                "status": rss_status[0],
                "may_miss_short_lived_descendants": True,
                "observed_descendant_pids": sorted(observed_descendants),
            },
            "temporary_directory_peak": {
                "path_isolated": True,
                "measurement": "maximum sampled logical bytes and regular-file count under TMPDIR",
                "sample_interval_seconds": sample_interval,
                "sample_count": temp_sample_count,
                "peak_regular_file_bytes": temp_peak_bytes,
                "peak_regular_file_count": temp_peak_file_count,
                "remaining_entries_after_test": remaining_entries,
                "status": "sampled" if temp_sample_count else "unavailable_no_samples",
            },
            "process_group_cleanup": {
                "method": "ps process-group scan after direct test process exit",
                "remaining_members": live_group_members,
                "status": (
                    "no_remaining_members"
                    if live_group_members == []
                    else "remaining_members"
                    if live_group_members is not None
                    else "unavailable"
                ),
            },
        }


def _summarize(test_output: dict[str, Any] | None) -> dict[str, Any]:
    if not test_output:
        return {"round_count": 0, "all_rounds_passed": False}
    rounds = test_output.get("rounds")
    if not isinstance(rounds, list):
        return {"round_count": 0, "all_rounds_passed": False}
    elapsed = [float(row["elapsed_seconds"]) for row in rounds if isinstance(row, dict) and "elapsed_seconds" in row]
    counts = [int(row["request_count"]) for row in rounds if isinstance(row, dict) and "request_count" in row]
    concurrency = [
        int(row["observed_server_concurrency"])
        for row in rounds
        if isinstance(row, dict) and "observed_server_concurrency" in row
    ]
    valid = (
        len(rounds) == REQUIRED_ROUNDS
        and len(elapsed) == REQUIRED_ROUNDS
        and all(row.get("input_order_verified") is True for row in rounds)
        and all(row.get("manifest_and_artifact_sha256_readback_verified") is True for row in rounds)
        and all(row.get("temp_residue_count") == 0 for row in rounds)
        and all(row.get("request_count") == test_output.get("frames_per_round") for row in rounds)
        and all(
            1 < row.get("observed_server_concurrency", 0) <= test_output.get("configured_concurrency_limit", 0)
            for row in rounds
        )
        and test_output.get("listener_host") == "127.0.0.1"
        and test_output.get("public_network_used") is False
    )
    return {
        "round_count": len(rounds),
        "all_rounds_passed": valid,
        "p50_download_and_commit_seconds": _percentile(elapsed, 0.50),
        "p95_download_and_commit_seconds": _percentile(elapsed, 0.95),
        "total_http_requests": sum(counts),
        "observed_server_concurrency_min": min(concurrency) if concurrency else None,
        "observed_server_concurrency_max": max(concurrency) if concurrency else None,
        "all_input_orders_verified": all(row.get("input_order_verified") is True for row in rounds),
        "all_manifest_and_artifact_readbacks_verified": all(
            row.get("manifest_and_artifact_sha256_readback_verified") is True for row in rounds
        ),
        "all_temp_roots_empty": all(row.get("temp_residue_count") == 0 for row in rounds),
    }


def _write_result(payload: dict[str, Any]) -> None:
    RESULT_PATH.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(payload, ensure_ascii=False, indent=2, allow_nan=False) + "\n"
    with RESULT_PATH.open("x", encoding="utf-8") as stream:
        stream.write(encoded)
        stream.flush()
        os.fsync(stream.fileno())


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-timeout-seconds", type=float, default=DEFAULT_TEST_TIMEOUT)
    parser.add_argument("--build-timeout-seconds", type=float, default=DEFAULT_BUILD_TIMEOUT)
    parser.add_argument("--sample-interval-seconds", type=float, default=DEFAULT_SAMPLE_INTERVAL)
    args = parser.parse_args()
    if (
        not math.isfinite(args.test_timeout_seconds)
        or args.test_timeout_seconds <= 0
        or not math.isfinite(args.build_timeout_seconds)
        or args.build_timeout_seconds <= 0
        or not math.isfinite(args.sample_interval_seconds)
        or args.sample_interval_seconds <= 0
    ):
        parser.error("timeouts and sample interval must be finite and positive")
    if RESULT_PATH.exists():
        parser.error(f"result already exists; refusing to overwrite: {RESULT_PATH}")

    started_at = datetime.now(timezone.utc).isoformat()
    build = _build_test_binary(args.build_timeout_seconds)
    execution: dict[str, Any] | None = None
    if build["exit_code"] == 0 and build.get("executable"):
        execution = _run_exact_test(
            build["executable"],
            timeout=args.test_timeout_seconds,
            sample_interval=args.sample_interval_seconds,
        )
    summary = _summarize(execution.get("test_output") if execution else None)
    gaps: list[dict[str, str]] = []
    if execution is None:
        gaps.append({"measurement": "benchmark_execution", "reason": "build failed or test executable was not reported by Cargo"})
    else:
        if execution.get("process_tree_peak_rss_bytes") is None:
            gaps.append({"measurement": "process_tree_peak_rss", "reason": "ps sampling and wait4 fallback both failed"})
        if execution.get("temporary_directory_peak", {}).get("status") != "sampled":
            gaps.append({"measurement": "temporary_directory_peak", "reason": "the isolated TMPDIR could not be sampled"})
        if execution.get("process_group_cleanup", {}).get("status") == "unavailable":
            gaps.append({"measurement": "remaining_subprocesses", "reason": "post-run process-group scan was unavailable"})

    passed = bool(
        build.get("exit_code") == 0
        and build.get("executable")
        and execution
        and execution.get("exit_code") == 0
        and not execution.get("timed_out")
        and execution.get("exactly_one_test_started")
        and summary.get("all_rounds_passed")
        and execution.get("temporary_directory_peak", {}).get("remaining_entries_after_test") == []
        and execution.get("process_group_cleanup", {}).get("status") == "no_remaining_members"
        and not gaps
    )
    result = {
        "schema_version": 1,
        "benchmark": "radiust-rust-batch-raw-only-loopback",
        "task": "T061",
        "status": "passed" if passed else "failed_or_incomplete",
        "captured_at_utc": started_at,
        "scope": {
            "operation": "raw-only local batch download and commit",
            "provider": "FixtureAdapter synthetic payloads",
            "rounds_required": REQUIRED_ROUNDS,
            "rounds_measured": summary.get("round_count", 0),
            "network_policy": {
                "cargo_build_offline": True,
                "proxy_environment_removed": True,
                "no_proxy": "*",
                "fixture_listener": "127.0.0.1 ephemeral port",
                "public_network_used": False,
            },
        },
        "environment": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": sys.version.split()[0],
            "rustc": _text_tail(
                subprocess.run(
                    ["rustc", "--version"],
                    cwd=ROOT,
                    env=_controlled_environment(),
                    capture_output=True,
                    text=True,
                    check=False,
                ).stdout,
                lines=1,
            ),
        },
        "build": build,
        "execution": execution,
        "summary": summary,
        "measurement_gaps": gaps,
    }
    try:
        _write_result(result)
    except FileExistsError:
        print(f"Refusing to overwrite a concurrently created result: {RESULT_PATH}", file=sys.stderr)
        return 2
    print(json.dumps({"status": result["status"], "result": str(RESULT_PATH), "summary": summary}, indent=2))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
