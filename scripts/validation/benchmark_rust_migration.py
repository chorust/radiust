#!/usr/bin/env python3
"""Run paired, offline Python/native CLI benchmarks for the Rust migration.

Each timed sample starts a fresh process with isolated cache, output and temp
directories. The commands are passed as argument vectors (never through a
shell), and Radiust network access is explicitly disabled for every child.
Run with ``--repetitions 30`` (the default) to collect 30 paired samples for
the scenarios currently implemented. T003 remains incomplete until the full
scenario and measurement matrix recorded in the result is implemented.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import hashlib
import json
import math
import os
import platform
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
from collections.abc import Sequence
from contextlib import suppress
from dataclasses import dataclass
from datetime import datetime, timezone
from functools import lru_cache
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

from benchmark_rust_migration_helper import DELAYS_SECONDS, SOURCES, LoopbackFixtureServer

ROOT = Path(__file__).resolve().parents[2]
IMAGE_FIXTURE = ROOT / "tests/fixtures/sources/windy/raw/tile-z1-x1-y1.png"
DOWNLOAD_DRY_RUN_GOLDEN = (
    ROOT / "tests/fixtures/rust-migration/cli/python-baseline/download-dry-run-network-disabled.json"
)
REQUIRED_REPETITIONS = 30
DEFAULT_SAMPLE_INTERVAL = 0.01
DEFAULT_TIMEOUT = 360.0
CAT_TTY_COLUMNS = 80
CAT_TTY_ROWS = 24
CAT_PREVIEW_COLUMNS = 48
_ANSI_CELL = re.compile(
    r"\x1b\[38;2;(\d+);(\d+);(\d+)m"
    r"(?:\x1b\[48;2;(\d+);(\d+);(\d+)m)?([▀▄])"
)
T003_GAPS: dict[str, str] = {}
T003_MEASUREMENT_LIMITATIONS = {
    "provider_network": "Source-delay replay uses an explicit 127.0.0.1 HTTP fixture and does not measure public-provider RTT or service load.",
    "host_page_cache": "Cold means the first operation in a fresh helper process; the host filesystem/page cache is not flushed.",
    "sampling": "Process-tree RSS and temporary-file peaks are sampled; short-lived descendants or files may fall between samples.",
}

FIXTURE_SCENARIOS = (
    ("list_sources_cold", "list_sources", 0, "cold"),
    ("list_sources_warm", "list_sources", 1, "warm"),
    ("discover_all_cold", "discover_all", 0, "cold"),
    ("discover_all_warm", "discover_all", 1, "warm"),
    ("four_delayed_sources_cold", "four_delayed_sources", 0, "cold"),
    ("four_delayed_sources_warm", "four_delayed_sources", 1, "warm"),
    ("batch_raw_acquisition", "batch_acquisition", None, "batch"),
)


@dataclass(frozen=True)
class Scenario:
    name: str
    title: str
    python_args: tuple[str, ...]
    native_args: tuple[str, ...]
    command: str
    required_options: tuple[str, ...]
    optional: bool = False


SCENARIOS = (
    Scenario(
        "list_sources",
        "list sources",
        ("list", "sources", "--json"),
        ("--json", "list", "sources"),
        "list",
        ("--json",),
    ),
    Scenario(
        "discover_all",
        "offline discover all",
        ("discover", "all", "--json"),
        ("--json", "discover", "all"),
        "discover",
        ("--json",),
    ),
    Scenario(
        "cat_file",
        "local cat --file ANSI first frame on an 80x24 PTY",
        ("cat", "--file", str(IMAGE_FIXTURE), "--renderer", "ansi", "--width", str(CAT_PREVIEW_COLUMNS), "--height", str(CAT_TTY_ROWS)),
        ("cat", "--file", str(IMAGE_FIXTURE), "--renderer", "ansi", "--width", str(CAT_PREVIEW_COLUMNS), "--height", str(CAT_TTY_ROWS)),
        "cat",
        ("--file", "--renderer"),
    ),
    Scenario(
        "download_dry_run",
        "offline download --dry-run",
        ("download", "au", "--latest", "--dry-run", "--json"),
        ("--json", "download", "au", "--latest", "--dry-run"),
        "download",
        ("--dry-run", "--json"),
        optional=True,
    ),
)


def _json_bytes(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False
    ).encode("utf-8")


def _fingerprint(value: Any) -> str:
    return hashlib.sha256(_json_bytes(value)).hexdigest()


def _percentile(values: Sequence[float], percentile: float) -> float | None:
    """Return the linearly interpolated percentile at index ``(n - 1) * p``."""
    if not values:
        return None
    if not 0 < percentile <= 1:
        raise ValueError("percentile must be in (0, 1]")
    ordered = sorted(values)
    index = (len(ordered) - 1) * percentile
    lower = math.floor(index)
    upper = math.ceil(index)
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (index - lower)


def _tree_file_bytes(path: Path) -> int:
    """Sum regular-file sizes beneath path without following symbolic links."""
    total = 0
    if not path.exists():
        return total
    for directory, subdirs, files in os.walk(path, followlinks=False):
        subdirs[:] = [name for name in subdirs if not (Path(directory) / name).is_symlink()]
        for name in files:
            child = Path(directory) / name
            try:
                if not child.is_symlink() and child.is_file():
                    total += child.stat().st_size
            except OSError:
                continue
    return total


def _tree_file_count(path: Path) -> int:
    """Count regular files beneath path without following symbolic links."""
    if not path.exists():
        return 0
    count = 0
    for directory, subdirs, files in os.walk(path, followlinks=False):
        subdirs[:] = [name for name in subdirs if not (Path(directory) / name).is_symlink()]
        count += sum(not (Path(directory) / name).is_symlink() for name in files)
    return count


def _tree_fingerprint(path: Path) -> str:
    digest = hashlib.sha256()
    for child in sorted(
        (item for item in path.rglob("*") if item.is_file() and "__pycache__" not in item.parts),
        key=lambda item: item.relative_to(path).as_posix(),
    ):
        relative = child.relative_to(path).as_posix().encode("utf-8")
        digest.update(len(relative).to_bytes(4, "big"))
        digest.update(relative)
        with child.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
    return digest.hexdigest()


def _macho_architectures(path: Path | None) -> list[str]:
    if path is None:
        return []
    try:
        with path.open("rb") as stream:
            header = stream.read(4096)
    except OSError:
        return []
    labels = {0x01000007: "x86_64", 0x0100000C: "arm64", 7: "i386", 12: "arm"}
    magic = header[:4]
    if magic in {b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe"}:
        cpu_type = struct.unpack_from("<I", header, 4)[0]
        return [labels[cpu_type]] if cpu_type in labels else [f"cpu_type_{cpu_type}"]
    if magic in {b"\xfe\xed\xfa\xcf", b"\xfe\xed\xfa\xce"}:
        cpu_type = struct.unpack_from(">I", header, 4)[0]
        return [labels[cpu_type]] if cpu_type in labels else [f"cpu_type_{cpu_type}"]
    fat_formats = {
        b"\xca\xfe\xba\xbe": (">", 20),
        b"\xbe\xba\xfe\xca": ("<", 20),
        b"\xca\xfe\xba\xbf": (">", 32),
        b"\xbf\xba\xfe\xca": ("<", 32),
    }
    if magic in fat_formats and len(header) >= 8:
        endian, entry_size = fat_formats[magic]
        count = struct.unpack_from(endian + "I", header, 4)[0]
        architectures: set[str] = set()
        for index in range(count):
            offset = 8 + index * entry_size
            if offset + 4 > len(header):
                break
            cpu_type = struct.unpack_from(endian + "I", header, offset)[0]
            architectures.add(labels.get(cpu_type, f"cpu_type_{cpu_type}"))
        return sorted(architectures)
    return []


def _resolve_executable(value: str) -> str | None:
    candidate = Path(value).expanduser()
    if candidate.parent != Path(".") or candidate.is_absolute():
        try:
            # Keep symlinked venv launchers intact: Python determines its
            # environment from the invoked path, while Path.resolve() points
            # at the base interpreter and drops venv site-packages.
            resolved = Path(os.path.abspath(candidate))
        except OSError:
            return None
        return str(resolved) if resolved.is_file() and os.access(resolved, os.X_OK) else None
    return shutil.which(value)


def _default_python() -> str:
    project_python = ROOT / ".venv/bin/python"
    if project_python.is_file() and os.access(project_python, os.X_OK):
        # Preserve the venv path. Resolving a macOS symlink to the base
        # interpreter bypasses pyvenv detection and hides installed project
        # dependencies such as PyYAML when PYTHONNOUSERSITE is enabled.
        return str(project_python)
    return sys.executable


def _native_candidate(explicit: str | None) -> tuple[str | None, str | None]:
    if explicit:
        resolved = _resolve_executable(explicit)
        return resolved, None if resolved else f"native CLI is not executable or was not found: {explicit}"
    configured = os.environ.get("RADIUST_NATIVE_CLI")
    if configured:
        resolved = _resolve_executable(configured)
        return resolved, None if resolved else "RADIUST_NATIVE_CLI is not executable or was not found"
    for candidate in (ROOT / "target/release/radiust", ROOT / "target/debug/radiust"):
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate.resolve()), None
    return None, "native CLI not found; pass --native-cli PATH or set RADIUST_NATIVE_CLI"


def _controlled_environment(
    base: dict[str, str], runtime_root: Path, python_source: Path | None = None
) -> dict[str, str]:
    """Return a clean, deterministic environment with Radiust networking off."""
    env = {key: value for key, value in base.items() if not key.startswith("RADIUST_")}
    for key in tuple(env):
        if key.lower().endswith("_proxy") or key.lower() in {"http_proxy", "https_proxy", "all_proxy", "ftp_proxy"}:
            env.pop(key, None)
    temp_root = runtime_root / "tmp"
    cache_root = runtime_root / "cache"
    output_root = runtime_root / "output"
    for directory in (temp_root, cache_root, output_root):
        directory.mkdir(parents=True, exist_ok=True)
    env.pop("PYTHONHOME", None)
    env.update(
        {
            "RADIUST_RUNTIME__ALLOW_NETWORK": "false",
            "RADIUST_RUNTIME__TEMP_ROOT": str(temp_root),
            "RADIUST_CACHE__DIR": str(cache_root),
            "RADIUST_STORAGE__OUTPUT": str(output_root),
            "TMPDIR": str(temp_root),
            "TMP": str(temp_root),
            "TEMP": str(temp_root),
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONNOUSERSITE": "1",
            "PYTHONHASHSEED": "0",
            "PYTHONUTF8": "1",
            "LC_ALL": "C",
            "LANG": "C",
            "TZ": "UTC",
            "NO_PROXY": "127.0.0.1,localhost,::1",
            "no_proxy": "127.0.0.1,localhost,::1",
        }
    )
    # Force imports to resolve to this checkout, not a stale installed wheel.
    env["PYTHONPATH"] = str((python_source or ROOT / "python").resolve())
    return env


def _probe(argv: Sequence[str], env: dict[str, str], *, timeout: float = 20.0) -> tuple[int | None, str]:
    try:
        completed = subprocess.run(
            list(argv),
            cwd=ROOT,
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return None, type(exc).__name__
    return completed.returncode, completed.stdout + completed.stderr


def _last_diagnostic(output: str) -> str:
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    return lines[-1][:300] if lines else "no diagnostic output"


def _help_capabilities(
    prefix: Sequence[str], env: dict[str, str], *, unavailable_reason: str | None = None
) -> tuple[dict[str, dict[str, Any]], str | None, str | None]:
    if unavailable_reason:
        return (
            {
                scenario.name: {"status": "unavailable", "reason": unavailable_reason}
                for scenario in SCENARIOS
            },
            None,
            None,
        )
    version_code, version_text = _probe([*prefix, "--version"], env)
    version = version_text.strip().splitlines()[0] if version_code == 0 and version_text.strip() else None
    root_code, root_help = _probe([*prefix, "--help"], env)
    if root_code != 0:
        reason = (
            f"CLI help exited {root_code}: {_last_diagnostic(root_help)}"
            if root_code is not None
            else f"CLI could not start: {root_help}"
        )
        return (
            {
                scenario.name: {"status": "unavailable", "reason": reason}
                for scenario in SCENARIOS
            },
            version,
            None,
        )

    root_commands = set(
        re.findall(r"^\s{1,}(list|discover|cat|download)\b", root_help, flags=re.MULTILINE)
    )
    capabilities: dict[str, dict[str, Any]] = {}
    help_by_command: dict[str, str] = {}
    for command in {scenario.command for scenario in SCENARIOS}:
        if command not in root_commands:
            continue
        code, command_help = _probe([*prefix, command, "--help"], env)
        if code == 0:
            help_by_command[command] = command_help
        else:
            help_by_command[command] = f"__HELP_FAILED__:{code}:{_last_diagnostic(command_help)}"

    for scenario in SCENARIOS:
        if scenario.command not in root_commands:
            capabilities[scenario.name] = {
                "status": "unsupported",
                "reason": f"current CLI help does not list the {scenario.command!r} command",
            }
            continue
        command_help = help_by_command.get(scenario.command)
        if command_help is None:
            capabilities[scenario.name] = {
                "status": "unsupported",
                "reason": f"current CLI does not provide help for {scenario.command!r}",
            }
            continue
        if command_help.startswith("__HELP_FAILED__:"):
            capabilities[scenario.name] = {
                "status": "unsupported",
                "reason": f"current CLI help failed: {command_help.split(':', 2)[-1]}",
            }
            continue
        missing = [option for option in scenario.required_options if option not in command_help and option not in root_help]
        if missing:
            capabilities[scenario.name] = {
                "status": "unsupported",
                "reason": "current CLI help does not advertise required option(s): " + ", ".join(missing),
            }
            continue
        if scenario.name == "cat_file" and not IMAGE_FIXTURE.is_file():
            capabilities[scenario.name] = {
                "status": "unavailable",
                "reason": f"local fixture is missing: {IMAGE_FIXTURE}",
            }
            continue
        capabilities[scenario.name] = {"status": "supported", "reason": None}

    return capabilities, version, root_help


def _extract_items(payload: Any) -> list[dict[str, Any]] | None:
    items: Any = payload
    if isinstance(payload, dict):
        items = payload.get("items")
        if items is None and isinstance(payload.get("result"), list):
            items = payload["result"]
    if not isinstance(items, list) or any(not isinstance(item, dict) for item in items):
        return None
    return items


def _normalize_payload(value: Any, temp_roots: Sequence[Path]) -> Any:
    if isinstance(value, dict):
        return {
            key: ("<RUN_ID>" if key == "run_id" and child is not None else _normalize_payload(child, temp_roots))
            for key, child in sorted(value.items())
        }
    if isinstance(value, list):
        return [_normalize_payload(child, temp_roots) for child in value]
    if isinstance(value, str):
        for root in temp_roots:
            value = value.replace(str(root), "<TEMP>")
        return value
    return value


def _source_semantics(payload: Any) -> tuple[dict[str, Any], bool]:
    items = _extract_items(payload)
    if items is None:
        return {"invalid_output": "expected a JSON source list"}, False
    sources: list[dict[str, Any]] = []
    for item in items:
        source_id = item.get("id") or item.get("source")
        products = item.get("products")
        if not isinstance(source_id, str) or not isinstance(products, list):
            return {"invalid_output": "source item lacks id or products"}, False
        product_ids = [
            product if isinstance(product, str) else product.get("id")
            for product in products
            if isinstance(product, (str, dict))
        ]
        sources.append(
            {
                "id": source_id,
                "description": item.get("description"),
                "availability": item.get("availability"),
                "products": sorted(product for product in product_ids if isinstance(product, str)),
            }
        )
    sources.sort(key=lambda source: source["id"])
    valid = bool(sources) and len({source["id"] for source in sources}) == len(sources)
    return {"sources": sources}, valid


def _discovery_semantics(payload: Any) -> tuple[dict[str, Any], bool]:
    items = _extract_items(payload)
    if items is None or not isinstance(payload, dict) or payload.get("command") != "discover":
        return {"invalid_output": "expected a discovery report with items"}, False
    rows: list[dict[str, Any]] = []
    for item in items:
        target = item.get("target") if isinstance(item.get("target"), dict) else {}
        frame = item.get("frame") if isinstance(item.get("frame"), dict) else {}
        error = item.get("error") if isinstance(item.get("error"), dict) else {}
        source = item.get("source", target.get("source"))
        product = item.get("product", target.get("product"))
        station = item.get("station", target.get("station"))
        status = item.get("status")
        if not isinstance(source, str) or not isinstance(status, str):
            return {"invalid_output": "discovery item lacks source or status"}, False
        rows.append(
            {
                "source": source,
                "product": product,
                "station": station,
                "status": status,
                "error_code": error.get("code"),
                "valid_time": item.get("valid_time") or frame.get("valid_time"),
                "logical_id": frame.get("logical_id"),
            }
        )
    rows.sort(
        key=lambda row: (
            row["source"], row["product"] or "", row["station"] or "", row["status"],
            row["error_code"] or "", row["valid_time"] or "", row["logical_id"] or "",
        )
    )
    valid_statuses = {
        "success", "no_data", "stale", "missing_credentials", "retired",
        "network_restricted", "upstream_failed", "ambiguous", "timeout",
        "cancelled", "not_started",
    }
    valid = bool(rows) and all(row["status"] in valid_statuses for row in rows)
    return {"items": rows}, valid


def _download_semantics(payload: Any) -> tuple[dict[str, Any], bool]:
    if not isinstance(payload, dict):
        return {"invalid_output": "expected a JSON download report"}, False
    items = _extract_items(payload)
    if items is None:
        items = []
    error = payload.get("error")
    try:
        legacy_download = json.loads(DOWNLOAD_DRY_RUN_GOLDEN.read_text(encoding="utf-8"))
        legacy_payload = legacy_download.get("stdout")
    except (OSError, json.JSONDecodeError):
        legacy_payload = None
    # The frozen Python CLI contract for an offline dry-run is the generic
    # validation error envelope. Preserve that baseline exactly until native
    # download planning is deliberately migrated with a new contract.
    if (
        legacy_payload == payload
        and payload.get("schema_version") == 1
        and payload.get("command") is None
        and payload.get("items") == []
        and isinstance(error, dict)
    ):
        return {
            "items": [{
                "source": "au",
                "product": "composite",
                "station": None,
                "status": "error",
                "error_code": "error",
                "valid_time": None,
            }],
            "command": "download",
        }, True
    rows: list[dict[str, Any]] = []
    for item in items:
        error = item.get("error") if isinstance(item.get("error"), dict) else {}
        rows.append(
            {
                "source": item.get("source", "au"),
                "product": item.get("product", "composite"),
                "station": item.get("station"),
                "status": item.get("status"),
                "error_code": error.get("code"),
                "valid_time": item.get("valid_time"),
            }
        )
    top_error = payload.get("error")
    if not rows and isinstance(top_error, dict):
        rows.append(
            {
                "source": "au",
                "product": "composite",
                "station": None,
                "status": top_error.get("code"),
                "error_code": top_error.get("code"),
                "valid_time": None,
            }
        )
    rows.sort(key=lambda row: (row["source"] or "", row["product"] or "", row["station"] or "", row["status"] or ""))
    allowed_statuses = {
        "planned", "network_restricted", "missing_credentials", "retired",
        "no_data", "stale", "upstream_failed", "ambiguous", "timeout",
    }
    recognized = bool(rows) and all(row["status"] in allowed_statuses for row in rows)
    return {"items": rows, "command": "download"}, recognized and payload.get("schema_version") == 1


def _ansi_frame_pixels(stdout: str) -> tuple[tuple[tuple[tuple[int, ...], ...], ...], ...] | None:
    """Extract the fixed 48x24 ANSI image grid, ignoring legacy text headers."""
    rows: list[tuple[tuple[tuple[int, ...], ...], ...]] = []
    for line in stdout.replace("\r\n", "\n").splitlines():
        cells: list[tuple[tuple[int, ...], ...]] = []
        for match in _ANSI_CELL.finditer(line):
            foreground = tuple(int(match.group(index)) for index in (1, 2, 3))
            background = (
                tuple(int(match.group(index)) for index in (4, 5, 6))
                if match.group(4) is not None
                else ()
            )
            cells.append((foreground, background, (ord(match.group(7)),)))
        if not cells:
            continue
        if len(cells) != CAT_PREVIEW_COLUMNS:
            return None
        rows.append(tuple(cells))
    if len(rows) != CAT_TTY_ROWS:
        return None
    return tuple(rows)


@lru_cache(maxsize=1)
def _expected_ansi_frame_pixels() -> tuple[tuple[tuple[tuple[int, ...], ...], ...], ...]:
    from PIL import Image

    with Image.open(IMAGE_FIXTURE) as image:
        rgba = image.convert("RGBA")
        source_width, source_height = rgba.size
        expected: list[tuple[tuple[tuple[int, ...], ...], ...]] = []
        for row in range(CAT_TTY_ROWS):
            cells: list[tuple[tuple[int, ...], ...]] = []
            for column in range(CAT_PREVIEW_COLUMNS):
                colors = []
                for source_row in (row * 2, row * 2 + 1):
                    x = column * source_width // CAT_PREVIEW_COLUMNS
                    y = source_row * source_height // (CAT_TTY_ROWS * 2)
                    red, green, blue, alpha = rgba.getpixel((x, y))
                    if alpha != 255:
                        raise ValueError("the fixed cat benchmark image must be fully opaque")
                    colors.append((red, green, blue))
                cells.append((colors[0], colors[1], (ord("▀"),)))
            expected.append(tuple(cells))
        return tuple(expected)


def _cat_semantics(stdout: str, returncode: int | None, timed_out: bool) -> tuple[dict[str, Any], bool]:
    if not IMAGE_FIXTURE.is_file():
        return {"invalid_output": "local fixture is missing"}, False
    image_data = IMAGE_FIXTURE.read_bytes()
    expected_hash = hashlib.sha256(image_data).hexdigest()
    pixels = _ansi_frame_pixels(stdout)
    try:
        expected_pixels = _expected_ansi_frame_pixels()
    except (ImportError, OSError, ValueError) as exc:
        return {"invalid_output": f"could not validate source pixels: {type(exc).__name__}"}, False
    correct = returncode == 0 and not timed_out and pixels == expected_pixels
    return (
        {
            "source_path": IMAGE_FIXTURE.name,
            "input_sha256": expected_hash,
            "renderer": "ansi",
            "pty_size": [CAT_TTY_COLUMNS, CAT_TTY_ROWS],
            "frame_size": [CAT_PREVIEW_COLUMNS, CAT_TTY_ROWS],
            "frame_pixel_sha256": _fingerprint(pixels) if pixels is not None else None,
        },
        correct,
    )


def _semantics(scenario: Scenario, stdout: str, returncode: int | None, timed_out: bool) -> tuple[Any, bool]:
    if scenario.name == "cat_file":
        return _cat_semantics(stdout, returncode, timed_out)
    try:
        payload = json.loads(stdout)
    except (json.JSONDecodeError, TypeError):
        return {"invalid_output": "stdout is not valid JSON"}, False
    if scenario.name == "list_sources":
        result, valid = _source_semantics(payload)
        return result, valid and returncode == 0 and not timed_out
    if scenario.name == "discover_all":
        result, valid = _discovery_semantics(payload)
        allowed_offline_codes = {0, 3, 4, 5}
        return result, valid and returncode in allowed_offline_codes and not timed_out
    result, valid = _download_semantics(payload)
    allowed_offline_codes = {0, 2, 3, 4, 5}
    return result, valid and returncode in allowed_offline_codes and not timed_out


def _ru_maxrss_bytes(value: int) -> int:
    # Darwin reports bytes; Linux and the common BSD-compatible runtimes report KiB.
    return int(value if sys.platform == "darwin" else value * 1024)


def _parse_ps_snapshot(output: str, root_pid: int) -> int | None:
    """Sum RSS for the root process and descendants in a ``ps -axo`` snapshot."""
    parents: dict[int, int] = {}
    rss_kib: dict[int, int] = {}
    for line in output.splitlines():
        fields = line.split()
        if len(fields) != 3:
            continue
        try:
            pid, ppid, rss = (int(value) for value in fields)
        except ValueError:
            continue
        parents[pid] = ppid
        rss_kib[pid] = rss
    if root_pid not in parents:
        return None
    members = {root_pid}
    while True:
        expanded = members | {pid for pid, ppid in parents.items() if ppid in members}
        if expanded == members:
            break
        members = expanded
    return sum(rss_kib.get(pid, 0) for pid in members) * 1024


def _sample_process_tree_rss(
    root_pid: int,
    stop: threading.Event,
    peak: list[int | None],
    status: list[str],
    interval: float,
) -> None:
    """Sample independently so process enumeration does not delay wait polling."""
    while not stop.is_set():
        try:
            snapshot = subprocess.run(
                ["ps", "-axo", "pid=,ppid=,rss="],
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                encoding="ascii",
                errors="replace",
                timeout=max(1.0, interval * 10),
                check=False,
            )
            if snapshot.returncode == 0:
                value = _parse_ps_snapshot(snapshot.stdout, root_pid)
                if value is not None and (peak[0] is None or value > peak[0]):
                    peak[0] = value
                    status[0] = "sampled"
            else:
                status[0] = "ps_failed"
        except OSError as exc:
            status[0] = type(exc).__name__
        except subprocess.TimeoutExpired:
            status[0] = "ps_timeout"
        stop.wait(interval)


def _kill_process_group(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError, OSError):
        with suppress(OSError):
            process.kill()


def _run_sample(
    argv: Sequence[str],
    env_base: dict[str, str],
    scenario: Scenario,
    *,
    timeout: float,
    sample_interval: float,
    python_source: Path,
) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="radiust-bench-") as temporary:
        root = Path(temporary)
        runtime_root = root / "runtime"
        runtime_root.mkdir()
        capture_root = root / "capture"
        capture_root.mkdir()
        # The child needs both handles open until wait completes; read and
        # close them immediately after exit below.
        stdout_file = tempfile.TemporaryFile(mode="w+b", dir=capture_root)  # noqa: SIM115
        stderr_file = tempfile.TemporaryFile(mode="w+b", dir=capture_root)  # noqa: SIM115
        env = _controlled_environment(env_base, runtime_root, python_source)
        tty_master_fd: int | None = None
        tty_slave_fd: int | None = None
        tty_output = bytearray()
        first_frame_observed_at: float | None = None
        stdout_target: Any = stdout_file
        if scenario.name == "cat_file":
            tty_master_fd, tty_slave_fd = pty.openpty()
            fcntl.ioctl(
                tty_slave_fd,
                termios.TIOCSWINSZ,
                struct.pack("HHHH", CAT_TTY_ROWS, CAT_TTY_COLUMNS, 0, 0),
            )
            os.set_blocking(tty_master_fd, False)
            stdout_target = tty_slave_fd
            env["TERM"] = "xterm-256color"
            env.pop("NO_COLOR", None)
            if env.get("COLORTERM") == "0":
                env.pop("COLORTERM")

        def drain_tty_output() -> None:
            nonlocal first_frame_observed_at
            if tty_master_fd is None:
                return
            while True:
                try:
                    chunk = os.read(tty_master_fd, 65536)
                except BlockingIOError:
                    break
                except OSError as exc:
                    if exc.errno in {errno.EIO, errno.EBADF}:
                        break
                    raise
                if not chunk:
                    break
                tty_output.extend(chunk)
                if first_frame_observed_at is None:
                    terminal_text = tty_output.decode("utf-8", errors="replace")
                    if _ansi_frame_pixels(terminal_text) is not None:
                        first_frame_observed_at = time.perf_counter()

        started = time.perf_counter()
        process: subprocess.Popen[bytes] | None = None
        peak_rss: int | None = None
        peak_tree_rss: list[int | None] = [None]
        tree_rss_status = ["target_not_observed"]
        rss_sampler_stop = threading.Event()
        max_temp_bytes = _tree_file_bytes(runtime_root)
        timed_out = False
        launch_error: str | None = None
        try:
            process = subprocess.Popen(
                list(argv),
                cwd=ROOT,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=stdout_target,
                stderr=stderr_file,
                start_new_session=True,
                shell=False,
            )
        except OSError as exc:
            launch_error = type(exc).__name__
        finally:
            if tty_slave_fd is not None:
                os.close(tty_slave_fd)
                tty_slave_fd = None

        returncode: int | None = None
        if process is not None:
            rss_sampler = threading.Thread(
                target=_sample_process_tree_rss,
                args=(
                    process.pid,
                    rss_sampler_stop,
                    peak_tree_rss,
                    tree_rss_status,
                    max(0.05, sample_interval),
                ),
                name="radiust-benchmark-rss",
                daemon=True,
            )
            rss_sampler.start()
            deadline = started + timeout
            if hasattr(os, "wait4"):
                while True:
                    drain_tty_output()
                    max_temp_bytes = max(max_temp_bytes, _tree_file_bytes(runtime_root))
                    try:
                        waited_pid, status, usage = os.wait4(process.pid, os.WNOHANG)
                    except InterruptedError:
                        continue
                    except ChildProcessError:
                        waited_pid, status, usage = process.pid, None, None
                    if waited_pid == process.pid:
                        returncode = os.waitstatus_to_exitcode(status) if status is not None else None
                        process.returncode = returncode
                        if usage is not None:
                            peak_rss = _ru_maxrss_bytes(usage.ru_maxrss)
                        break
                    if not timed_out and time.monotonic() >= deadline:
                        timed_out = True
                        _kill_process_group(process)
                    if tty_master_fd is not None:
                        select.select([tty_master_fd], [], [], sample_interval)
                    else:
                        time.sleep(sample_interval)
            else:
                while process.poll() is None:
                    drain_tty_output()
                    max_temp_bytes = max(max_temp_bytes, _tree_file_bytes(runtime_root))
                    if time.monotonic() >= deadline:
                        timed_out = True
                        _kill_process_group(process)
                        break
                    if tty_master_fd is not None:
                        select.select([tty_master_fd], [], [], sample_interval)
                    else:
                        time.sleep(sample_interval)
                returncode = process.wait()
            drain_tty_output()
            if peak_tree_rss[0] is None and peak_rss is not None:
                # Very short native commands can exit before the independent
                # `ps` sampler observes them. For the current single-process
                # CLI scenarios, retain wait4's exact child peak as a disclosed
                # fallback instead of writing a false zero/missing RSS value.
                peak_tree_rss[0] = peak_rss
                tree_rss_status[0] = "direct_child_wait4_fallback"
            max_temp_bytes = max(max_temp_bytes, _tree_file_bytes(runtime_root))
            rss_sampler_stop.set()
            rss_sampler.join(timeout=2)

        ended = time.perf_counter()
        if tty_master_fd is not None:
            os.close(tty_master_fd)
            tty_master_fd = None
            stdout_bytes = bytes(tty_output)
        else:
            stdout_file.seek(0)
            stdout_bytes = stdout_file.read()
        stderr_file.seek(0)
        stderr_bytes = stderr_file.read()
        stdout_file.close()
        stderr_file.close()
        stdout = stdout_bytes.decode("utf-8", errors="replace")
        stderr = stderr_bytes.decode("utf-8", errors="replace")

        semantic_value, correctness_ok = _semantics(scenario, stdout, returncode, timed_out)
        normalized_stdout = stdout.rstrip("\r\n")
        try:
            parsed_stdout = json.loads(stdout)
        except (json.JSONDecodeError, TypeError):
            parsed_stdout = None
        if parsed_stdout is not None:
            normalized_output: Any = _normalize_payload(parsed_stdout, (runtime_root,))
        else:
            normalized_output = normalized_stdout.replace(str(runtime_root), "<TEMP>")
        normalized_stderr = stderr.rstrip("\r\n").replace(str(runtime_root), "<TEMP>")
        output_fingerprint = _fingerprint(
            {"exit_code": returncode, "stdout": normalized_output, "stderr": normalized_stderr}
        )
        sample: dict[str, Any] = {
            "elapsed_seconds": round(ended - started, 9),
            "first_frame_output_seconds": (
                round(first_frame_observed_at - started, 9)
                if first_frame_observed_at is not None
                else None
            ),
            "exit_code": returncode,
            "timed_out": timed_out,
            "direct_child_peak_rss_bytes": peak_rss,
            "rss_measurement": (
                "wait4 ru_maxrss for direct child; bytes on macOS"
                if hasattr(os, "wait4")
                else "unavailable: os.wait4 is not supported on this platform"
            ),
            "sampled_process_tree_peak_rss_bytes": peak_tree_rss[0],
            "process_tree_rss_measurement": {
                "method": "sampled ps RSS sum for the CLI process and observed descendants",
                "interval_seconds": max(0.05, sample_interval),
                "status": tree_rss_status[0],
                "may_miss_short_lived_descendants": True,
            },
            "sampled_temporary_bytes": max_temp_bytes,
            "temporary_bytes_measurement": {
                "method": "maximum observed regular-file bytes beneath isolated runtime root",
                "interval_seconds": sample_interval,
            },
            "stdout_sha256": hashlib.sha256(stdout_bytes).hexdigest(),
            "stderr_sha256": hashlib.sha256(stderr_bytes).hexdigest(),
            "output_fingerprint": output_fingerprint,
            "semantic_fingerprint": _fingerprint(semantic_value),
            "correctness_ok": correctness_ok and launch_error is None,
            "correctness": semantic_value,
        }
        if launch_error is not None:
            sample["launch_error"] = launch_error
        return sample


def _fixture_semantics(scenario: str, value: Any) -> tuple[Any, bool]:
    if not isinstance(value, dict):
        return {"invalid_output": "fixture helper semantic payload is not an object"}, False
    if scenario.startswith("list_sources_"):
        rows = value.get("sources")
        if not isinstance(rows, list):
            return {"invalid_output": "fixture source list is missing"}, False
        return _source_semantics({"items": rows})
    if scenario.startswith("discover_all_"):
        payload = {"command": "discover", "items": value.get("items")}
        return _discovery_semantics(payload)
    if scenario.startswith("four_delayed_sources_"):
        rows = value.get("items")
        if not isinstance(rows, list) or any(not isinstance(item, dict) for item in rows):
            return {"invalid_output": "delayed-source discovery items are missing"}, False
        normalized = [
            {
                "source": item.get("source"),
                "product": item.get("product"),
                "station": item.get("station"),
                "status": item.get("status"),
                "error_code": item.get("error_code"),
                "valid_time": item.get("valid_time"),
                "logical_id": item.get("logical_id"),
            }
            for item in rows
        ]
        normalized.sort(key=lambda row: (row["source"] or "", row["product"] or ""))
        valid = (
            len(normalized) == len(SOURCES)
            and {item["source"] for item in normalized} == set(SOURCES)
            and all(item["status"] == "success" and item["logical_id"] for item in normalized)
        )
        return {"items": normalized}, valid
    rows = value.get("items")
    if not isinstance(rows, list) or any(not isinstance(item, dict) for item in rows):
        return {"invalid_output": "batch acquisition items are missing"}, False
    normalized_batch: list[dict[str, Any]] = []
    for item in rows:
        artifacts = item.get("artifacts")
        if not isinstance(artifacts, list):
            return {"invalid_output": "batch result has no artifact list"}, False
        normalized_artifacts = []
        for artifact in artifacts:
            if not isinstance(artifact, dict):
                return {"invalid_output": "batch artifact receipt is invalid"}, False
            normalized_artifacts.append(
                {
                    "name": artifact.get("name"),
                    "size_bytes": artifact.get("size_bytes"),
                    "sha256": artifact.get("sha256"),
                }
            )
        normalized_batch.append(
            {
                "source": item.get("source"),
                "logical_id": item.get("logical_id"),
                "status": item.get("status"),
                "artifacts": normalized_artifacts,
            }
        )
    normalized_batch.sort(key=lambda row: row["source"] or "")
    valid = (
        len(normalized_batch) == len(SOURCES)
        and {item["source"] for item in normalized_batch} == set(SOURCES)
        and all(item["status"] == "success" and item["logical_id"] for item in normalized_batch)
        and all(len(item["artifacts"]) == 1 for item in normalized_batch)
    )
    return {"items": normalized_batch}, valid


def _run_fixture_helper(
    argv: Sequence[str],
    env_base: dict[str, str],
    *,
    python_source: Path,
    timeout: float,
    sample_interval: float,
) -> dict[str, Any]:
    """Run one loopback-only cold/warm replay sequence in an isolated process."""
    with tempfile.TemporaryDirectory(prefix="radiust-fixture-bench-") as temporary:
        root = Path(temporary)
        runtime_root = root / "runtime"
        runtime_root.mkdir()
        capture_root = root / "capture"
        capture_root.mkdir()
        temp_root = runtime_root / "tmp"
        temp_root.mkdir()
        env = _controlled_environment(env_base, runtime_root, python_source)
        env.pop("RADIUST_TEST_ALLOW_LIVE", None)
        env["RADIUST_RUNTIME__ALLOW_NETWORK"] = "true"
        try:
            server_value = argv[list(argv).index("--server") + 1]
            fixture_server = urlsplit(server_value)
        except (ValueError, IndexError) as exc:
            raise ValueError("fixture helper must receive one explicit --server URL") from exc
        if (
            fixture_server.scheme != "http"
            or fixture_server.hostname != "127.0.0.1"
            or fixture_server.port is None
        ):
            raise ValueError("fixture helper network access is restricted to 127.0.0.1")
        command = [
            str(runtime_root) if argument == "{runtime_root}" else argument
            for argument in argv
        ]
        stdout_file = tempfile.TemporaryFile(mode="w+b", dir=capture_root)  # noqa: SIM115
        stderr_file = tempfile.TemporaryFile(mode="w+b", dir=capture_root)  # noqa: SIM115
        started = time.perf_counter()
        process: subprocess.Popen[bytes] | None = None
        launch_error: str | None = None
        timed_out = False
        direct_peak: int | None = None
        tree_peak: list[int | None] = [None]
        tree_status = ["target_not_observed"]
        stop = threading.Event()
        max_temp_bytes = _tree_file_bytes(temp_root)
        try:
            process = subprocess.Popen(
                command,
                cwd=ROOT,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=stdout_file,
                stderr=stderr_file,
                start_new_session=True,
                shell=False,
            )
        except OSError as exc:
            launch_error = type(exc).__name__

        returncode: int | None = None
        sampler: threading.Thread | None = None
        if process is not None:
            sampler = threading.Thread(
                target=_sample_process_tree_rss,
                args=(process.pid, stop, tree_peak, tree_status, max(0.05, sample_interval)),
                name="radiust-fixture-benchmark-rss",
                daemon=True,
            )
            sampler.start()
            deadline = started + timeout
            if hasattr(os, "wait4"):
                while True:
                    max_temp_bytes = max(max_temp_bytes, _tree_file_bytes(temp_root))
                    try:
                        waited_pid, status, usage = os.wait4(process.pid, os.WNOHANG)
                    except InterruptedError:
                        continue
                    except ChildProcessError:
                        waited_pid, status, usage = process.pid, None, None
                    if waited_pid == process.pid:
                        returncode = os.waitstatus_to_exitcode(status) if status is not None else None
                        process.returncode = returncode
                        if usage is not None:
                            direct_peak = _ru_maxrss_bytes(usage.ru_maxrss)
                        break
                    if not timed_out and time.monotonic() >= deadline:
                        timed_out = True
                        _kill_process_group(process)
                    time.sleep(sample_interval)
            else:
                while process.poll() is None:
                    max_temp_bytes = max(max_temp_bytes, _tree_file_bytes(temp_root))
                    if time.monotonic() >= deadline:
                        timed_out = True
                        _kill_process_group(process)
                        break
                    time.sleep(sample_interval)
                returncode = process.wait()
            max_temp_bytes = max(max_temp_bytes, _tree_file_bytes(temp_root))
            stop.set()
            sampler.join(timeout=2)
            if tree_peak[0] is None and direct_peak is not None:
                tree_peak[0] = direct_peak
                tree_status[0] = "direct_child_wait4_fallback"

        ended = time.perf_counter()
        stdout_file.seek(0)
        stderr_file.seek(0)
        stdout_bytes = stdout_file.read()
        stderr_bytes = stderr_file.read()
        stdout_file.close()
        stderr_file.close()
        stdout = stdout_bytes.decode("utf-8", errors="replace")
        stderr = stderr_bytes.decode("utf-8", errors="replace")
        helper_result: dict[str, Any] | None = None
        parse_error: str | None = None
        for line in reversed(stdout.splitlines()):
            try:
                candidate = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(candidate, dict) and isinstance(candidate.get("operations"), dict):
                helper_result = candidate
                break
        if helper_result is None:
            parse_error = "helper did not emit a JSON result with operations"
        temporary_file_count = _tree_file_count(temp_root)
        sample = {
            "process_wall_elapsed_seconds": round(ended - started, 9),
            "exit_code": returncode,
            "timed_out": timed_out,
            "launch_error": launch_error,
            "parse_error": parse_error,
            "direct_child_peak_rss_bytes": direct_peak,
            "sampled_process_tree_peak_rss_bytes": tree_peak[0],
            "process_tree_rss_status": tree_status[0],
            "sampled_temporary_peak_bytes": max_temp_bytes,
            "temporary_file_count_after_exit": temporary_file_count,
            "stdout_sha256": hashlib.sha256(stdout_bytes).hexdigest(),
            "stderr_sha256": hashlib.sha256(stderr_bytes).hexdigest(),
            "stderr_tail": stderr[-1200:],
            "helper_result": helper_result,
            "process_ok": returncode == 0 and not timed_out and launch_error is None and helper_result is not None,
        }
        return sample


def _expected_fixture_requests(runtime: str) -> dict[str, int]:
    return {
        f"{runtime}.cold.discover.{source}": 1 for source in SOURCES
    } | {
        f"{runtime}.warm.discover.{source}": 1 for source in SOURCES
    } | {
        f"{runtime}.batch.artifact.{source}": 1 for source in SOURCES
    }


def _fixture_request_counts(server_counts: dict[str, Any], runtime: str, scenario: str) -> dict[str, Any]:
    all_counts = server_counts.get("by_runtime_phase_kind_source", {})
    expected_all = {
        key: count
        for selected_runtime in ("python", "native")
        for key, count in _expected_fixture_requests(selected_runtime).items()
    }
    exact_sequence_ok = all_counts == expected_all
    if scenario.startswith("four_delayed_sources_cold"):
        phase, kind = "cold", "discover"
    elif scenario.startswith("four_delayed_sources_warm"):
        phase, kind = "warm", "discover"
    elif scenario == "batch_raw_acquisition":
        phase, kind = "batch", "artifact"
    else:
        phase, kind = "", ""
    selected = {
        f"{runtime}.{phase}.{kind}.{source}": all_counts.get(f"{runtime}.{phase}.{kind}.{source}", 0)
        for source in SOURCES
    } if phase else {}
    expected_count = len(SOURCES) if phase else 0
    active = server_counts.get("peak_concurrent_by_runtime_phase_kind", {})
    return {
        "by_source": selected,
        "total": sum(selected.values()),
        "expected_total": expected_count,
        "operation_count_ok": sum(selected.values()) == expected_count,
        "full_helper_sequence_count_ok": exact_sequence_ok,
        "full_helper_sequence_expected": expected_all,
        "full_helper_sequence_observed": all_counts,
        "peak_concurrent_for_runtime_phase_kind": (
            active.get(f"{runtime}.{phase}.{kind}") if phase else None
        ),
    }


def _build_rust_fixture_helper() -> dict[str, Any]:
    """Build the benchmark-only Engine helper with Cargo strictly offline."""
    cargo = shutil.which("cargo")
    rustc = shutil.which("rustc")
    if not cargo or not rustc:
        return {
            "status": "unavailable",
            "reason": "cargo or rustc executable was not found",
            "binary": None,
        }
    project = Path(tempfile.mkdtemp(prefix="radiust-rust-fixture-helper-"))
    source_path = ROOT / "scripts/validation/benchmark_rust_migration_rust_helper.rs"
    (project / "src").mkdir()
    (project / "src/main.rs").write_bytes(source_path.read_bytes())
    core_path = json.dumps(str((ROOT / "crates/radiust-core").resolve()))
    manifest = f'''[package]
name = "radiust-migration-bench-helper"
version = "0.1.0"
edition = "2024"
rust-version = "1.92"

[workspace]

[dependencies]
radiust-core = {{ path = {core_path} }}
futures-util = "0.3"
serde_json = "1.0"
tokio = {{ version = "1.48", features = ["rt-multi-thread", "time"] }}
'''
    (project / "Cargo.toml").write_text(manifest, encoding="utf-8")
    with tempfile.TemporaryDirectory(prefix="radiust-cargo-build-env-") as temporary:
        env = _controlled_environment(os.environ.copy(), Path(temporary) / "runtime")
        env["CARGO_NET_OFFLINE"] = "true"
        env["CARGO_TARGET_DIR"] = str(ROOT / "target")
        command = [cargo, "build", "--release", "--offline", "--manifest-path", str(project / "Cargo.toml")]
        started = time.perf_counter()
        try:
            completed = subprocess.run(
                command,
                cwd=ROOT,
                env=env,
                stdin=subprocess.DEVNULL,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=1800,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            return {
                "status": "failed",
                "reason": type(exc).__name__,
                "command": command,
                "duration_seconds": round(time.perf_counter() - started, 6),
                "binary": None,
                "project_path": str(project),
                "cargo_net_offline": True,
            }
    binary = ROOT / "target/release/radiust-migration-bench-helper"
    status = "complete" if completed.returncode == 0 and binary.is_file() else "failed"
    return {
        "status": status,
        "return_code": completed.returncode,
        "command": command,
        "duration_seconds": round(time.perf_counter() - started, 6),
        "stdout_tail": completed.stdout[-3000:],
        "stderr_tail": completed.stderr[-6000:],
        "binary": str(binary) if binary.is_file() else None,
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest() if binary.is_file() else None,
        "source_sha256": hashlib.sha256(source_path.read_bytes()).hexdigest(),
        "project_path": str(project),
        "cargo_net_offline": True,
    }


def _fixture_operation_payload(helper: dict[str, Any], scenario_name: str) -> dict[str, Any] | None:
    scenario = next((item for item in FIXTURE_SCENARIOS if item[0] == scenario_name), None)
    operations = helper.get("operations")
    if scenario is None or not isinstance(operations, dict):
        return None
    _name, operation_name, operation_index, _phase = scenario
    value = operations.get(operation_name)
    if operation_index is None:
        return value if isinstance(value, dict) else None
    if not isinstance(value, list) or operation_index >= len(value):
        return None
    operation = value[operation_index]
    return operation if isinstance(operation, dict) else None


def _fixture_runtime_summary(samples: list[dict[str, Any]], runtime: str) -> dict[str, Any]:
    rows = [sample[runtime] for sample in samples]
    elapsed = [float(row["elapsed_seconds"]) for row in rows if row.get("elapsed_seconds") is not None]
    rss = [
        int(row["process_tree_peak_rss_bytes"])
        for row in rows
        if row.get("process_tree_peak_rss_bytes") is not None
    ]
    temporary = [
        int(row["temporary_peak_bytes"])
        for row in rows
        if row.get("temporary_peak_bytes") is not None
    ]
    fingerprints = [row["semantic_fingerprint"] for row in rows if row.get("semantic_fingerprint")]
    return {
        "sample_count": len(rows),
        "p50_elapsed_seconds": _percentile(elapsed, 0.50),
        "p95_elapsed_seconds": _percentile(elapsed, 0.95),
        "max_process_tree_peak_rss_bytes": max(rss) if rss else None,
        "max_temporary_peak_bytes": max(temporary) if temporary else None,
        "all_temporary_files_removed": bool(rows)
        and all(row.get("temporary_file_count_after_exit") == 0 for row in rows),
        "semantic_fingerprint_stable": len(set(fingerprints)) <= 1,
        "stable_semantic_fingerprint": fingerprints[0]
        if fingerprints and len(set(fingerprints)) == 1
        else None,
        "all_samples_correct": bool(rows) and all(row.get("correct") is True for row in rows),
    }


def _run_loopback_fixture_matrix(
    args: argparse.Namespace,
    python_executable: str | None,
    python_source: Path,
) -> dict[str, Any]:
    """Compare old serial discovery and current Rust workers on local HTTP replay."""
    if python_executable is None:
        return {"complete": False, "reason": "Python executable is unavailable", "scenarios": {}}
    if not (python_source / "radiust" / "sources" / "base.py").is_file():
        return {
            "complete": False,
            "reason": "captured pre-migration Python source tree is unavailable",
            "python_source": str(python_source),
            "scenarios": {},
        }
    helper_build = _build_rust_fixture_helper()
    if helper_build.get("status") != "complete" or not helper_build.get("binary"):
        return {
            "complete": False,
            "reason": "Rust loopback benchmark helper failed to build",
            "helper_build": helper_build,
            "scenarios": {},
        }

    scenario_samples: dict[str, list[dict[str, Any]]] = {
        name: [] for name, _operation, _index, _phase in FIXTURE_SCENARIOS
    }
    helper_samples: dict[str, list[dict[str, Any]]] = {"python": [], "native": []}
    concurrency_peaks: dict[str, int] = {}
    server = LoopbackFixtureServer(IMAGE_FIXTURE.read_bytes())
    server.start()
    try:
        for repetition in range(args.repetitions):
            token = f"migration-bench-{os.getpid()}-{repetition + 1:03d}"
            python_argv = [
                python_executable,
                str(ROOT / "scripts/validation/benchmark_rust_migration_helper.py"),
                "--runtime",
                "python",
                "--server",
                server.base_url,
                "--token",
                token,
                "--runtime-root",
                "{runtime_root}",
            ]
            native_argv = [
                str(helper_build["binary"]),
                "--server",
                server.base_url,
                "--token",
                token,
                "--runtime-root",
                "{runtime_root}",
            ]
            order = (
                (("python", python_argv), ("native", native_argv))
                if repetition % 2 == 0
                else (("native", native_argv), ("python", python_argv))
            )
            current: dict[str, dict[str, Any]] = {}
            for runtime, command in order:
                current[runtime] = _run_fixture_helper(
                    command,
                    os.environ.copy(),
                    python_source=python_source,
                    timeout=args.timeout_seconds,
                    sample_interval=args.sample_interval_seconds,
                )
                helper_samples[runtime].append(current[runtime])

            counts = server.counts(token)
            count_results = {
                runtime: {
                    label: _fixture_request_counts(counts, runtime, scenario_name)
                    for label, scenario_name in (
                        ("cold_discovery", "four_delayed_sources_cold"),
                        ("warm_discovery", "four_delayed_sources_warm"),
                        ("batch_acquisition", "batch_raw_acquisition"),
                    )
                }
                for runtime in ("python", "native")
            }
            for phase in ("cold", "warm"):
                for runtime in ("python", "native"):
                    key = f"{runtime}.{phase}.discover"
                    concurrency_peaks[key] = max(
                        concurrency_peaks.get(key, 0),
                        int(counts.get("peak_concurrent_by_runtime_phase_kind", {}).get(key, 0)),
                    )
            for scenario_name, _operation, _index, _phase in FIXTURE_SCENARIOS:
                runtime_rows: dict[str, Any] = {}
                for runtime in ("python", "native"):
                    helper_sample = current[runtime]
                    helper_result = helper_sample.get("helper_result")
                    operation = (
                        _fixture_operation_payload(helper_result, scenario_name)
                        if isinstance(helper_result, dict)
                        else None
                    )
                    semantic, semantic_ok = _fixture_semantics(
                        scenario_name,
                        operation.get("semantic") if operation else None,
                    )
                    fingerprint = _fingerprint(semantic) if semantic_ok else None
                    request_result = _fixture_request_counts(counts, runtime, scenario_name)
                    process_ok = helper_sample.get("process_ok") is True
                    operation_count_ok = request_result["operation_count_ok"]
                    correct = (
                        process_ok
                        and semantic_ok
                        and request_result["full_helper_sequence_count_ok"]
                        and operation_count_ok
                        and helper_sample.get("temporary_file_count_after_exit") == 0
                    )
                    runtime_rows[runtime] = {
                        "elapsed_seconds": operation.get("elapsed_seconds") if operation else None,
                        "semantic_fingerprint": fingerprint,
                        "correct": correct,
                        "process_wall_elapsed_seconds": helper_sample.get("process_wall_elapsed_seconds"),
                        "process_tree_peak_rss_bytes": helper_sample.get(
                            "sampled_process_tree_peak_rss_bytes"
                        ),
                        "direct_child_peak_rss_bytes": helper_sample.get(
                            "direct_child_peak_rss_bytes"
                        ),
                        "temporary_peak_bytes": helper_sample.get("sampled_temporary_peak_bytes"),
                        "temporary_file_count_after_exit": helper_sample.get(
                            "temporary_file_count_after_exit"
                        ),
                        "process_tree_rss_status": helper_sample.get("process_tree_rss_status"),
                        "request_count_result": request_result,
                    }
                matched = (
                    runtime_rows["python"]["semantic_fingerprint"] is not None
                    and runtime_rows["python"]["semantic_fingerprint"]
                    == runtime_rows["native"]["semantic_fingerprint"]
                )
                scenario_samples[scenario_name].append(
                    {
                        "repetition": repetition + 1,
                        "run_order": [runtime for runtime, _command in order],
                        "python": runtime_rows["python"],
                        "native": runtime_rows["native"],
                        "semantic_fingerprint_match": matched,
                        "both_correct": runtime_rows["python"]["correct"]
                        and runtime_rows["native"]["correct"],
                        "request_counts": count_results,
                    }
                )
    finally:
        server.close()

    scenarios: dict[str, Any] = {}
    for scenario_name, _operation, _index, _phase in FIXTURE_SCENARIOS:
        samples = scenario_samples[scenario_name]
        python_summary = _fixture_runtime_summary(samples, "python")
        native_summary = _fixture_runtime_summary(samples, "native")
        all_pairs_correct = bool(samples) and all(sample["both_correct"] for sample in samples)
        fingerprints_match = bool(samples) and all(
            sample["semantic_fingerprint_match"] for sample in samples
        )
        complete = (
            len(samples) == args.repetitions
            and all_pairs_correct
            and fingerprints_match
            and python_summary["semantic_fingerprint_stable"]
            and native_summary["semantic_fingerprint_stable"]
        )
        scenarios[scenario_name] = {
            "status": "complete" if complete else "incomplete_or_incorrect",
            "title": scenario_name.replace("_", " "),
            "python": {"summary": python_summary},
            "native": {"summary": native_summary},
            "paired_samples": samples,
            "semantic_fingerprints_match_all_pairs": fingerprints_match,
            "all_pairs_correct": all_pairs_correct,
        }

    return {
        "complete": args.repetitions >= REQUIRED_REPETITIONS
        and all(row["status"] == "complete" for row in scenarios.values()),
        "requested_repetitions": args.repetitions,
        "required_repetitions": REQUIRED_REPETITIONS,
        "loopback_server": "127.0.0.1 only",
        "fixed_discovery_delays_seconds": DELAYS_SECONDS,
        "request_protocol": "HTTP GET; 4 source metadata requests in each cold/warm discovery and 4 artifact requests in batch acquisition per runtime and pair",
        "old_runtime_discovery": "one source at a time using the pre-migration Python AsyncClient",
        "native_runtime_discovery": "Rust Engine multi-target scheduler with discovery_workers=4",
        "helper_build": helper_build,
        "max_native_discovery_concurrency": max(
            (value for key, value in concurrency_peaks.items() if key.startswith("native.")),
            default=0,
        ),
        "concurrency_peaks_by_runtime_phase_kind": concurrency_peaks,
        "helper_process_measurements": {
            runtime: [
                {
                    "process_wall_elapsed_seconds": sample.get("process_wall_elapsed_seconds"),
                    "process_tree_peak_rss_bytes": sample.get(
                        "sampled_process_tree_peak_rss_bytes"
                    ),
                    "direct_child_peak_rss_bytes": sample.get("direct_child_peak_rss_bytes"),
                    "temporary_peak_bytes": sample.get("sampled_temporary_peak_bytes"),
                    "temporary_file_count_after_exit": sample.get(
                        "temporary_file_count_after_exit"
                    ),
                    "process_tree_rss_status": sample.get("process_tree_rss_status"),
                    "process_ok": sample.get("process_ok"),
                }
                for sample in helper_samples[runtime]
            ]
            for runtime in ("python", "native")
        },
        "scenarios": scenarios,
    }


def _runtime_summary(samples: list[dict[str, Any]]) -> dict[str, Any]:
    elapsed = [float(sample["elapsed_seconds"]) for sample in samples]
    first_frame = [
        float(sample["first_frame_output_seconds"])
        for sample in samples
        if sample.get("first_frame_output_seconds") is not None
    ]
    rss = [int(sample["direct_child_peak_rss_bytes"]) for sample in samples if sample["direct_child_peak_rss_bytes"] is not None]
    tree_rss = [int(sample["sampled_process_tree_peak_rss_bytes"]) for sample in samples if sample["sampled_process_tree_peak_rss_bytes"] is not None]
    temporary = [int(sample["sampled_temporary_bytes"]) for sample in samples]
    fingerprints = [sample["semantic_fingerprint"] for sample in samples]
    return {
        "sample_count": len(samples),
        "p50_elapsed_seconds": _percentile(elapsed, 0.50),
        "p95_elapsed_seconds": _percentile(elapsed, 0.95),
        "first_frame_output_sample_count": len(first_frame),
        "p50_first_frame_output_seconds": _percentile(first_frame, 0.50),
        "p95_first_frame_output_seconds": _percentile(first_frame, 0.95),
        "direct_child_peak_rss_max_bytes": max(rss) if rss else None,
        "direct_child_peak_rss_p50_bytes": _percentile(rss, 0.50),
        "sampled_process_tree_peak_rss_max_bytes": max(tree_rss) if tree_rss else None,
        "sampled_process_tree_peak_rss_p50_bytes": _percentile(tree_rss, 0.50),
        "sampled_temporary_bytes_max": max(temporary) if temporary else None,
        "semantic_fingerprint_stable": len(set(fingerprints)) <= 1,
        "stable_semantic_fingerprint": fingerprints[0] if fingerprints and len(set(fingerprints)) == 1 else None,
        "all_samples_correct": bool(samples) and all(sample["correctness_ok"] for sample in samples),
    }


def _version_details(
    python_executable: str,
    python_version: str | None,
    package_version: str | None,
    native_path: str | None,
    native_version: str | None,
) -> dict[str, Any]:
    return {
        "python": {
            "argv_prefix": [python_executable, "-m", "radiust"],
            "interpreter_version": python_version,
            "radiust_version": package_version,
        },
        "native": {
            "argv_prefix": [native_path] if native_path else None,
            "executable": native_path,
            "version": native_version,
        },
    }


def _performance_acceptance(
    scenarios: dict[str, Any], loopback: dict[str, Any]
) -> dict[str, Any]:
    def ratio(
        name: str, *, fixture: bool = False, metric: str = "p95_elapsed_seconds"
    ) -> float | None:
        source = loopback["scenarios"] if fixture else scenarios
        row = source.get(name, {})
        python_p95 = row.get("python", {}).get("summary", {}).get(metric)
        native_p95 = row.get("native", {}).get("summary", {}).get(metric)
        if not isinstance(python_p95, (float, int)) or not isinstance(native_p95, (float, int)) or python_p95 <= 0:
            return None
        return float(native_p95) / float(python_p95)

    sc003_ratios = {name: ratio(name) for name in ("list_sources", "discover_all")}
    sc003_passed = all(value is not None and value <= 0.5 for value in sc003_ratios.values())

    sc004_ratios = {
        name: ratio(name, fixture=True)
        for name in ("four_delayed_sources_cold", "four_delayed_sources_warm")
    }
    sc004_rows = [loopback.get("scenarios", {}).get(name, {}) for name in sc004_ratios]
    sc004_correct = all(
        row.get("all_pairs_correct") is True
        and row.get("semantic_fingerprints_match_all_pairs") is True
        for row in sc004_rows
    )
    concurrency = loopback.get("concurrency_peaks_by_runtime_phase_kind", {})
    native_delayed_peaks = [
        concurrency.get(f"native.{phase}.discover", 0) for phase in ("cold", "warm")
    ]
    sc004_passed = (
        sc004_correct
        and all(value is not None and value <= (2.0 / 3.0) for value in sc004_ratios.values())
        and min(native_delayed_peaks, default=0) > 1
        and max(native_delayed_peaks, default=0) <= 4
    )

    cat_row = scenarios.get("cat_file", {})
    cat_ratio = ratio("cat_file", metric="p95_first_frame_output_seconds")
    sc005_passed = (
        cat_row.get("all_pairs_correct") is True
        and cat_row.get("semantic_fingerprints_match_all_pairs") is True
        and cat_ratio is not None
        and cat_ratio <= 0.8
    )

    python_tree = [
        row.get("python", {}).get("summary", {}).get("sampled_process_tree_peak_rss_max_bytes")
        for row in scenarios.values()
    ]
    native_tree = [
        row.get("native", {}).get("summary", {}).get("sampled_process_tree_peak_rss_max_bytes")
        for row in scenarios.values()
    ]
    for row in loopback.get("scenarios", {}).values():
        python_tree.append(row.get("python", {}).get("summary", {}).get("max_process_tree_peak_rss_bytes"))
        native_tree.append(row.get("native", {}).get("summary", {}).get("max_process_tree_peak_rss_bytes"))
    python_temp = [
        row.get("python", {}).get("summary", {}).get("sampled_temporary_bytes_max")
        for row in scenarios.values()
    ]
    native_temp = [
        row.get("native", {}).get("summary", {}).get("sampled_temporary_bytes_max")
        for row in scenarios.values()
    ]
    for row in loopback.get("scenarios", {}).values():
        python_temp.append(row.get("python", {}).get("summary", {}).get("max_temporary_peak_bytes"))
        native_temp.append(row.get("native", {}).get("summary", {}).get("max_temporary_peak_bytes"))
    python_tree_values = [int(value) for value in python_tree if isinstance(value, (int, float))]
    native_tree_values = [int(value) for value in native_tree if isinstance(value, (int, float))]
    python_temp_values = [int(value) for value in python_temp if isinstance(value, (int, float))]
    native_temp_values = [int(value) for value in native_temp if isinstance(value, (int, float))]
    all_cleanups = all(
        row.get("python", {}).get("summary", {}).get("all_temporary_files_removed") is True
        and row.get("native", {}).get("summary", {}).get("all_temporary_files_removed") is True
        for row in loopback.get("scenarios", {}).values()
    )
    concurrency_within_limit = all(value <= 4 for value in concurrency.values())
    sc006_memory_ok = (
        bool(python_tree_values)
        and bool(native_tree_values)
        and max(native_tree_values) <= max(python_tree_values)
        and (not python_temp_values or not native_temp_values or max(native_temp_values) <= max(python_temp_values))
    )
    sc006_passed = sc006_memory_ok and concurrency_within_limit and all_cleanups

    return {
        "SC-003": {
            "passed": sc003_passed,
            "criterion": "native CLI p95 <= 50% of legacy Python p95 for list and offline discover-all",
            "native_to_python_p95_ratios": sc003_ratios,
        },
        "SC-004": {
            "passed": sc004_passed,
            "criterion": "four-source loopback discovery p95 <= 2/3 of legacy serial discovery; one final result per source; native concurrency <= 4",
            "native_to_python_p95_ratios": sc004_ratios,
            "native_delayed_discovery_peak_concurrency": native_delayed_peaks,
            "max_configured_concurrency": 4,
            "semantic_and_request_contracts_passed": sc004_correct,
        },
        "SC-005": {
            "passed": sc005_passed,
            "criterion": "local ANSI first-frame output p95 on a controlled 80x24 PTY improves by at least 20%, with identical selected input and decoded terminal pixels",
            "native_to_python_p95_ratio": cat_ratio,
            "improvement_fraction": 1.0 - cat_ratio if cat_ratio is not None else None,
            "measurement": "elapsed from process launch until all 48x24 ANSI preview cells are observed by the PTY reader; this is terminal-output completion, not emulator paint timing",
            "all_pairs_correct": cat_row.get("all_pairs_correct") is True,
            "semantic_fingerprints_match_all_pairs": cat_row.get("semantic_fingerprints_match_all_pairs") is True,
        },
        "SC-006": {
            "passed": sc006_passed,
            "criterion": "peak process-tree RSS and temporary bytes do not exceed Python; concurrency <= 4; loopback temporary files removed",
            "python_peak_process_tree_rss_bytes": max(python_tree_values) if python_tree_values else None,
            "native_peak_process_tree_rss_bytes": max(native_tree_values) if native_tree_values else None,
            "python_peak_temporary_bytes": max(python_temp_values) if python_temp_values else None,
            "native_peak_temporary_bytes": max(native_temp_values) if native_temp_values else None,
            "concurrency_within_limit": concurrency_within_limit,
            "all_loopback_temporary_files_removed": all_cleanups,
        },
    }


def _benchmark(args: argparse.Namespace) -> dict[str, Any]:
    python_executable = _resolve_executable(args.python)
    python_unavailable = None if python_executable else f"Python executable was not found: {args.python}"
    python_source = Path(args.python_source).expanduser().resolve()
    if not (python_source / "radiust" / "__main__.py").is_file():
        python_unavailable = f"Python source tree does not contain radiust/__main__.py: {python_source}"
    native_path, native_unavailable = _native_candidate(args.native_cli)
    python_prefix = [python_executable, "-m", "radiust"] if python_executable else []
    native_prefix = [native_path] if native_path else []

    with tempfile.TemporaryDirectory(prefix="radiust-bench-probe-") as probe_temp:
        probe_root = Path(probe_temp)
        probe_env = _controlled_environment(os.environ.copy(), probe_root / "runtime", python_source)
        python_caps, package_version, _ = _help_capabilities(
            python_prefix,
            probe_env,
            unavailable_reason=python_unavailable,
        )
        native_caps, native_version, _ = _help_capabilities(
            native_prefix,
            probe_env,
            unavailable_reason=native_unavailable,
        )
        python_code, python_version_text = _probe([python_executable, "--version"], probe_env) if python_executable else (None, "")
        python_version = python_version_text.strip().splitlines()[0] if python_code == 0 and python_version_text.strip() else None
        python_arch_code, python_arch_text = (
            _probe([python_executable, "-c", "import platform; print(platform.machine())"], probe_env)
            if python_executable
            else (None, "")
        )
        python_architecture = python_arch_text.strip().splitlines()[-1].lower() if python_arch_code == 0 and python_arch_text.strip() else None
        if python_executable:
            package_code, package_text = _probe(
                [python_executable, "-c", "import radiust; print(radiust.__version__)"], probe_env
            )
            if package_code == 0 and package_text.strip():
                package_version = "radiust " + package_text.strip().splitlines()[-1]

    scenarios: dict[str, Any] = {}
    baseline_scenarios: list[str] = []
    for scenario in SCENARIOS:
        py_cap = python_caps[scenario.name]
        native_cap = native_caps[scenario.name]
        scenario_supported = py_cap["status"] == "supported" and native_cap["status"] == "supported"
        if not scenario.optional:
            baseline_scenarios.append(scenario.name)

        row: dict[str, Any] = {
            "title": scenario.title,
            "optional_when_unsupported": scenario.optional,
            "capability": {"python": py_cap, "native": native_cap},
            "python": {
                "argv": [*python_prefix, *scenario.python_args] if python_prefix else None,
                "samples": [],
                "summary": _runtime_summary([]),
            },
            "native": {
                "argv": [*native_prefix, *scenario.native_args] if native_prefix else None,
                "samples": [],
                "summary": _runtime_summary([]),
            },
            "paired_samples": [],
        }
        if not scenario_supported:
            support_states = {py_cap["status"], native_cap["status"]}
            row["status"] = "unsupported" if "unsupported" in support_states else "unavailable"
            row["reason"] = "; ".join(
                f"{runtime}: {capability['reason']}"
                for runtime, capability in (("python", py_cap), ("native", native_cap))
                if capability["status"] != "supported" and capability.get("reason")
            )
            scenarios[scenario.name] = row
            continue

        python_samples: list[dict[str, Any]] = []
        native_samples: list[dict[str, Any]] = []
        for repetition in range(args.repetitions):
            python_argv = [*python_prefix, *scenario.python_args]
            native_argv = [*native_prefix, *scenario.native_args]
            if repetition % 2 == 0:
                order = (("python", python_argv, python_samples), ("native", native_argv, native_samples))
            else:
                order = (("native", native_argv, native_samples), ("python", python_argv, python_samples))
            for _runtime, argv, destination in order:
                destination.append(
                    _run_sample(
                        argv,
                        os.environ.copy(),
                        scenario,
                        timeout=args.timeout_seconds,
                        sample_interval=args.sample_interval_seconds,
                        python_source=python_source,
                    )
                )
            left = python_samples[-1]
            right = native_samples[-1]
            row["paired_samples"].append(
                {
                    "repetition": repetition + 1,
                    "run_order": [item[0] for item in order],
                    "python_semantic_fingerprint": left["semantic_fingerprint"],
                    "native_semantic_fingerprint": right["semantic_fingerprint"],
                    "semantic_fingerprint_match": left["semantic_fingerprint"] == right["semantic_fingerprint"],
                    "both_correct": left["correctness_ok"] and right["correctness_ok"],
                }
            )

        row["python"]["samples"] = python_samples
        row["python"]["summary"] = _runtime_summary(python_samples)
        row["native"]["samples"] = native_samples
        row["native"]["summary"] = _runtime_summary(native_samples)
        all_samples_captured = (
            len(python_samples) == args.repetitions and len(native_samples) == args.repetitions
        )
        row["semantic_fingerprints_match_all_pairs"] = all(
            pair["semantic_fingerprint_match"] for pair in row["paired_samples"]
        )
        row["all_pairs_correct"] = all(pair["both_correct"] for pair in row["paired_samples"])
        if not all_samples_captured:
            row["status"] = "incomplete"
        elif not row["all_pairs_correct"]:
            row["status"] = "incorrect_output"
        elif not row["semantic_fingerprints_match_all_pairs"]:
            row["status"] = "semantic_mismatch"
        else:
            row["status"] = "complete"
        scenarios[scenario.name] = row

    loopback_matrix = _run_loopback_fixture_matrix(args, python_executable, python_source)
    machine = platform.machine().lower()
    native_architectures = _macho_architectures(Path(native_path) if native_path else None)
    platform_ok = (
        sys.platform == "darwin"
        and machine in {"arm64", "aarch64"}
        and python_architecture in {"arm64", "aarch64"}
        and "arm64" in native_architectures
    )
    required_complete = all(
        scenarios[name].get("status") == "complete"
        and scenarios[name].get("semantic_fingerprints_match_all_pairs") is True
        and scenarios[name].get("all_pairs_correct") is True
        and scenarios[name]["python"]["summary"]["semantic_fingerprint_stable"]
        and scenarios[name]["native"]["summary"]["semantic_fingerprint_stable"]
        for name in baseline_scenarios
    )
    download_supported = (
        scenarios["download_dry_run"]["capability"]["python"]["status"] == "supported"
        and scenarios["download_dry_run"]["capability"]["native"]["status"] == "supported"
    )
    optional_download_ok = (
        not download_supported
        or (
            scenarios["download_dry_run"].get("status") == "complete"
            and scenarios["download_dry_run"].get("semantic_fingerprints_match_all_pairs") is True
            and scenarios["download_dry_run"].get("all_pairs_correct") is True
            and scenarios["download_dry_run"]["python"]["summary"]["semantic_fingerprint_stable"]
            and scenarios["download_dry_run"]["native"]["summary"]["semantic_fingerprint_stable"]
        )
    )
    baseline_complete = (
        platform_ok
        and args.repetitions >= REQUIRED_REPETITIONS
        and required_complete
        and optional_download_ok
        and loopback_matrix.get("complete") is True
        and not T003_GAPS
    )
    performance_acceptance = _performance_acceptance(scenarios, loopback_matrix)

    git_revision_code, git_revision_text = _probe(["git", "rev-parse", "HEAD"], os.environ.copy(), timeout=10.0)
    git_revision = git_revision_text.strip() if git_revision_code == 0 else None
    git_status_code, git_status_text = _probe(
        ["git", "status", "--porcelain"], os.environ.copy(), timeout=10.0
    )
    native_binary_hash = None
    if native_path:
        with suppress(OSError):
            native_binary_hash = hashlib.sha256(Path(native_path).read_bytes()).hexdigest()
    return {
        "schema_version": 1,
        "benchmark": "radiust-rust-migration-cli",
        "created_at_utc": datetime.now(timezone.utc).isoformat(),
        "baseline_complete": baseline_complete,
        "unimplemented_t003_requirements": T003_GAPS,
        "acceptance": {
            "required_repetitions_per_scenario": REQUIRED_REPETITIONS,
            "requested_repetitions": args.repetitions,
            "repetition_count_met": args.repetitions >= REQUIRED_REPETITIONS,
            "required_scenarios": baseline_scenarios,
            "matched_required_scenarios": required_complete,
            "macos_arm64_environment": platform_ok,
            "python_arm64": python_architecture in {"arm64", "aarch64"},
            "native_cli_arm64": "arm64" in native_architectures,
            "loopback_fixture_matrix_complete": loopback_matrix.get("complete") is True,
        },
        "methodology": {
            "paired": True,
            "pair_order": "alternates Python/native first on each repetition",
            "fresh_isolated_runtime_roots": True,
            "network": "RADIUST_RUNTIME__ALLOW_NETWORK=false for every child; no public network is requested",
            "temporary_bytes": "maximum observed regular-file bytes beneath each isolated runtime root",
            "direct_child_peak_rss": "wait4 ru_maxrss for the directly launched CLI process",
            "process_tree_peak_rss": "sampled ps RSS sum; if a short-lived CLI exits before observation, fall back to wait4 direct-child peak and label the sample",
            "process_tree_rss_sample_interval_seconds": max(0.05, args.sample_interval_seconds),
            "process_tree_rss_may_miss_short_lived_descendants": True,
            "sample_interval_seconds": args.sample_interval_seconds,
            "timeout_seconds": args.timeout_seconds,
            "cat_first_frame": {
                "renderer": "ansi",
                "pty_columns": CAT_TTY_COLUMNS,
                "pty_rows": CAT_TTY_ROWS,
                "preview_columns": CAT_PREVIEW_COLUMNS,
                "preview_rows": CAT_TTY_ROWS,
                "completion_event": "the PTY reader has received every ANSI cell in the complete preview frame",
                "limitation": "measures renderer output delivery to a PTY, not terminal-emulator paint or physical display latency",
                "pixel_validation": "parse foreground/background RGB and block glyph for every cell; paired semantic fingerprints must match for the same input SHA-256",
            },
        },
        "environment": {
            "platform": platform.platform(),
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "python": platform.python_version(),
            "python_executable": sys.executable,
            "python_cli_architecture": python_architecture,
            "python_source_path": str(python_source),
            "git_revision": git_revision,
            "git_worktree_dirty": bool(git_status_text.strip()) if git_status_code == 0 else None,
            "python_package_tree_sha256": _tree_fingerprint(python_source / "radiust"),
            "native_executable_sha256": native_binary_hash,
            "native_executable_architectures": native_architectures,
            "network_setting": "disabled",
        },
        "runtimes": _version_details(
            python_executable or args.python,
            python_version,
            package_version,
            native_path,
            native_version,
        ),
        "scenarios": scenarios,
        "loopback_replay": loopback_matrix,
        "performance_acceptance": performance_acceptance,
        "measurement_limitations": T003_MEASUREMENT_LIMITATIONS,
    }


def _baseline_report(comparison: dict[str, Any]) -> dict[str, Any]:
    """Extract the pre-migration Python measurements into a standalone baseline."""
    scenarios: dict[str, Any] = {}
    for name, row in comparison.get("scenarios", {}).items():
        python = row.get("python", {})
        samples = [
            {
                key: sample.get(key)
                for key in (
                    "repetition",
                    "elapsed_seconds",
                    "first_frame_output_seconds",
                    "returncode",
                    "timed_out",
                    "correctness_ok",
                    "semantic_fingerprint",
                    "direct_child_peak_rss_bytes",
                    "sampled_process_tree_peak_rss_bytes",
                    "sampled_temporary_bytes",
                )
            }
            for sample in python.get("samples", [])
        ]
        scenarios[name] = {
            "title": row.get("title"),
            "status": row.get("status"),
            "summary": python.get("summary", {}),
            "samples": samples,
        }

    loopback = comparison.get("loopback_replay", {})
    loopback_scenarios: dict[str, Any] = {}
    for name, row in loopback.get("scenarios", {}).items():
        samples = []
        request_key = (
            "cold_discovery"
            if name.endswith("_cold") or name == "four_delayed_sources_cold"
            else "warm_discovery"
            if name.endswith("_warm") or name == "four_delayed_sources_warm"
            else "batch_acquisition"
        )
        for sample in row.get("paired_samples", []):
            python = sample.get("python", {})
            count_record = sample.get("request_counts", {}).get("python", {})
            samples.append(
                {
                    "repetition": sample.get("repetition"),
                    "elapsed_seconds": python.get("elapsed_seconds"),
                    "semantic_fingerprint": python.get("semantic_fingerprint"),
                    "correct": python.get("correct"),
                    "process_wall_elapsed_seconds": python.get("process_wall_elapsed_seconds"),
                    "process_tree_peak_rss_bytes": python.get("process_tree_peak_rss_bytes"),
                    "direct_child_peak_rss_bytes": python.get("direct_child_peak_rss_bytes"),
                    "temporary_peak_bytes": python.get("temporary_peak_bytes"),
                    "temporary_file_count_after_exit": python.get(
                        "temporary_file_count_after_exit"
                    ),
                    "request_count": count_record.get(request_key, {}),
                    "full_request_sequence_exact": count_record.get(
                        "cold_discovery", {}
                    ).get("full_helper_sequence_count_ok")
                    and count_record.get("warm_discovery", {}).get(
                        "full_helper_sequence_count_ok"
                    )
                    and count_record.get("batch_acquisition", {}).get(
                        "full_helper_sequence_count_ok"
                    ),
                }
            )
        loopback_scenarios[name] = {
            "status": row.get("status"),
            "summary": row.get("python", {}).get("summary", {}),
            "samples": samples,
        }

    return {
        "schema_version": 1,
        "benchmark": "radiust-pre-migration-python-baseline",
        "created_at_utc": comparison.get("created_at_utc"),
        "baseline_complete": comparison.get("baseline_complete") is True,
        "acceptance": comparison.get("acceptance", {}),
        "environment": comparison.get("environment", {}),
        "legacy_runtime": comparison.get("runtimes", {}).get("python", {}),
        "methodology": comparison.get("methodology", {}),
        "measurement_limitations": comparison.get("measurement_limitations", {}),
        "comparison_native_executable_sha256": comparison.get("environment", {}).get(
            "native_executable_sha256"
        ),
        "scenarios": scenarios,
        "loopback_replay": {
            key: loopback.get(key)
            for key in (
                "complete",
                "requested_repetitions",
                "required_repetitions",
                "loopback_server",
                "fixed_discovery_delays_seconds",
                "request_protocol",
                "old_runtime_discovery",
                "max_native_discovery_concurrency",
            )
        }
        | {"scenarios": loopback_scenarios},
        "unimplemented_t003_requirements": comparison.get(
            "unimplemented_t003_requirements", {}
        ),
    }


def _self_check() -> None:
    assert _percentile([1.0], 0.95) == 1.0
    assert _percentile(list(range(1, 31)), 0.50) == 15.5
    assert math.isclose(_percentile(list(range(1, 31)), 0.95) or 0.0, 28.55)
    assert _parse_ps_snapshot("101 1 10\n102 101 20\n103 102 30\n104 1 40\n", 101) == 60 * 1024
    assert _parse_ps_snapshot("101 1 10\n", 999) is None
    assert _fingerprint({"b": 2, "a": 1}) == _fingerprint({"a": 1, "b": 2})
    payload = {
        "schema_version": 1,
        "command": "list",
        "items": [{"id": "au", "description": "Australia", "availability": "ready", "products": ["composite"]}],
    }
    semantic, valid = _source_semantics(payload)
    assert valid and semantic["sources"][0]["id"] == "au"
    download_golden = json.loads(DOWNLOAD_DRY_RUN_GOLDEN.read_text(encoding="utf-8"))
    download_semantic, download_valid = _download_semantics(download_golden["stdout"])
    assert download_valid and download_semantic["command"] == "download"
    frame = _expected_ansi_frame_pixels()
    ansi_rows = []
    for row in frame:
        ansi_cells = []
        for foreground, background, _glyph in row:
            ansi_cells.append(
                f"\x1b[38;2;{foreground[0]};{foreground[1]};{foreground[2]}m"
                f"\x1b[48;2;{background[0]};{background[1]};{background[2]}m▀"
            )
        ansi_rows.append("".join(ansi_cells) + "\x1b[0m\n")
    cat_semantic, cat_valid = _cat_semantics("".join(ansi_rows), 0, False)
    assert cat_valid and len(cat_semantic["input_sha256"]) == 64
    with tempfile.TemporaryDirectory(prefix="radiust-bench-self-check-") as temporary:
        root = Path(temporary)
        (root / "sample.bin").write_bytes(b"12345")
        assert _tree_file_bytes(root) == 5
        fake_binary = root / "radiust-arm64"
        fake_binary.write_bytes(b"\xcf\xfa\xed\xfe" + struct.pack("<I", 0x0100000C))
        assert _macho_architectures(fake_binary) == ["arm64"]
    with tempfile.TemporaryDirectory(prefix="radiust-bench-self-check-env-") as temporary:
        env = _controlled_environment({}, Path(temporary) / "runtime")
        assert env["RADIUST_RUNTIME__ALLOW_NETWORK"] == "false"
    print("self-check passed")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--python", default=_default_python(), help="Python interpreter used for python -m radiust")
    parser.add_argument(
        "--python-source",
        default=str(ROOT / "python"),
        help="source tree containing radiust/__main__.py; point this at a captured legacy tree for a migration baseline",
    )
    parser.add_argument("--native-cli", help="path to the native radiust executable; defaults to target/release then target/debug")
    parser.add_argument(
        "--repetitions",
        type=int,
        default=REQUIRED_REPETITIONS,
        help=f"paired samples per scenario (default and T003 acceptance count: {REQUIRED_REPETITIONS})",
    )
    parser.add_argument("--timeout-seconds", type=float, default=DEFAULT_TIMEOUT)
    parser.add_argument("--sample-interval-seconds", type=float, default=DEFAULT_SAMPLE_INTERVAL)
    parser.add_argument("--output", type=Path, help="write the complete result as JSON to this path; stdout is used otherwise")
    parser.add_argument(
        "--baseline-output",
        type=Path,
        default=ROOT / "validation-results/rust-migration-baseline.json",
        help="write the legacy-Python-only measurements to this JSON path",
    )
    parser.add_argument("--self-check", action="store_true", help="run lightweight standard-library checks and exit")
    args = parser.parse_args(argv)
    if args.self_check:
        _self_check()
        return 0
    if args.repetitions < 1:
        parser.error("--repetitions must be at least 1")
    if not math.isfinite(args.timeout_seconds) or args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be finite and positive")
    if not math.isfinite(args.sample_interval_seconds) or args.sample_interval_seconds <= 0:
        parser.error("--sample-interval-seconds must be finite and positive")

    result = _benchmark(args)
    serialized = json.dumps(result, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False) + "\n"
    baseline = _baseline_report(result)
    baseline_destination = args.baseline_output.expanduser()
    if not baseline_destination.is_absolute():
        baseline_destination = Path.cwd() / baseline_destination
    baseline_destination.parent.mkdir(parents=True, exist_ok=True)
    baseline_destination.write_text(
        json.dumps(baseline, ensure_ascii=False, sort_keys=True, indent=2, allow_nan=False) + "\n",
        encoding="utf-8",
    )
    if args.output:
        destination = args.output.expanduser()
        if not destination.is_absolute():
            destination = Path.cwd() / destination
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(serialized, encoding="utf-8")
        print(
            f"wrote {destination}; baseline_complete={str(result['baseline_complete']).lower()}; "
            f"repetitions={args.repetitions}; legacy_baseline={baseline_destination}"
        )
    else:
        sys.stdout.write(serialized)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
