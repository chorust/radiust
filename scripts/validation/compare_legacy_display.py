"""Compatibility entry point for the canonical gray comparison command."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def main(argv: list[str] | None = None) -> int:
    repo_root = Path(__file__).resolve().parents[2]
    canonical = Path(__file__).with_name("compare_gray.py")
    command = [
        sys.executable,
        str(canonical),
        "--report",
        "validation-results/legacy-display.json",
        *(sys.argv[1:] if argv is None else argv),
    ]
    try:
        return subprocess.run(command, cwd=repo_root, check=False).returncode
    except OSError as exc:
        print(f"could not run the compatibility gray comparison: {type(exc).__name__}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
