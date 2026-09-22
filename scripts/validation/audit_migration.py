"""Audit source migration coverage and explicit specification gates.

The audit is intentionally structural. It does not turn a replay fixture into
provider science evidence and it does not print credential values. A normal
run is useful in offline CI even while external acceptance gates are open;
``--require-complete`` is available for a release check after those gates are
closed.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

TASK_RE = re.compile(r"^- \[([ Xx])\] (T\d+) (.+)$", re.MULTILINE)


def _read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object: {path}")
    return value


def _task_report(path: Path) -> tuple[dict[str, int], list[str], list[dict[str, str]]]:
    rows = TASK_RE.findall(path.read_text(encoding="utf-8"))
    open_tasks = [task_id for mark, task_id, _description in rows if mark == " "]
    descriptions = [
        {"id": task_id, "description": description}
        for mark, task_id, description in rows
        if mark == " "
    ]
    counts = {
        "total": len(rows),
        "checked": sum(mark.lower() == "x" for mark, _task_id, _description in rows),
        "open": len(open_tasks),
    }
    return counts, open_tasks, descriptions


def build_report(root: Path) -> dict[str, Any]:
    """Return a secret-free migration coverage report for ``root``."""

    root = root.resolve()
    inventory = _read_json(root / "migration/inventory.json")
    sources = inventory.get("sources", [])
    if not isinstance(sources, list):
        raise ValueError("migration/inventory.json sources must be a list")

    missing: list[dict[str, Any]] = []
    source_rows: list[dict[str, Any]] = []
    for item in sources:
        if not isinstance(item, dict) or not isinstance(item.get("id"), str):
            missing.append({"source": None, "paths": ["invalid inventory row"]})
            continue
        source_id = item["id"]
        module_name = source_id.replace("-", "_")
        paths = {
            "adapter": root / "python/radiust/sources" / f"{module_name}.py",
            "resource": root / "python/radiust/resources/sources" / f"{source_id}.json",
            "test": root / "tests/sources" / f"test_{module_name}.py",
            "fixture": root / item["fixture"],
            "migration": root / item["migration"],
        }
        absent = [name for name, path in paths.items() if not path.is_file()]
        if absent:
            missing.append({"source": source_id, "paths": absent})

        row: dict[str, Any] = {"id": source_id, "status": item.get("status")}
        if not absent:
            resource = _read_json(paths["resource"])
            fixture = _read_json(paths["fixture"])
            migration = _read_json(paths["migration"])
            if resource.get("source") != source_id:
                missing.append({"source": source_id, "paths": ["resource.source"]})
            if fixture.get("source") != source_id:
                missing.append({"source": source_id, "paths": ["fixture.source"]})
            if migration.get("source") != source_id:
                missing.append({"source": source_id, "paths": ["migration.source"]})
            expected_adapter = f"python/radiust/sources/{module_name}.py"
            if migration.get("adapter") != expected_adapter:
                missing.append({"source": source_id, "paths": ["migration.adapter"]})
            row["fixture_status"] = fixture.get("status", "available")
            row["migration_status"] = migration.get("status")
        source_rows.append(row)

    task_counts, open_tasks, open_task_details = _task_report(
        root / "specs/001-radiust-v1-migration/tasks.md"
    )
    return {
        "schema_version": 1,
        "source_count": len(sources),
        "sources": source_rows,
        "missing": missing,
        "task_counts": task_counts,
        "open_tasks": open_tasks,
        "open_task_details": open_task_details,
        "ready_for_v1": not missing and not open_tasks,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--output", type=Path, help="write the JSON report to this path")
    parser.add_argument("--json", action="store_true", help="print the full JSON report")
    parser.add_argument(
        "--require-complete",
        action="store_true",
        help="fail if any specification task remains open",
    )
    args = parser.parse_args(argv)
    try:
        report = build_report(args.root)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"migration audit failed: {exc}", file=sys.stderr)
        return 2

    payload = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(payload, encoding="utf-8")
    if args.json or not args.output:
        print(payload, end="")
    if report["missing"]:
        return 2
    if args.require_complete and report["open_tasks"]:
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
