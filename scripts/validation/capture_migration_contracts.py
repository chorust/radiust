#!/usr/bin/env python3
"""Capture deterministic, network-disabled CLI compatibility samples."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/fixtures/rust-migration/cli/python-baseline"
IMAGE = ROOT / "tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"
RUN_ID = re.compile(r"^[0-9a-f]{32}$")
SECRET_NAME = r"(?:access[_-]?key|client[_-]?secret|credential|password|passwd|secret(?:[_-]?key)?|token|api[_-]?key)"
SECRET_KEY = re.compile(SECRET_NAME, re.IGNORECASE)
SECRET_ASSIGNMENT = re.compile(rf"(?i)(\b{SECRET_NAME}\b\s*[:=]\s*)([^\s,;\"']+)")
BEARER_TOKEN = re.compile(r"(?i)(\bbearer\s+)[A-Za-z0-9._~+/=-]+")
URL_USERINFO = re.compile(r"(?i)(https?://[^:/\s?#]+:)[^@/\s?#]+(@)")
URL_SECRET_QUERY = re.compile(rf"(?i)([?&]{SECRET_NAME}=)[^&#\s]+")


def _normalize(value: Any, temp_root: Path) -> Any:
    if isinstance(value, dict):
        result = {}
        for key, child in value.items():
            if SECRET_KEY.search(str(key)) and child not in (None, ""):
                result[key] = "[REDACTED]"
            elif key == "run_id" and isinstance(child, str) and RUN_ID.fullmatch(child):
                result[key] = "<RUN_ID>"
            else:
                result[key] = _normalize(child, temp_root)
        return result
    if isinstance(value, list):
        return [_normalize(item, temp_root) for item in value]
    if isinstance(value, str):
        value = value.replace(str(temp_root), "<TEMP>")
        value = URL_USERINFO.sub(r"\1<REDACTED>\2", value)
        value = URL_SECRET_QUERY.sub(r"\1<REDACTED>", value)
        value = SECRET_ASSIGNMENT.sub(r"\1<REDACTED>", value)
        return BEARER_TOKEN.sub(r"\1<REDACTED>", value)
    return value


def _capture_control_case(
    args: list[str], error: BaseException, env: dict[str, str], temp_root: Path, trigger: str
) -> dict[str, Any]:
    from unittest.mock import patch

    from click.testing import CliRunner
    from radiust.cli.main import main

    with patch.dict(os.environ, env, clear=True), patch(
        "radiust.cli.main.sdk_download", side_effect=error
    ):
        result = CliRunner().invoke(main, args)
    try:
        stdout: Any = json.loads(result.stdout)
    except json.JSONDecodeError:
        stdout = result.stdout.strip()
    return {
        "argv": ["radiust", *args],
        "trigger": trigger,
        "exit_code": result.exit_code,
        "stdout": _normalize(stdout, temp_root),
        "stderr": _normalize(result.stderr.strip(), temp_root),
    }


def _capture_empty_discovery_case(env: dict[str, str], temp_root: Path) -> dict[str, Any]:
    from unittest.mock import patch

    from click.testing import CliRunner
    from radiust.cli.main import main

    args = ["discover", "my", "--latest", "--json"]
    with patch.dict(os.environ, env, clear=True), patch(
        "radiust.pipeline.Client.discover", return_value=[]
    ):
        result = CliRunner().invoke(main, args)
    try:
        stdout: Any = json.loads(result.stdout)
    except json.JSONDecodeError:
        stdout = result.stdout.strip()
    return {
        "argv": ["radiust", *args],
        "trigger": "synthetic source discovery returned an empty list",
        "exit_code": result.exit_code,
        "stdout": _normalize(stdout, temp_root),
        "stderr": _normalize(result.stderr.strip(), temp_root),
    }


def _capture_planned_download_case(env: dict[str, str], temp_root: Path) -> dict[str, Any]:
    from datetime import datetime, timezone
    from unittest.mock import patch

    from click.testing import CliRunner
    from radiust.cli.main import main
    from radiust.models import FrameRef

    args = ["download", "au", "--latest", "--dry-run", "--json"]
    frame = FrameRef(
        source="au",
        product="composite",
        station="Sydney",
        valid_time=datetime(2026, 9, 18, 5, 11, tzinfo=timezone.utc),
        revision="fixture-revision",
    )
    with patch.dict(os.environ, env, clear=True), patch(
        "radiust.pipeline.Client.discover", return_value=[frame]
    ):
        result = CliRunner().invoke(main, args)
    try:
        stdout: Any = json.loads(result.stdout)
    except json.JSONDecodeError:
        stdout = result.stdout.strip()
    return {
        "argv": ["radiust", *args],
        "trigger": "synthetic discovery returned one fixture frame; dry-run must not fetch or write",
        "exit_code": result.exit_code,
        "stdout": _normalize(stdout, temp_root),
        "stderr": _normalize(result.stderr.strip(), temp_root),
    }


def capture(temp_root: Path) -> dict[str, Any]:
    cache_dir = temp_root / "cache"
    output_dir = temp_root / "output"
    cache_dir.mkdir(parents=True)
    output_dir.mkdir(parents=True)
    env = {
        key: value
        for key, value in os.environ.items()
        if key not in {"HOME", "XDG_CONFIG_HOME", "APPDATA"}
        and not re.search(SECRET_NAME, key, re.IGNORECASE)
    }
    isolated_home = temp_root / "home"
    isolated_config = temp_root / "config"
    isolated_home.mkdir()
    isolated_config.mkdir()
    env["HOME"] = str(isolated_home)
    env["XDG_CONFIG_HOME"] = str(isolated_config)
    env.update({
        "RADIUST_RUNTIME__ALLOW_NETWORK": "false",
        "RADIUST_CACHE__DIR": str(cache_dir),
        "RADIUST_STORAGE__OUTPUT": str(output_dir),
    })
    cases: dict[str, list[str]] = {
        "list-sources": ["list", "sources", "--json"],
        "discover-all": ["discover", "all", "--json"],
        "discover-single": ["discover", "au", "--latest", "--json"],
        "download-dry-run": ["download", "au", "--latest", "--dry-run", "--json"],
        "cat-file-text": ["cat", "--file", str(IMAGE), "--renderer", "text"],
        "doctor": ["doctor", "--json"],
        "config-show": ["config", "show", "--json"],
        "cache-status": ["cache", "status", "--cache-dir", str(cache_dir), "--output-root", str(output_dir), "--json"],
    }
    results: dict[str, Any] = {}
    for name, args in cases.items():
        proc = subprocess.run(
            [sys.executable, "-m", "radiust", *args],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
            timeout=360,
        )
        try:
            stdout: Any = json.loads(proc.stdout)
        except json.JSONDecodeError:
            stdout = proc.stdout.strip()
        results[name] = {
            "argv": [
                "radiust",
                *[_normalize(str(x).replace(str(ROOT), "<REPO>"), temp_root) for x in args],
            ],
            "exit_code": proc.returncode,
            "stdout": _normalize(stdout, temp_root),
            "stderr": _normalize(proc.stderr.strip(), temp_root),
        }
    results["download-dry-run-network-disabled"] = results["download-dry-run"]
    results["download-dry-run"] = _capture_planned_download_case(env, temp_root)
    results["discover-single-empty"] = _capture_empty_discovery_case(env, temp_root)
    from radiust.errors import BatchError

    results["download-stop"] = _capture_control_case(
        ["download", "au", "--latest", "--raw-only", "--on-error", "stop", "--json"],
        BatchError(
            "batch stopped after first failure",
            partial_result={
                "items": [
                    {"input_index": 0, "status": "failed"},
                    {"input_index": 1, "status": "not_started"},
                ]
            },
        ),
        env,
        temp_root,
        "synthetic BatchError with failed and not_started partial items",
    )
    results["download-interrupt"] = _capture_control_case(
        ["download", "au", "--latest", "--json"],
        KeyboardInterrupt(),
        env,
        temp_root,
        "synthetic KeyboardInterrupt during the SDK operation",
    )
    return {
        "schema_version": 1,
        "runtime": {"network": "disabled", "source": "python/radiust before Rust migration"},
        "cases": results,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, default=FIXTURES)
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="radiust-contracts-") as temp:
        data = capture(Path(temp))
    for name, result in data["cases"].items():
        (args.output_dir / f"{name}.json").write_text(
            json.dumps(result, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
            encoding="utf-8",
        )
    (args.output_dir / "index.json").write_text(
        json.dumps({"schema_version": 1, "cases": sorted(data["cases"])}, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"captured {len(data['cases'])} CLI cases in {args.output_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
