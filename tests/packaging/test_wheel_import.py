from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def test_source_tree_import_exposes_version() -> None:
    result = subprocess.run([sys.executable, "-c", "import radiust; print(radiust.__version__)"], check=True, capture_output=True, text=True)
    assert result.stdout.strip()


def test_build_configuration_points_at_maturin() -> None:
    text = Path("pyproject.toml").read_text(encoding="utf-8")
    assert "build-backend = \"maturin\"" in text
    assert "module-name = \"radiust._core\"" in text
