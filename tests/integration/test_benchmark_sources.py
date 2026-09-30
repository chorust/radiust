"""The source benchmark must not pass off legacy Python timings as Rust evidence."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


def test_offline_report_exposes_missing_native_replay_instead_of_fake_timings(
    tmp_path: Path,
) -> None:
    root = Path(__file__).parents[2]
    report_path = tmp_path / "benchmarks.json"
    result = subprocess.run(
        [
            sys.executable,
            str(root / "scripts/validation/benchmark_sources.py"),
            "--offline",
            "--output",
            str(report_path),
        ],
        cwd=root,
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == 0, result.stderr or result.stdout
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report_text = report_path.read_text(encoding="utf-8")

    assert set(report["sources"]) == {"my", "id_sidarma", "rainviewer", "au", "ph", "fr"}
    for source in report["sources"].values():
        assert source["status"] == "not_measured"
        assert source["cold"] is None
        assert source["warm"] is None
        assert "Rust Engine" in source["reason"]
    assert "benchmark-only-token" not in report_text
    assert report["task_status"] == "partial"
    assert report["native_cli_baseline"]["status"] == "available"
    assert report["native_cli_baseline"]["baseline_complete"] is True
    assert report["native_cli_baseline"]["requested_repetitions"] == 30
    assert report["old_chain_comparison"]["status"] == "unavailable"
