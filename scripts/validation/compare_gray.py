"""Run the canonical native Rust offline gray-rule comparison tool.

Decoding, rule verification, gray transforms, pixel comparisons, and report
creation all run in ``radiust-core``; Python only forwards arguments to Cargo.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path("tests/fixtures/legacy-display/manifest.json"))
    parser.add_argument("--fixture-root", type=Path, default=None)
    parser.add_argument("--report", type=Path, default=Path("validation-results/gray.json"))
    args = parser.parse_args(argv)

    repo_root = Path(__file__).resolve().parents[2]
    manifest = args.manifest if args.manifest.is_absolute() else repo_root / args.manifest
    fixture_root = args.fixture_root or manifest.parent
    if not fixture_root.is_absolute():
        fixture_root = repo_root / fixture_root
    report = args.report if args.report.is_absolute() else repo_root / args.report
    command = [
        "cargo",
        "run",
        "--quiet",
        "--offline",
        "--locked",
        "-p",
        "radiust-core",
        "--example",
        "compare_gray",
        "--",
        "--manifest",
        str(manifest),
        "--fixture-root",
        str(fixture_root),
        "--report",
        str(report),
    ]
    try:
        return subprocess.run(command, cwd=repo_root, check=False).returncode
    except OSError as exc:
        print(f"could not run the native gray comparison: {type(exc).__name__}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
