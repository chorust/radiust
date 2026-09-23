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


def _audit_display(root: Path) -> dict[str, Any]:
    """Check per-path display evidence without promoting scientific status."""
    from radiust.display.rules import DisplayEvidence

    inventory = _read_json(root / "migration/legacy-display-inventory.json")
    schema = _read_json(root / "migration/legacy-display.schema.json")
    manifest = _read_json(root / "tests/fixtures/legacy-display/manifest.json")
    replay = _read_json(root / "validation-results/legacy-display.json")
    packaged = _read_json(root / "python/radiust/resources/legacy_display/index.json")
    errors: list[str] = []
    if inventory.get("schema_version") != 1 or schema.get("$schema") is None:
        errors.append("legacy display inventory or schema version is invalid")
    definitions = schema.get("$defs", {})
    required_inventory = set(definitions.get("inventoryPath", {}).get("required", []))
    required_evidence = set(definitions.get("evidence", {}).get("required", []))

    def index(items: object, label: str) -> dict[str, dict[str, Any]]:
        if not isinstance(items, list):
            errors.append(f"{label} paths must be an array")
            return {}
        results: dict[str, dict[str, Any]] = {}
        for item in items:
            if not isinstance(item, dict) or not isinstance(item.get("path_id"), str):
                errors.append(f"{label} contains an invalid path")
                continue
            path_id = item["path_id"]
            if path_id in results:
                errors.append(f"{label} has duplicate {path_id}")
            results[path_id] = item
        return results

    source_paths = index(inventory.get("paths"), "inventory")
    manifest_paths = index(manifest.get("entries"), "manifest")
    replay_paths = index(replay.get("paths"), "replay")
    packaged_paths = index(packaged.get("paths"), "packaged index")
    for label, path_set in (("manifest", manifest_paths), ("replay", replay_paths),
                            ("packaged index", packaged_paths)):
        if set(path_set) != set(source_paths):
            errors.append(f"{label} path coverage differs from inventory")
    per_source: dict[str, dict[str, dict[str, Any]]] = {}
    for path_id, row in source_paths.items():
        source, product = row.get("source"), row.get("product")
        if not isinstance(source, str) or not isinstance(product, str) or not (
            path_id == f"{source}/{product}" or path_id.startswith(f"{source}/{product}/")
        ):
            errors.append(f"inventory identity invalid for {path_id}")
            continue
        if not required_inventory <= set(row):
            errors.append(f"inventory path lacks schema-required fields: {path_id}")
        if source not in per_source:
            migration_file = root / "migration/sources" / f"{source}.json"
            try:
                migration = _read_json(migration_file)
                display = migration.get("display_migration", {})
                per_source[source] = index(display.get("paths"), f"source {source}")
                if display.get("schema_version") != 1:
                    errors.append(f"source {source} display migration schema version is invalid")
            except (OSError, ValueError, json.JSONDecodeError):
                errors.append(f"source {source} has no valid display migration record")
                per_source[source] = {}
        if path_id not in per_source[source]:
            errors.append(f"source {source} is missing display path {path_id}")
            continue
        record = per_source[source][path_id]
        if not required_evidence <= set(record):
            errors.append(f"source {source} display evidence lacks required fields: {path_id}")
        try:
            DisplayEvidence.from_mapping(record)
        except (TypeError, ValueError):
            errors.append(f"source {source} has invalid display evidence: {path_id}")
        if record.get("status") != row.get("status") or record.get("scientific_status_unchanged") is not True:
            errors.append(f"source {source} display status differs from inventory: {path_id}")
        for other, label in ((manifest_paths.get(path_id), "manifest"),
                             (replay_paths.get(path_id), "replay"),
                             (packaged_paths.get(path_id), "packaged index")):
            if other is None or other.get("status") != row.get("status"):
                errors.append(f"{label} display status differs: {path_id}")
        if row.get("status") == "passed":
            if not row.get("old_config_verified") or not row.get("source_baseline_verified"):
                errors.append(f"passed display lacks old-rule or legal-baseline verification: {path_id}")
            if not record.get("input_hashes") or not record.get("baseline_hash"):
                errors.append(f"passed display lacks source-matched fingerprints: {path_id}")
        elif row.get("status") == "blocked" and not row.get("blocked_reasons"):
            errors.append(f"blocked display has no missing-material explanation: {path_id}")
    for source, entries in per_source.items():
        expected = {key for key, row in source_paths.items() if row.get("source") == source}
        if set(entries) != expected:
            errors.append(f"source {source} display migration has missing or extra paths")
    counts = {status: sum(row.get("status") == status for row in source_paths.values())
              for status in ("passed", "difference_pending", "blocked")}
    if replay.get("counts") != {"total": len(source_paths), **counts}:
        errors.append("legacy replay status totals differ from inventory")
    if manifest.get("coverage_status") != inventory.get("coverage_status"):
        errors.append("manifest and inventory declare different coverage status")
    coverage_closed = inventory.get("coverage_status", "").startswith("verified:")
    return {
        "total_paths": len(source_paths), "source_count": len(per_source), "counts": counts,
        "coverage_closed": coverage_closed, "structural_errors": sorted(set(errors)),
        "ready": bool(source_paths) and not errors and coverage_closed and counts["passed"] == len(source_paths),
    }


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
    display = _audit_display(root)
    return {
        "schema_version": 1,
        "source_count": len(sources),
        "sources": source_rows,
        "missing": missing,
        "task_counts": task_counts,
        "open_tasks": open_tasks,
        "open_task_details": open_task_details,
        "display_migration": display,
        "ready_for_v1": not missing and not open_tasks and display["ready"],
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
    if args.require_complete and (report["open_tasks"] or not report["display_migration"]["ready"]):
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
