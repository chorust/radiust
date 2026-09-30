import hashlib
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
LOCAL_IMAGES = (
    ("tests/fixtures/sources/th_royalrain/raw/takhli.png", "PNG", "1020x800"),
    ("tests/fixtures/sources/th/raw/kkn240Loop.gif", "GIF", "680x680"),
)
ENTRYPOINTS = ("console", "module")


def _run_cli(entrypoint: str, *arguments: str) -> subprocess.CompletedProcess[str]:
    if entrypoint == "console":
        executable = Path(sys.executable).parent / ("radiust.exe" if os.name == "nt" else "radiust")
        assert executable.is_file(), f"radiust console command is missing: {executable}"
        command = [str(executable), *arguments]
    else:
        command = [sys.executable, "-m", "radiust", *arguments]
    return subprocess.run(
        command,
        cwd=ROOT,
        capture_output=True,
        check=False,
        text=True,
        timeout=30,
    )


@pytest.mark.parametrize("entrypoint", ENTRYPOINTS)
@pytest.mark.parametrize(("relative_path", "image_format", "size"), LOCAL_IMAGES)
def test_local_png_and_gif_render_raw_text_with_unknown_identity(
    entrypoint: str, relative_path: str, image_format: str, size: str
):
    path = ROOT / relative_path
    result = _run_cli(entrypoint, "cat", "--file", str(path), "--renderer", "text")

    assert result.returncode == 0, f"stdout={result.stdout!r}\nstderr={result.stderr!r}"
    assert result.stderr == ""
    expected = (
        f"image path={path.name} raw source=unknown product=unknown station=unknown "
        f"time=unknown units=unknown format={image_format} size={size} "
        f"sha256={hashlib.sha256(path.read_bytes()).hexdigest()} "
        "display=original rule=unknown; original source pixels preserved\n"
    )
    assert result.stdout == expected
    assert "\x1b" not in result.stdout


@pytest.mark.parametrize("entrypoint", ENTRYPOINTS)
@pytest.mark.parametrize(
    ("options", "message"),
    [
        (("--renderer", "ansi"), "requires a TTY"),
        (("--station", "cmp1"), "source query options require SOURCE"),
    ],
)
def test_local_image_rejects_options_outside_local_preview_boundary(
    entrypoint: str, options: tuple[str, ...], message: str
):
    path = ROOT / LOCAL_IMAGES[0][0]
    result = _run_cli(entrypoint, "cat", "--file", str(path), *options)

    assert result.returncode == 2, f"stdout={result.stdout!r}\nstderr={result.stderr!r}"
    assert result.stdout == ""
    assert message in result.stderr


@pytest.mark.parametrize("entrypoint", ENTRYPOINTS)
def test_legacy_display_requires_a_source_identity(entrypoint: str):
    path = ROOT / LOCAL_IMAGES[0][0]
    result = _run_cli(
        entrypoint,
        "cat",
        "--file",
        str(path),
        "--legacy-display",
        "--renderer",
        "text",
    )

    assert result.returncode == 2, f"stdout={result.stdout!r}\nstderr={result.stderr!r}"
    assert result.stdout == ""
    assert "--legacy-display requires SOURCE" in result.stderr
