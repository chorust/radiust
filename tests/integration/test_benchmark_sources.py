"""The offline benchmark must distinguish real fixtures from synthetic replay inputs."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


def test_offline_benchmark_records_actual_scope_and_unavailable_sources(tmp_path: Path) -> None:
    root = Path(__file__).parents[2]
    report_path = tmp_path / "benchmarks.json"
    result = subprocess.run(
        [sys.executable, str(root / "scripts/validation/benchmark_sources.py"), "--offline", "--output", str(report_path)],
        cwd=root,
        capture_output=True,
        text=True,
        timeout=120,
        check=False,
    )
    assert result.returncode == 0, result.stderr or result.stdout
    report = json.loads(report_path.read_text(encoding="utf-8"))
    report_text = report_path.read_text(encoding="utf-8")
    sources = report["sources"]
    assert set(sources) == {"my", "id_sidarma", "rainviewer", "au", "ph", "fr"}
    for name in ("my", "id_sidarma", "ph"):
        assert sources[name]["status"] == "synthetic_replay_only"
        assert sources[name]["canonical_fixture_status"] == "unavailable"
        assert sources[name]["cold"]["scope"] == "registered_adapter_synthetic_replay_raw_acquisition_no_scientific_decode"
        assert sources[name]["warm"]["scope"] == "registered_adapter_synthetic_replay_raw_acquisition_no_scientific_decode"
        assert sources[name]["cold"]["cache_hits_observed"] == 0
        assert sources[name]["warm"]["cache_hits_observed"] >= 1
        expected_requests = {"my": (3, 2), "id_sidarma": (2, 1), "ph": (3, 2)}
        expected_cold_requests, expected_warm_requests = expected_requests[name]
        assert sources[name]["cold"]["request_count"] == expected_cold_requests
        assert sources[name]["warm"]["request_count"] == expected_warm_requests
        assert sources[name]["cold"]["input_bytes"] > 0
        assert sources[name]["warm"]["input_bytes"] == sources[name]["cold"]["input_bytes"]
    assert "benchmark-only-token" not in report_text
    assert report["task_status"] == "partial"
    assert sum(item["status"] == "measured" for item in sources.values()) == 3
    assert sources["rainviewer"]["cold"]["scope"].startswith("registered_adapter_discover_acquire_decode")
    assert sources["rainviewer"]["cold"]["request_count"] == 5
    assert sources["rainviewer"]["warm"]["request_count"] == 5  # Tiles intentionally do not persist in raw cache.
    for name in ("au", "fr"):
        assert sources[name]["cold"]["request_count"] == 2
        expected_warm_requests = 0 if name == "fr" else 1
        assert sources[name]["warm"]["request_count"] == expected_warm_requests
        assert sources[name]["warm"]["cache_hits_observed"] >= 1
        assert "no_scientific_decode" in sources[name]["cold"]["scope"]
    assert report["old_chain_comparison"]["status"] == "unavailable"
