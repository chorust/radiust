"""Native CLI contracts for raw and legacy display boundaries."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).parents[2]
IMAGE = ROOT / "tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"


def _native_command(*args: str) -> list[str]:
    binary = ROOT / "target/debug/radiust"
    if binary.is_file():
        return [str(binary), *args]
    return [
        "cargo", "run", "--quiet", "--offline", "--package", "radiust-cli",
        "--bin", "radiust", "--", *args,
    ]


def _run_native(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        _native_command(*args),
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )


def test_native_local_preview_preserves_raw_identity_and_marks_original_display():
    result = _run_native(
        "cat", "--file", str(IMAGE), "--renderer", "text", "--json",
    )

    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert report["schema_version"] == 1
    assert report["command"] == "cat"
    assert report["error"] is None
    assert "display=original" in report["result"]
    assert "original source pixels preserved" in report["result"]
    assert hashlib.sha256(IMAGE.read_bytes()).hexdigest() in report["result"]


def test_native_cli_rejects_legacy_display_without_a_source_identity():
    result = _run_native(
        "cat", "--file", str(IMAGE), "--legacy-display", "--renderer", "text",
    )

    assert result.returncode == 2
    assert result.stdout == ""
    assert "--legacy-display requires SOURCE" in result.stderr
