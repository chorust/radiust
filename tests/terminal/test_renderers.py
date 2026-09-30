from __future__ import annotations

import fcntl
import os
import pty
import select
import struct
import subprocess
import termios
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[2]
IMAGE = ROOT / "tests/fixtures/sources/au/raw/IDR021.T.202609180511.png"


def _native_command(*args: str) -> list[str]:
    binary = ROOT / "target/debug/radiust"
    if binary.is_file():
        return [str(binary), *args]
    return [
        "cargo", "run", "--quiet", "--offline", "--package", "radiust-cli",
        "--bin", "radiust", "--", *args,
    ]


@pytest.mark.skipif(os.name == "nt", reason="POSIX pseudo-terminal required")
def test_native_ansi_renderer_resets_each_row_without_changing_terminal_modes():
    env = os.environ.copy()
    env["TERM"] = "xterm-256color"
    env.pop("NO_COLOR", None)
    env.pop("KITTY_WINDOW_ID", None)
    env.pop("TERM_PROGRAM", None)

    master_fd, slave_fd = pty.openpty()
    fcntl.ioctl(slave_fd, termios.TIOCSWINSZ, struct.pack("HHHH", 10, 30, 0, 0))
    process = subprocess.Popen(
        _native_command("cat", "--file", str(IMAGE), "--renderer", "ansi"),
        cwd=ROOT,
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        env=env,
        close_fds=True,
    )
    os.close(slave_fd)

    output = bytearray()
    deadline = time.monotonic() + 15
    try:
        while time.monotonic() < deadline:
            readable, _, _ = select.select([master_fd], [], [], 0.1)
            if readable:
                try:
                    output.extend(os.read(master_fd, 65536))
                except OSError:
                    break
            elif process.poll() is not None:
                break
        assert process.poll() is not None, "native CLI did not finish in the PTY"
        assert process.returncode == 0
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=3)
        os.close(master_fd)

    transcript = output.decode("utf-8")
    assert "\x1b[38;2;" in transcript
    assert "\x1b[0m" in transcript
    assert "\x1b[?1049h" not in transcript
    assert "\x1b[?1049l" not in transcript
    assert "\x1b[?25l" not in transcript
    assert "\x1b[?2004h" not in transcript
