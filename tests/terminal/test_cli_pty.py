from __future__ import annotations

import os
import select
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
