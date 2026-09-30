#!/usr/bin/env python3
"""Audit an installed Radiust macOS wheel and its Mach-O dependencies.

This check covers clean installation, the installed CLI/Python extension, and
Mach-O architecture/dependency inspection. Optional format readback installs
the wheel's science/output extras into the isolated environment.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any
from zipfile import BadZipFile, ZipFile

ROOT = Path(__file__).resolve().parents[2]
ARCHITECTURES = ("arm64", "x86_64")
MACOS_VERSION_RE = re.compile(r"^(\d+)\.(\d+)$")
MACHO_MAGICS = (
    b"\xfe\xed\xfa\xce",
    b"\xce\xfa\xed\xfe",
    b"\xfe\xed\xfa\xcf",
    b"\xcf\xfa\xed\xfe",
    b"\xca\xfe\xba\xbe",
    b"\xbe\xba\xfe\xca",
    b"\xca\xfe\xba\xbf",
    b"\xbf\xba\xfe\xca",
)
HOMEBREW_REFERENCE_RE = re.compile(
    r"(?<![A-Za-z0-9_])/(?:opt/homebrew(?:/|$)|usr/local/(?:Cellar|opt)(?:/|$))"
)
DEPENDENCY_RE = re.compile(r"^\s*(.+?) \(compatibility version [^)]+\)$")
RPATH_RE = re.compile(r"^\s*path (.+?) \(offset \d+\)$")


class AuditError(RuntimeError):
    """A release audit failure with a stable report check name."""

    def __init__(self, check: str, message: str):
        super().__init__(message)
        self.check = check


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wheel", type=Path, help="macOS wheel file to audit")
    parser.add_argument(
        "--expected-architecture",
        required=True,
        choices=ARCHITECTURES,
        help="architecture expected for both this host and the wheel",
    )
    parser.add_argument(
        "--expected-macos-version",
        default="11.0",
        help="minimum macOS deployment version expected in the wheel tag",
    )
    parser.add_argument(
        "--with-format-readback",
        action="store_true",
        help="install wheel extras and run independent PNG/NetCDF/GeoTIFF/Zarr readback",
    )
    parser.add_argument(
        "--index-url",
        help="optional package index override for the isolated wheel installation",
    )
    parser.add_argument(
        "--report",
        type=Path,
        help="write the structured JSON result to this path",
    )
    return parser


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    return build_parser().parse_args(argv)


def host_error(system: str, machine: str, expected_architecture: str) -> str | None:
    """Return why a host is unsuitable, or ``None`` when it matches."""
    if system != "Darwin":
        return f"macOS is required; detected host system {system!r}"
    aliases = {"aarch64": "arm64", "AMD64": "x86_64", "amd64": "x86_64"}
    normalized_machine = aliases.get(machine, machine)
    if normalized_machine != expected_architecture:
        return (
            f"host architecture {normalized_machine!r} does not match explicit expected "
            f"architecture {expected_architecture!r}"
        )
    return None


def new_report(
    wheel: Path,
    expected_architecture: str,
    system: str,
    machine: str,
    expected_macos_version: str = "11.0",
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "check": "macos-wheel-release-audit",
        "status": "running",
        "scope": {
            "installed_wheel_smoke": True,
            "macho_dependency_audit": True,
            "format_readback": "not_performed",
        },
        "wheel": {"path": str(wheel), "sha256": None},
        "expected_architecture": expected_architecture,
        "expected_macos_version": expected_macos_version,
        "host": {"system": system, "machine": machine},
        "checks": {},
        "errors": [],
    }


def record_check(
    report: dict[str, Any],
    name: str,
    passed: bool,
    *,
    details: Any = None,
    error: str | None = None,
) -> None:
    result: dict[str, Any] = {"passed": passed}
    if details is not None:
        result["details"] = details
    if error is not None:
        result["error"] = error
        report["errors"].append({"check": name, "message": error})
    report["checks"][name] = result


def finish_report(report: dict[str, Any]) -> dict[str, Any]:
    report["errors"] = sorted(
        report["errors"], key=lambda item: (item["check"], item["message"])
    )
    report["status"] = "passed" if not report["errors"] else "failed"
    return report


def serialize_report(report: dict[str, Any]) -> str:
    return json.dumps(report, ensure_ascii=False, sort_keys=True, indent=2) + "\n"


def write_report(path: Path, report: dict[str, Any]) -> None:
    path = path.expanduser()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(serialize_report(report), encoding="utf-8")


def _clean_environment(temp_root: Path, venv_bin: Path | None = None) -> dict[str, str]:
    """Build a predictable environment without developer Python/Homebrew paths."""
    removed_prefixes = ("DYLD_", "GDAL_", "PROJ_", "PYTHON", "PIP_", "RADIUST_")
    removed_names = {"VIRTUAL_ENV", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"}
    env = {
        key: value
        for key, value in os.environ.items()
        if key not in removed_names and not key.startswith(removed_prefixes)
    }
    home = temp_root / "home"
    temp = temp_root / "tmp"
    home.mkdir(parents=True, exist_ok=True)
    temp.mkdir(parents=True, exist_ok=True)
    for directory in ("config", "cache", "data"):
        (temp_root / f"xdg-{directory}").mkdir(parents=True, exist_ok=True)
    path_parts = ([str(venv_bin)] if venv_bin is not None else []) + [
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ]
    env.update(
        {
            "HOME": str(home),
            "TMPDIR": str(temp),
            "PATH": os.pathsep.join(path_parts),
            "XDG_CONFIG_HOME": str(temp_root / "xdg-config"),
            "XDG_CACHE_HOME": str(temp_root / "xdg-cache"),
            "XDG_DATA_HOME": str(temp_root / "xdg-data"),
            "PIP_CONFIG_FILE": os.devnull,
            "PIP_DISABLE_PIP_VERSION_CHECK": "1",
        }
    )
    return env


def _run(
    command: list[str],
    *,
    env: dict[str, str],
    cwd: Path,
    check: str,
    timeout: int = 300,
) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise AuditError(check, f"could not run {Path(command[0]).name}: {error}") from error
    if result.returncode != 0:
        output = "\n".join(part.strip() for part in (result.stdout, result.stderr) if part.strip())
        raise AuditError(
            check,
            f"{Path(command[0]).name} exited {result.returncode}"
            + (f": {output[-4000:]}" if output else ""),
        )
    return result


def _required_tools() -> dict[str, str]:
    tools: dict[str, str] = {}
    for name in ("otool", "lipo", "delocate-listdeps"):
        path = shutil.which(name)
        if path is None:
            raise AuditError("inspection_tools", f"required inspection tool is missing: {name}")
        tools[name] = str(Path(path).resolve())
    return tools


def _wheel_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _wheel_entries(wheel: Path) -> tuple[list[tuple[str, bytes]], list[str]]:
    try:
        with ZipFile(wheel) as archive:
            corrupt_member = archive.testzip()
            if corrupt_member is not None:
                raise AuditError("wheel", f"wheel ZIP member failed CRC validation: {corrupt_member}")
            wheel_metadata = [
                name for name in archive.namelist() if name.endswith(".dist-info/WHEEL")
            ]
            if len(wheel_metadata) != 1:
                raise AuditError(
                    "wheel", f"expected one .dist-info/WHEEL file, found {len(wheel_metadata)}"
                )
            text = archive.read(wheel_metadata[0]).decode("utf-8", errors="strict")
            tags = [line[5:].strip() for line in text.splitlines() if line.startswith("Tag: ")]
            macho_members: list[tuple[str, bytes]] = []
            for item in archive.infolist():
                if item.is_dir():
                    continue
                with archive.open(item) as stream:
                    prefix = stream.read(4)
                    if prefix.startswith(MACHO_MAGICS):
                        macho_members.append((item.filename, prefix + stream.read()))
    except AuditError:
        raise
    except (OSError, BadZipFile, UnicodeDecodeError) as error:
        raise AuditError("wheel", f"could not read a valid wheel: {error}") from error
    if not tags:
        raise AuditError("wheel", "wheel metadata contains no Tag entries")
    if not macho_members:
        raise AuditError("wheel", "wheel contains no Mach-O files to inspect")
    return macho_members, tags


def _is_macho(data: bytes) -> bool:
    return data.startswith(MACHO_MAGICS)


def _wheel_tag_architectures(tags: list[str]) -> set[str]:
    architectures: set[str] = set()
    for tag in tags:
        parts = tag.split("-", 2)
        if len(parts) != 3:
            continue
        platform_tag = parts[2]
        match = re.fullmatch(r"macosx_\d+_\d+_(arm64|x86_64|universal2)", platform_tag)
        if match:
            architectures.add(match.group(1))
    return architectures


def _wheel_macos_versions(tags: list[str]) -> set[tuple[int, int]]:
    versions: set[tuple[int, int]] = set()
    for tag in tags:
        parts = tag.split("-", 2)
        if len(parts) != 3:
            continue
        match = re.fullmatch(r"macosx_(\d+)_(\d+)_(?:arm64|x86_64|universal2)", parts[2])
        if match:
            versions.add((int(match.group(1)), int(match.group(2))))
    return versions


def _temporary_root() -> tempfile.TemporaryDirectory[str]:
    temporary = tempfile.TemporaryDirectory(prefix="radiust-macos-release-")
    path = Path(temporary.name).resolve()
    try:
        path.relative_to(ROOT)
    except ValueError:
        return temporary
    temporary.cleanup()
    raise AuditError("temporary_environment", "temporary audit directory is inside the source tree")


def _site_packages(venv_python: Path, env: dict[str, str], cwd: Path) -> Path:
    code = "import sysconfig; print(sysconfig.get_paths()['purelib'])"
    result = _run([str(venv_python), "-c", code], env=env, cwd=cwd, check="venv")
    return Path(result.stdout.strip()).resolve()


def _installed_macho_files(site_packages: Path) -> list[Path]:
    results: list[Path] = []
    for directory, subdirectories, files in os.walk(site_packages, followlinks=False):
        subdirectories.sort()
        for filename in sorted(files):
            path = Path(directory) / filename
            if path.is_symlink() or not path.is_file():
                continue
            try:
                with path.open("rb") as stream:
                    if _is_macho(stream.read(4)):
                        results.append(path)
            except OSError as error:
                raise AuditError("installed_macho", f"could not inspect {path}: {error}") from error
    return results


def _inspect_macho(
    path: Path,
    label: str,
    *,
    tools: dict[str, str],
    env: dict[str, str],
    cwd: Path,
    expected_architecture: str,
) -> dict[str, Any]:
    arch_result = _run(
        [tools["lipo"], "-archs", str(path)],
        env=env,
        cwd=cwd,
        check="macho_architecture",
    )
    architectures = sorted(set(arch_result.stdout.split()))
    if expected_architecture not in architectures:
        raise AuditError(
            "macho_architecture",
            f"{label} has architectures {architectures!r}; expected {expected_architecture}",
        )

    load_result = _run(
        [tools["otool"], "-L", str(path)], env=env, cwd=cwd, check="macho_dependencies"
    )
    load_commands = _run(
        [tools["otool"], "-l", str(path)], env=env, cwd=cwd, check="macho_dependencies"
    )
    dependencies = sorted(
        match.group(1).strip()
        for line in load_result.stdout.splitlines()
        if (match := DEPENDENCY_RE.match(line))
    )
    rpaths = sorted(
        match.group(1).strip()
        for line in load_commands.stdout.splitlines()
        if (match := RPATH_RE.match(line))
    )
    combined = "\n".join((load_result.stdout, load_commands.stdout))
    if HOMEBREW_REFERENCE_RE.search(combined):
        raise AuditError(
            "homebrew_references", f"{label} contains a Homebrew dependency or rpath reference"
        )
    return {
        "path": label,
        "architectures": architectures,
        "dependencies": dependencies,
        "rpaths": rpaths,
    }


def _delocate_dependencies(
    target: Path,
    *,
    label: str,
    tools: dict[str, str],
    env: dict[str, str],
    cwd: Path,
) -> list[str]:
    result = _run(
        [tools["delocate-listdeps"], "--all", str(target)],
        env=env,
        cwd=cwd,
        check=f"delocate_{label}",
    )
    lines = sorted({line.strip() for line in result.stdout.splitlines() if line.strip()})
    if HOMEBREW_REFERENCE_RE.search("\n".join(lines)):
        raise AuditError(
            f"delocate_{label}", "delocate found a dependency under a developer Homebrew path"
        )
    return lines


def _installed_smoke(
    venv: Path,
    venv_python: Path,
    site_packages: Path,
    temp_root: Path,
) -> dict[str, Any]:
    cwd = temp_root / "work"
    cwd.mkdir(exist_ok=True)
    env = _clean_environment(temp_root, venv / "bin")
    radiust = venv / "bin" / "radiust"
    if not radiust.is_file() or not os.access(radiust, os.X_OK):
        raise AuditError("installed_cli", "wheel did not install an executable radiust command")
    help_result = _run([str(radiust), "--help"], env=env, cwd=cwd, check="installed_cli")
    listing = _run(
        [str(radiust), "list", "sources", "--json"],
        env=env,
        cwd=cwd,
        check="installed_cli",
    )
    try:
        list_report = json.loads(listing.stdout)
    except json.JSONDecodeError as error:
        raise AuditError("installed_cli", f"source-list command did not emit JSON: {error}") from error
    if not isinstance(list_report, dict) or list_report.get("command") != "list" or list_report.get("error") is not None:
        raise AuditError("installed_cli", "source-list command returned an unexpected JSON report")

    smoke_code = "\n".join(
        (
            "import importlib, json, radiust, sysconfig",
            "from pathlib import Path",
            "root = Path(sysconfig.get_paths()['purelib']).resolve()",
            "core = importlib.import_module('radiust._core')",
            "package_path = Path(radiust.__file__).resolve()",
            "core_path = Path(core.__file__).resolve()",
            "assert package_path.is_relative_to(root), package_path",
            "assert core_path.is_relative_to(root), core_path",
            "version = core.version()",
            "assert isinstance(version, str) and version",
            "print(json.dumps({'package_version': radiust.__version__, 'core_version': version}, sort_keys=True))",
        )
    )
    extension = _run(
        [str(venv_python), "-c", smoke_code],
        env=env,
        cwd=cwd,
        check="installed_python_extension",
    )
    try:
        extension_report = json.loads(extension.stdout)
    except json.JSONDecodeError as error:
        raise AuditError(
            "installed_python_extension", f"native extension smoke did not emit JSON: {error}"
        ) from error
    return {
        "cli_help_returncode": help_result.returncode,
        "cli_list_sources_returncode": listing.returncode,
        "cli_list_sources_report": list_report,
        "python_extension": extension_report,
    }


def run_audit(args: argparse.Namespace) -> dict[str, Any]:
    system, machine = platform.system(), platform.machine()
    report = new_report(
        args.wheel,
        args.expected_architecture,
        system,
        machine,
        args.expected_macos_version,
    )
    host_problem = host_error(system, machine, args.expected_architecture)
    if host_problem:
        record_check(report, "host", False, error=host_problem)
        return finish_report(report)
    record_check(report, "host", True, details={"system": system, "machine": machine})

    wheel = args.wheel.expanduser().resolve()
    if not wheel.is_file() or wheel.suffix != ".whl":
        message = f"wheel path must name an existing .whl file: {wheel}"
        record_check(report, "wheel", False, error=message)
        return finish_report(report)
    try:
        report["wheel"].update({"path": str(wheel), "sha256": _wheel_sha256(wheel)})
        members, tags = _wheel_entries(wheel)
        tag_architectures = _wheel_tag_architectures(tags)
        macos_versions = _wheel_macos_versions(tags)
        if args.expected_architecture not in tag_architectures and "universal2" not in tag_architectures:
            raise AuditError(
                "wheel",
                f"wheel tags {sorted(tags)!r} do not include expected macOS architecture "
                f"{args.expected_architecture}",
            )
        version_match = MACOS_VERSION_RE.fullmatch(args.expected_macos_version)
        if version_match is None:
            raise AuditError("wheel", "expected macOS version must use MAJOR.MINOR form")
        expected_version = (int(version_match.group(1)), int(version_match.group(2)))
        if expected_version not in macos_versions:
            raise AuditError(
                "wheel",
                f"wheel tags {sorted(tags)!r} do not include expected macOS deployment "
                f"version {args.expected_macos_version}",
            )
        record_check(
            report,
            "wheel",
            True,
            details={
                "macho_files": len(members),
                "tags": sorted(tags),
                "tag_architectures": sorted(tag_architectures),
                "macos_deployment_versions": [f"{major}.{minor}" for major, minor in sorted(macos_versions)],
            },
        )
    except AuditError as error:
        record_check(report, error.check, False, error=str(error))
        return finish_report(report)
    except (OSError, BadZipFile) as error:
        record_check(report, "wheel", False, error=f"could not read wheel: {error}")
        return finish_report(report)

    try:
        tools = _required_tools()
    except AuditError as error:
        record_check(report, error.check, False, error=str(error))
        return finish_report(report)
    record_check(report, "inspection_tools", True, details=tools)

    temporary: tempfile.TemporaryDirectory[str] | None = None
    try:
        temporary = _temporary_root()
        temp_root = Path(temporary.name).resolve()
        scratch = temp_root / "scratch"
        scratch.mkdir()
        cwd = temp_root / "work"
        cwd.mkdir()
        base_env = _clean_environment(temp_root)

        venv = temp_root / "venv"
        _run([sys.executable, "-m", "venv", str(venv)], env=base_env, cwd=cwd, check="venv")
        venv_python = venv / "bin" / "python"
        wheel_requirement = str(wheel)
        extra_requirements: list[str] = []
        if args.with_format_readback:
            wheel_requirement = f"radiust[science,geotiff,zarr] @ {wheel.as_uri()}"
            extra_requirements = ["pytest>=8,<9", "netCDF4>=1.6,<2"]
        index_args = [] if args.index_url is None else ["--index-url", args.index_url]
        install = _run(
            [
                str(venv_python),
                "-m",
                "pip",
                "--isolated",
                "install",
                "--disable-pip-version-check",
                "--no-cache-dir",
                "--timeout",
                "120",
                "--retries",
                "5",
                *index_args,
                (
                    "--only-binary=numpy,pyyaml,pillow,pyproj,rasterio,h5py,netcdf4,numcodecs"
                    if args.with_format_readback
                    else "--only-binary=:all:"
                ),
                wheel_requirement,
                *extra_requirements,
            ],
            env=_clean_environment(temp_root, venv / "bin"),
            cwd=cwd,
            check="wheel_install",
            timeout=600,
        )
        record_check(
            report,
            "wheel_install",
            True,
            details={"installer": "pip", "output": install.stdout.strip()[-2000:]},
        )

        site_packages = _site_packages(
            venv_python, _clean_environment(temp_root, venv / "bin"), cwd
        )
        try:
            site_packages.relative_to(venv.resolve())
        except ValueError as error:
            raise AuditError("venv", "site-packages resolved outside the temporary virtualenv") from error

        try:
            smoke = _installed_smoke(venv, venv_python, site_packages, temp_root)
            record_check(report, "installed_wheel_smoke", True, details=smoke)
        except AuditError as error:
            record_check(report, error.check, False, error=str(error))
            return finish_report(report)

        if args.with_format_readback:
            report["scope"]["format_readback"] = "performed_in_isolated_wheel_environment"
            readback_paths = [
                ROOT / "tests/contract/test_rust_png_readback.py",
                ROOT / "tests/contract/test_rust_netcdf_readback.py",
                ROOT / "tests/contract/test_rust_geotiff_readback.py",
                ROOT / "tests/contract/test_rust_zarr_readback.py",
            ]
            try:
                result = _run(
                    [
                        str(venv_python),
                        "-m",
                        "pytest",
                        "-q",
                        *(str(path) for path in readback_paths),
                    ],
                    env=_clean_environment(temp_root, venv / "bin"),
                    cwd=ROOT,
                    check="format_readback",
                    timeout=600,
                )
                record_check(
                    report,
                    "format_readback",
                    True,
                    details={
                        "tests": [path.name for path in readback_paths],
                        "output": result.stdout.strip()[-2000:],
                    },
                )
            except AuditError as error:
                record_check(report, error.check, False, error=str(error))

            proj_check = """import json
from pathlib import Path
import pyproj
directory = Path(pyproj.datadir.get_data_dir()).resolve()
database = directory / 'proj.db'
assert database.is_file(), database
print(json.dumps({'directory': str(directory), 'proj_db': str(database)}))
"""
            try:
                result = _run(
                    [str(venv_python), "-c", proj_check],
                    env=_clean_environment(temp_root, venv / "bin"),
                    cwd=cwd,
                    check="proj_data",
                )
                details = json.loads(result.stdout)
                if HOMEBREW_REFERENCE_RE.search(result.stdout):
                    raise AuditError("proj_data", "pyproj resolved proj.db through a Homebrew path")
                record_check(report, "proj_data", True, details=details)
            except (AuditError, json.JSONDecodeError) as error:
                check = error.check if isinstance(error, AuditError) else "proj_data"
                record_check(report, check, False, error=str(error))

        wheel_macho_dir = scratch / "wheel-macho"
        wheel_macho_dir.mkdir()
        wheel_records: list[dict[str, Any]] = []
        for member_name, member_data in members:
            flattened_name = hashlib.sha256(member_name.encode("utf-8")).hexdigest()
            extracted = wheel_macho_dir / flattened_name
            extracted.write_bytes(member_data)
            wheel_records.append(
                _inspect_macho(
                    extracted,
                    member_name,
                    tools=tools,
                    env=base_env,
                    cwd=cwd,
                    expected_architecture=args.expected_architecture,
                )
            )
        wheel_dependencies = _delocate_dependencies(
            wheel,
            label="wheel",
            tools=tools,
            env=base_env,
            cwd=cwd,
        )
        record_check(
            report,
            "wheel_macho_dependencies",
            True,
            details={"files": wheel_records, "delocate_dependencies": wheel_dependencies},
        )

        # Audit the wheel's installed package, not unrelated optional dependency
        # wheels such as Rasterio or pyproj installed for readback tests.
        installed_package = site_packages / "radiust"
        installed_files = _installed_macho_files(installed_package)
        if not installed_files:
            raise AuditError("installed_macho", "installed site-packages contains no Mach-O files")
        installed_records = [
            _inspect_macho(
                path,
                path.relative_to(site_packages).as_posix(),
                tools=tools,
                env=base_env,
                cwd=cwd,
                expected_architecture=args.expected_architecture,
            )
            for path in installed_files
        ]
        installed_dependencies = _delocate_dependencies(
            installed_package,
            label="installed",
            tools=tools,
            env=base_env,
            cwd=cwd,
        )
        record_check(
            report,
            "installed_macho_dependencies",
            True,
            details={"files": installed_records, "delocate_dependencies": installed_dependencies},
        )
    except AuditError as error:
        record_check(report, error.check, False, error=str(error))
    except (OSError, BadZipFile) as error:
        record_check(report, "audit", False, error=f"audit could not complete: {error}")
    finally:
        if temporary is not None:
            temporary.cleanup()
    return finish_report(report)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    report = run_audit(args)
    output = serialize_report(report)
    if args.report is not None:
        try:
            write_report(args.report, report)
        except OSError as error:
            print(f"could not write report {args.report}: {error}", file=sys.stderr)
            return 2
    sys.stdout.write(output)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
