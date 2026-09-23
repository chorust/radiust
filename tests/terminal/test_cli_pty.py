from __future__ import annotations

import json
import os
import select
import signal
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import pytest
import xarray as xr
from radiust.field import RadarField
from radiust.grids import GeographicGrid
from radiust.outputs.netcdf import write_netcdf


@pytest.mark.parametrize("columns", [40, 80, 120])
def test_human_report_width_matrix_preserves_multilingual_content(columns):
    from radiust.cli.layout import cell_width, render

    source = "雷达站A\u0301" * 12
    station = "station-" + "z" * 90
    reason = "上游暂时无法响应" * 12
    stamp = "2026-09-22T02:03:04Z"
    lines = render([{"source": source, "station": station, "status": "failed",
                     "valid_time": stamp, "error": {"message": reason}}],
                   command="discover", columns=columns).splitlines()
    assert all(cell_width(line) <= columns for line in lines)
    content = "".join(lines).replace(" ", "")
    for expected in (source, station, stamp, reason, "failed"):
        assert expected in content


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
def test_terminal_session_restores_termios_after_real_sigint():
    import pty
    import termios

    program = (
        "import sys, time, termios\n"
        "from radiust.terminal.session import TerminalSession\n"
        "with TerminalSession(sys.stdout):\n"
        "    fd = sys.stdout.fileno()\n"
        "    state = termios.tcgetattr(fd)\n"
        "    state[3] &= ~termios.ECHO\n"
        "    termios.tcsetattr(fd, termios.TCSANOW, state)\n"
        "    sys.stdout.write('PTY_READY\\n')\n"
        "    sys.stdout.flush()\n"
        "    time.sleep(30)\n"
    )
    master, slave = pty.openpty()
    baseline = termios.tcgetattr(slave)
    proc = subprocess.Popen([sys.executable, "-c", program], stdin=slave,
                            stdout=slave, stderr=slave, close_fds=True)
    try:
        output = bytearray()
        deadline = time.monotonic() + 5
        while b"PTY_READY" not in output and time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.1)
            if ready:
                output.extend(os.read(master, 1024))
            if proc.poll() is not None:
                break
        assert b"PTY_READY" in output, bytes(output)
        assert not termios.tcgetattr(slave)[3] & termios.ECHO
        os.kill(proc.pid, signal.SIGINT)
        assert proc.wait(timeout=3) != 0
        assert termios.tcgetattr(slave) == baseline
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=3)
        os.close(slave)
        os.close(master)


@pytest.mark.skipif(os.name == "nt", reason="POSIX pseudo-terminal required")
@pytest.mark.parametrize("no_color", [False, True])
def test_real_pty_progress_stays_on_stderr_and_clears_before_image(no_color):
    import pty

    program = (
        "import sys\n"
        "from radiust.cli.progress import Progress, ProgressEvent\n"
        "from radiust.display.raw import preview_bytes\n"
        "from radiust.terminal.api import show\n"
        "from tests.support.cli_experience import image_bytes\n"
        "with Progress(stream=sys.stderr) as progress:\n"
        "    progress.update(ProgressEvent('discover', 1, None))\n"
        "    progress.update(ProgressEvent('acquire', 1, 1))\n"
        "show(preview_bytes(image_bytes(), name='frame.png'), renderer='text')\n"
    )
    master, slave = pty.openpty()
    env = os.environ.copy()
    env["TERM"] = "xterm-256color"
    if no_color:
        env["NO_COLOR"] = "1"
    else:
        env.pop("NO_COLOR", None)
    proc = subprocess.Popen([sys.executable, "-c", program], stdin=slave,
                            stdout=subprocess.PIPE, stderr=slave, env=env, close_fds=True)
    os.close(slave)
    try:
        output = bytearray()
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            ready, _, _ = select.select([master], [], [], 0.1)
            if not ready:
                if proc.poll() is not None:
                    break
                continue
            try:
                data = os.read(master, 65536)
            except OSError:
                break
            if not data:
                break
            output.extend(data)
        stdout, _ = proc.communicate(timeout=3)
        assert proc.returncode == 0, bytes(output)
        assert b"discover 1" in output and b"acquire 1/1" in output
        assert b"%" not in output
        assert b"raw" in stdout and b"\x1b" not in stdout and b"discover" not in stdout
        if no_color:
            assert b"\x1b" not in output
        else:
            assert output.endswith(b"\r\x1b[2K")
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=3)
        os.close(master)


@pytest.mark.skipif(os.name == "nt", reason="POSIX pseudo-terminal required")
def test_cli_ansi_renderer_runs_in_a_real_pty_process(tmp_path: Path) -> None:
    import pty

    field = RadarField(
        xr.DataArray(
            np.array([[1.0, 2.0], [3.0, 4.0]], dtype="float32"),
            dims=("latitude", "longitude"),
            name="reflectivity",
            attrs={"units": "dBZ"},
        ),
        GeographicGrid([1.0, 0.0], [0.0, 1.0]),
        provenance={
            "source": "pty-fixture",
            "product": "composite",
            "valid_time": "2026-09-21T00:00:00Z",
        },
    )
    path = tmp_path / "pty-field.nc"
    write_netcdf(field, path)

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
            "--renderer",
            "ansi",
            "--width",
            "2",
            "--height",
            "1",
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
        assert return_code == 0
    finally:
        os.close(master_fd)
        if process.poll() is None:
            process.kill()
            process.wait()

    output = b"".join(chunks)
    assert b"pty-fixture / composite" in output
    assert b"legend=" in output
    assert b"\x1b[" in output
    assert b"\xe2\x96\x80" in output  # upper half block used by the ANSI renderer
