"""Opt-in real-Chromium startup replay for the native Rust CDP session."""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest

pytestmark = pytest.mark.live
ROOT = Path(__file__).parents[2]
RUST_TEST = "source::browser::tests::real_chromium_cdp_handshake_and_private_profile_cleanup"


def _chromium_executable() -> str | None:
    configured = os.environ.get("RADIUST_CHROMIUM_EXECUTABLE")
    if configured and Path(configured).is_file():
        return configured
    for name in ("chromium", "chromium-browser", "google-chrome", "google-chrome-stable"):
        if executable := shutil.which(name):
            return executable
    try:
        from playwright.sync_api import sync_playwright

        with sync_playwright() as playwright:
            executable = Path(playwright.chromium.executable_path)
            if executable.is_file():
                return str(executable)
    except (ImportError, OSError):
        return None
    return None


def test_real_chromium_cdp_handshake_and_private_profile_cleanup() -> None:
    if os.environ.get("RADIUST_TEST_REAL_BROWSER") != "1":
        pytest.skip("set RADIUST_TEST_REAL_BROWSER=1 to run the Rust Chromium smoke test")
    executable = _chromium_executable()
    if executable is None:
        pytest.skip("no Chromium executable is installed")
    cargo = shutil.which("cargo")
    if cargo is None:
        pytest.skip("cargo is unavailable; the opt-in Rust browser test cannot run")

    env = os.environ.copy()
    wrapper_root = None
    browser_log = None
    if env.get("RADIUST_TEST_CHROMIUM_NO_SANDBOX") == "1":
        wrapper_root = tempfile.TemporaryDirectory(prefix="radiust-chromium-test-")
        wrapper = Path(wrapper_root.name) / "chromium-wrapper"
        browser_log = Path(wrapper_root.name) / "chromium-stderr.log"
        wrapper.write_text(
            "#!/bin/sh\n"
            f"exec {shlex.quote(executable)} --no-sandbox --disable-dev-shm-usage "
            f'--enable-logging=stderr "$@" 2>{shlex.quote(str(browser_log))}\n',
            encoding="utf-8",
        )
        wrapper.chmod(0o755)
        executable = str(wrapper)
    env["RADIUST_CHROMIUM_EXECUTABLE"] = executable
    try:
        result = subprocess.run(
            [
                cargo,
                "test",
                "-p",
                "radiust-core",
                "--lib",
                "--offline",
                "--locked",
                RUST_TEST,
                "--",
                "--ignored",
                "--exact",
                "--nocapture",
            ],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            timeout=240,
            check=False,
        )
        if browser_log is not None and browser_log.is_file():
            chromium_stderr = browser_log.read_text(encoding="utf-8", errors="replace")
            if chromium_stderr:
                result.stderr = f"{result.stderr}\nChromium stderr:\n{chromium_stderr}"
    finally:
        if wrapper_root is not None:
            wrapper_root.cleanup()
    assert result.returncode == 0, f"native Chromium test failed:\n{result.stdout}\n{result.stderr}"
