#!/usr/bin/env python3
"""Report per-source benchmark gaps independently from CLI migration timing.

The previous runner patched Python ``radiust.pipeline`` transports and
``FixtureSource`` instances. That path no longer measures production behavior:
the public SDK delegates to the Rust Engine, whose built-in adapters currently
have no injectable replay transport. This report keeps the missing evidence
explicit and points to the separately measured native CLI baseline; it never
labels a synthetic Python replay as Rust adapter performance.
"""

from __future__ import annotations

import argparse
import json
import platform
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SOURCES = ("my", "id_sidarma", "rainviewer", "au", "ph", "fr")
BASELINE = ROOT / "validation-results/rust-migration-baseline.json"
REPLAY_GAP = (
    "source adapters now run inside the Rust Engine; the public binding has no deterministic "
    "transport replay injection, so this source-level timing would exercise the retired "
    "Python pipeline rather than production adapters"
)


def _cli_baseline_status() -> dict[str, Any]:
    if not BASELINE.is_file():
        return {"status": "unavailable", "path": str(BASELINE)}
    try:
        report = json.loads(BASELINE.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {"status": "invalid", "path": str(BASELINE)}
    scenarios = report.get("scenarios", {})
    return {
        "status": "available",
        "path": str(BASELINE.relative_to(ROOT)),
        "baseline_complete": report.get("baseline_complete") is True,
        "scenarios": sorted(scenarios),
        "requested_repetitions": report.get("acceptance", {}).get("requested_repetitions"),
    }


def _run_offline() -> dict[str, Any]:
    return {
        "schema_version": 2,
        "task": "per-source Rust adapter performance evidence",
        "task_status": "partial",
        "mode": "offline",
        "measured_at": datetime.now(timezone.utc).isoformat(),
        "environment": {
            "python": platform.python_version(),
            "platform": platform.platform(),
            "architecture": platform.machine(),
        },
        "sources": {
            source: {
                "status": "not_measured",
                "reason": REPLAY_GAP,
                "cold": None,
                "warm": None,
            }
            for source in SOURCES
        },
        "native_cli_baseline": _cli_baseline_status(),
        "measurement_notes": [
            "No source-specific Rust adapter timings, request counts, RSS, or temporary-byte peaks were collected for these six production adapters.",
            "The completed T003 CLI comparison includes a separate four-source loopback scheduler replay; it does not claim canonical provider performance for these six adapters.",
            "This report remains partial until the same permitted canonical source replays can be run through both legacy and Rust adapters.",
        ],
        "old_chain_comparison": {
            "status": "unavailable",
            "reason": "no matched Rust adapter replay and legacy source fixture run under the same harness",
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Write an offline evidence-status report")
    parser.add_argument(
        "--output", type=Path, default=ROOT / "validation-results/rust-source-benchmarks.json"
    )
    args = parser.parse_args(argv)
    if not args.offline:
        parser.error("explicit --offline is required; live benchmarking is not configured")
    report = _run_offline()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    measured = sum(item["status"] == "measured" for item in report["sources"].values())
    print(
        f"Wrote {args.output} ({measured}/{len(SOURCES)} production source adapters measured; "
        "per-source evidence remains partial)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
