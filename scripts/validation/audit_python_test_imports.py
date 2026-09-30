#!/usr/bin/env python3
"""Report tests that directly import Python modules excluded from a wheel."""

from __future__ import annotations

import argparse
import ast
import hashlib
import json
from collections import Counter, defaultdict
from pathlib import Path
from zipfile import BadZipFile, ZipFile

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = ROOT / "python/radiust"
TESTS = ROOT / "tests"


def _module_name(path: str) -> str:
    module = path[:-3].replace("/", ".")
    return module.removesuffix(".__init__")


def _source_modules() -> set[str]:
    modules = set()
    for path in PACKAGE.rglob("*.py"):
        relative = path.relative_to(ROOT / "python").as_posix()
        modules.add(_module_name(relative))
    return modules


def _wheel_modules(wheel: Path) -> set[str]:
    with ZipFile(wheel) as archive:
        return {
            _module_name(name)
            for name in archive.namelist()
            if name.startswith("radiust/") and name.endswith(".py")
        }


def _targets(node: ast.AST, source_modules: set[str]) -> set[str]:
    if isinstance(node, ast.Import):
        return {
            alias.name
            for alias in node.names
            if alias.name.startswith("radiust.") and alias.name in source_modules
        }

    if not isinstance(node, ast.ImportFrom) or node.module is None:
        return set()
    if not node.module.startswith("radiust"):
        return set()

    candidates = {node.module} if node.module in source_modules else set()
    for alias in node.names:
        candidate = f"{node.module}.{alias.name}"
        if candidate in source_modules:
            candidates.add(candidate)
    return candidates


def audit(wheel: Path) -> dict[str, object]:
    source_modules = _source_modules()
    packaged_modules = _wheel_modules(wheel)
    excluded_modules = source_modules - packaged_modules
    references: Counter[str] = Counter()
    module_files: dict[str, set[str]] = defaultdict(set)
    matching_files: set[str] = set()
    statement_count = 0
    parse_errors: list[dict[str, str]] = []
    test_files = sorted(TESTS.rglob("*.py"))

    for path in test_files:
        relative = path.relative_to(ROOT).as_posix()
        try:
            tree = ast.parse(path.read_text(encoding="utf-8"), filename=relative)
        except (OSError, SyntaxError) as error:
            parse_errors.append({"path": relative, "error": str(error)})
            continue

        file_matches = False
        for node in ast.walk(tree):
            targets = _targets(node, source_modules) & excluded_modules
            if not targets:
                continue
            statement_count += 1
            file_matches = True
            for module in targets:
                references[module] += 1
                module_files[module].add(relative)
        if file_matches:
            matching_files.add(relative)

    digest = hashlib.sha256(wheel.read_bytes()).hexdigest()
    return {
        "schema_version": 1,
        "check": "python-test-imports-of-wheel-excluded-modules",
        "wheel": {"path": str(wheel), "sha256": digest},
        "scope": "direct AST imports only; dynamically constructed imports are not detected",
        "test_file_count": len(test_files),
        "test_files_with_excluded_module_imports": len(matching_files),
        "direct_import_statement_count": statement_count,
        "distinct_excluded_module_paths": len(references),
        "excluded_module_reference_counts": dict(sorted(references.items())),
        "matching_test_files": sorted(matching_files),
        "parse_errors": parse_errors,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wheel", type=Path, help="wheel whose module list defines exclusions")
    parser.add_argument("--output", type=Path, help="write JSON to this path instead of stdout")
    args = parser.parse_args()
    try:
        result = audit(args.wheel.resolve())
    except (OSError, BadZipFile, ValueError) as error:
        parser.error(str(error))
    payload = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(payload, encoding="utf-8")
    else:
        print(payload, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
