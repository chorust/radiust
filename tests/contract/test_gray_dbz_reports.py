"""CLI compatibility contracts for additive gray/dbz reporting."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import h5netcdf
import numpy as np

ROOT = Path(__file__).parents[2]
IMAGE = ROOT / "tests/fixtures/gray-dbz/local/225-codes.png"


def _binary() -> list[str]:
    binary = ROOT / "target/debug/radiust"
    if binary.is_file():
        return [str(binary)]
    return ["cargo", "run", "--quiet", "--offline", "--package", "radiust-cli", "--bin", "radiust", "--"]


def _run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*_binary(), "--json", *args], cwd=ROOT, capture_output=True, text=True, check=False, timeout=60
    )


def test_gray_and_dbz_reports_keep_envelope_and_result_shape():
    for mode, units, actual in (("--gray", "gray_code", "gray"), ("--dbz", "dBZ", "dbz")):
        result = _run("cat", "--file", str(IMAGE), mode, "--renderer", "text")
        assert result.returncode == 0, result.stderr
        report = json.loads(result.stdout)
        assert report["schema_version"] == 1
        assert report["command"] == "cat"
        assert report["error"] is None
        assert report["mode_schema_version"] == 1
        assert report["mode_info"]["actual"] == actual
        assert report["mode_info"]["units"] == units
        assert isinstance(report["result"], str)


def test_decode_failure_has_null_actual_and_json_does_not_receive_migration_noise():
    invalid = ROOT / "tests/fixtures/gray-dbz/local/visible-out-of-range.png"
    result = _run("cat", "--file", str(invalid), "--dbz")
    assert result.returncode == 5
    report = json.loads(result.stdout)
    assert report["mode_schema_version"] == 1
    assert report["error"]["code"] == "invalid_gray_encoding"
    assert report["mode_info"]["actual"] is None
    assert "deprecated" not in result.stderr.lower()


def test_dbz_keeps_real_non_reflectivity_units_in_a_structured_mismatch(tmp_path: Path):
    path = tmp_path / "rain.nc"
    with h5netcdf.File(path, "w") as file:
        file.dimensions = {"y": 2, "x": 2}
        time = file.create_variable("time", (), dtype="f8")
        time[...] = 1_790_380_800.0
        time.attrs["units"] = "seconds since 1970-01-01 00:00:00 UTC"
        field = file.create_variable("rain_rate", ("y", "x"), dtype="f4")
        field[:] = np.asarray([[0.0, 1.0], [2.0, 3.0]], dtype="f4")
        field.attrs["units"] = "mm h-1"

    generic = _run("cat", "--file", str(path), "--variable", "rain_rate", "--decoded")
    assert generic.returncode == 0, generic.stderr
    generic_report = json.loads(generic.stdout)
    assert generic_report["mode_schema_version"] == 1
    assert generic_report["mode_info"]["actual"] == "scientific"
    assert generic_report["mode_info"]["units"] == "mm h-1"
    assert generic_report["result"]["display_mode"] == "decoded"

    result = _run("cat", "--file", str(path), "--variable", "rain_rate", "--dbz")
    assert result.returncode == 5
    report = json.loads(result.stdout)
    assert report["schema_version"] == 1
    assert report["error"]["code"] == "unit_mismatch"
    assert report["mode_info"]["actual"] is None
    assert "mm h-1" in report["error"]["message"]


def test_local_mode_conflict_is_machine_readable_and_preserves_stderr_contract():
    result = _run("cat", "--file", str(IMAGE), "--gray", "--dbz")
    assert result.returncode == 2
    report = json.loads(result.stdout)
    assert report["schema_version"] == 1
    assert report["error"]["code"] == "mode_conflict"
    assert report["mode_info"]["actual"] is None
    assert result.stderr == ""


def test_legacy_cli_alias_warns_only_on_human_stderr():
    human = subprocess.run(
        [*_binary(), "cat", "fr", "--legacy-display"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )
    assert human.returncode == 2
    assert "deprecated; use --gray" in human.stderr
    assert "requires SOURCE" not in human.stderr

    machine = _run("cat", "fr", "--legacy-display")
    assert machine.returncode == 5, machine.stderr
    assert machine.stderr == ""
    machine_report = json.loads(machine.stdout)
    assert machine_report["mode_info"]["requested"] == "gray"
    assert machine_report["mode_info"]["actual"] is None
