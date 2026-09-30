from __future__ import annotations

import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).parents[2]
IMAGE = ROOT / "tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"
NETCDF = ROOT / "tests/fixtures/rust-migration/output/netcdf/field.nc"


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


def test_native_text_and_json_transcripts_do_not_contain_terminal_controls():
    result = _run_native(
        "cat", "--file", str(IMAGE), "--renderer", "text", "--json",
    )

    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert report["command"] == "cat"
    assert "image path=IDR021.T.202609180511.png" in report["result"]
    assert "\x1b" not in result.stdout
    assert "\x1b" not in result.stderr


def test_native_image_renderer_rejects_a_pipe_before_writing_output():
    result = _run_native("cat", "--file", str(IMAGE), "--renderer", "ansi")

    assert result.returncode == 2
    assert result.stdout == ""
    assert "renderer ansi requires a TTY" in result.stderr


def test_native_netcdf_text_contract_reports_the_selected_field():
    result = _run_native(
        "cat", "--file", str(NETCDF), "--variable", "reflectivity",
        "--renderer", "text", "--json",
    )

    assert result.returncode == 0, result.stderr
    report = json.loads(result.stdout)
    assert report["result"]["variable"] == "reflectivity"
    assert report["result"]["valid_time"] == "2026-09-26T01:02:03Z"
    assert report["result"]["units"] == "dBZ"
    assert report["result"]["display_mode"] == "decoded"
    assert "\x1b" not in result.stdout
