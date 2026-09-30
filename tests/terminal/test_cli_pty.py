from __future__ import annotations

import json
import os
import select
import subprocess
import sys
import time
from pathlib import Path

import pytest


def test_non_tty_cli_text_and_json_never_emit_terminal_controls():
    from click.testing import CliRunner
    from radiust.cli.main import main

    runner = CliRunner()
    for argv in (["list", "sources", "--json"],
                 ["cat", "--file", "tests/fixtures/sources/th_royalrain/raw/takhli.png",
                  "--renderer", "text"]):
        result = runner.invoke(main, argv)
        assert result.exit_code == 0, result.output
        assert "\x1b" not in result.output and "\r" not in result.output
        if "--json" in argv:
            assert json.loads(result.output)["schema_version"] == 1


def test_root_and_subcommand_quiet_verbose_conflict_before_any_discovery(monkeypatch):
    from click.testing import CliRunner
    from radiust.cli.main import main
    from radiust.client import Client

    calls = []
    monkeypatch.setattr(Client, "discover", lambda *_args: calls.append("discover"))
    cases = (["--quiet", "discover", "th", "--verbose"],
             ["--verbose", "download", "th", "--quiet", "--dry-run"],
             ["config", "show", "--quiet", "--verbose"])
    for argv in cases:
        result = CliRunner().invoke(main, argv)
        assert result.exit_code == 2
        assert "mutually exclusive" in result.output.lower()
    assert calls == []


@pytest.mark.skipif(os.name == "nt", reason="POSIX pseudo-terminal required")
def test_cli_ansi_renderer_runs_in_a_real_pty_process() -> None:
    import pty

    path = Path("tests/fixtures/rust-migration/output/netcdf/field.nc").resolve()

    master_fd, slave_fd = pty.openpty()
    env = os.environ.copy()
    env["TERM"] = "xterm-256color"
    env.pop("NO_COLOR", None)
    process = subprocess.Popen(
        [
            sys.executable,
            "-m",
            "radiust",
            "cat",
            "--file",
            str(path),
            "--variable",
            "reflectivity",
            "--renderer",
            "ansi",
        ],
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        env=env,
        close_fds=True,
    )
    os.close(slave_fd)

    chunks: list[bytes] = []
    deadline = time.monotonic() + 10
    try:
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master_fd], [], [], 0.1)
            if ready:
                try:
                    chunk = os.read(master_fd, 65536)
                except OSError:
                    break
                if not chunk:
                    break
                chunks.append(chunk)
            elif process.poll() is not None:
                break
        if process.poll() is None:
            try:
                return_code = process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                pytest.fail("radiust cat did not exit within 12 seconds in a pseudo-terminal")
        else:
            return_code = process.returncode
        assert return_code == 0, b"".join(chunks).decode("utf-8", errors="replace")
    finally:
        os.close(master_fd)
        if process.poll() is None:
            process.kill()
            process.wait()

    output = b"".join(chunks)
    assert b"\x1b[" in output
    assert b"\xe2\x96\x80" in output  # upper half block used by the ANSI renderer
    assert output.endswith(b"\x1b[0m\r\n")
