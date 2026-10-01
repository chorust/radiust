from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[2]
IMAGE = ROOT / "tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"


@pytest.fixture(scope="module", autouse=True)
def _build_native_cli_once():
    binary = ROOT / "target/debug/radiust"
    if binary.is_file():
        return
    subprocess.run(
        [
            "cargo",
            "build",
            "--quiet",
            "--offline",
            "--locked",
            "-p",
            "radiust-cli",
            "--bin",
            "radiust",
        ],
        cwd=ROOT,
        check=True,
        timeout=240,
    )


def _native_command(*args: str) -> list[str]:
    binary = ROOT / "target/debug/radiust"
    if binary.is_file():
        return [str(binary), *args]
    return [
        "cargo",
        "run",
        "--quiet",
        "--offline",
        "--package",
        "radiust-cli",
        "--bin",
        "radiust",
        "--",
        *args,
    ]


@pytest.mark.parametrize(
    ("term", "extra"),
    [
        ("xterm-256color", {"KITTY_WINDOW_ID": "123", "TERM_PROGRAM": "iTerm.app"}),
        ("dumb", {}),
        ("xterm-256color", {"COLORTERM": "0"}),
    ],
)
def test_native_auto_renderer_uses_plain_text_when_stdout_is_not_a_tty(term, extra):
    env = os.environ.copy()
    for name in ("NO_COLOR", "KITTY_WINDOW_ID", "TERM_PROGRAM", "COLORTERM"):
        env.pop(name, None)
    env["TERM"] = term
    env.update(extra)

    result = subprocess.run(
        _native_command("cat", "--file", str(IMAGE), "--renderer", "auto"),
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )

    assert result.returncode == 0, result.stderr
    assert "\x1b" not in result.stdout
    assert "\x1b" not in result.stderr
    assert "display=original" in result.stdout
