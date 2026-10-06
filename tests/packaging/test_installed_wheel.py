from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from email.parser import BytesParser
from email.policy import default
from pathlib import Path
from zipfile import ZipFile

import pytest
from packaging.requirements import Requirement
from packaging.tags import sys_tags
from packaging.utils import InvalidWheelFilename, parse_wheel_filename

EXTRA_MODULES = {
    "core": set(),
    "geotiff": {"rasterio"},
    "zarr": {"zarr", "numcodecs"},
    "playwright": {"playwright"},
    "recovery": {"scipy", "cv2"},
    "scraping": {"bs4", "brotli", "dateutil"},
    "storage": set(),
}
EXTRA_MODULES["all"] = set().union(*EXTRA_MODULES.values())

SMOKE_SCRIPT = r'''
import importlib.util
import asyncio
import json
import math
import os
import struct
import subprocess
import sys
from pathlib import Path

extra = os.environ["RADIUST_TEST_WHEEL_EXTRA"]
mode = os.environ["RADIUST_WHEEL_SMOKE_MODE"]
modules = {
    "rasterio": "geotiff",
    "zarr": "zarr",
    "numcodecs": "zarr",
    "playwright": "playwright",
    "scipy": "recovery",
    "cv2": "recovery",
    "bs4": "scraping",
    "brotli": "scraping",
    "dateutil": "scraping",
}
enabled = set() if extra == "core" else (set(modules) if extra == "all" else {
    module for module, group in modules.items() if group == extra
})
if mode == "core":
    assert importlib.util.find_spec("numpy") is None, "base wheel unexpectedly installed NumPy"
    assert importlib.util.find_spec("xarray") is None, "base wheel unexpectedly installed xarray"
elif mode == "science":
    assert importlib.util.find_spec("numpy") is not None, "science extra did not install NumPy"
    assert importlib.util.find_spec("xarray") is not None, "science extra did not install xarray"
for module in modules:
    if module == "dateutil" and (extra == "core" or mode == "science"):
        continue  # PyYAML or xarray may pull it in transitively.
    assert (importlib.util.find_spec(module) is not None) == (module in enabled), (extra, module)

import radiust
from radiust import Query, RadarDataset, RadarField, to_xarray
from radiust.models import DiscoveryItem, DiscoveryTarget

assert radiust.__version__
assert radiust._core.version()
assert callable(radiust.fetch)
assert callable(radiust.Client)
assert RadarField is radiust._core.RadarField
assert RadarDataset is radiust._core.RadarDataset
assert Query("fr", latest=True).source == "fr"
assert importlib.util.find_spec("radiust.safety") is not None
discovery_item = DiscoveryItem(
    DiscoveryTarget("ph", "composite", None),
    "missing_credentials",
    error={"message": "token=wheel-secret"},
)
assert discovery_item.as_dict()["error"]["message"] == "token=[REDACTED]"
assert importlib.util.find_spec("radiust.registry") is not None, "Rust-backed SDK catalog facade is missing"
assert importlib.util.find_spec("radiust.sources") is None, "legacy Python source adapters leaked into the wheel"
from radiust.registry import sources

source_catalog = sources()
assert len(source_catalog) == 25, "SDK catalog facade diverged from the Rust source catalog"
rdcap = next(source for source in source_catalog if source.id == "rdcap")
assert rdcap.metadata["country_capabilities"] == {
    country: {
        "discovery": "unverified",
        "raw_acquisition": "unverified",
        "science": "unverified",
        "readback": "unverified",
    }
    for country in ("TWN", "JPN", "PHL")
}
assert rdcap.metadata["last_live_validation_attempt"]["status"] == "blocked_before_raw_acquisition"
for module in (
    "radiust.batch",
    "radiust.cache",
    "radiust.discovery",
    "radiust.display",
    "radiust.field",
    "radiust.outputs.base",
    "radiust.outputs.geotiff",
    "radiust.outputs.netcdf",
    "radiust.outputs.png",
    "radiust.outputs.registry",
    "radiust.pipeline",
    "radiust.raw",
    "radiust.raw_replay",
    "radiust.rendering",
    "radiust.storage",
    "radiust.terminal",
    "radiust.transport",
    "radiust.cli.cat",
    "radiust.cli.cache",
    "radiust.cli.doctor",
    "radiust.cli.query",
    "radiust.cli.progress",
    "radiust.cli.safety",
    "radiust.cli.layout",
    "radiust.cli.reporting",
):
    assert importlib.util.find_spec(module) is None, f"legacy Python runtime module leaked: {module}"
assert importlib.util.find_spec("radiust.decoders") is not None
assert importlib.util.find_spec("radiust.decoders.gray_dbz") is not None
assert importlib.util.find_spec("radiust.decoders.exact") is None
assert importlib.util.find_spec("radiust.decoders.nearest") is None
from radiust.decoders import GrayDbzDecoder, LegacyGrayDbzDecoder

assert callable(GrayDbzDecoder) and callable(LegacyGrayDbzDecoder)
if mode == "science":
    import numpy as np

    values, quality = GrayDbzDecoder().decode(np.asarray([[0, 224]], dtype=np.uint8))
    assert values.tolist() == [[0.0, 70.0]]
    assert quality.shape == values.shape
assert importlib.util.find_spec("radiust.outputs.zarr") is not None
from radiust.outputs.zarr import write_zarr

assert callable(write_zarr)

field_document = {
    "name": "reflectivity",
    "values": [1.5, 2.5, 3.5, 4.5],
    "shape": [2, 2],
    "quality": [0, 1, 2, 3],
    "units": "dBZ",
    "valid_time": "2026-09-24T00:00:00Z",
    "grid": {
        "shape": [2, 2],
        "crs": "EPSG:4326",
        "x": [100.0, 101.0],
        "y": [20.0, 21.0],
        "affine": None,
    },
    "provenance": ["wheel-smoke"],
}
field = radiust._core.RadarField(json.dumps(field_document))
assert field.name == "reflectivity"
assert list(struct.unpack("<4f", field.values_le_bytes())) == field_document["values"]
assert list(struct.unpack("<4H", field.quality_le_bytes())) == field_document["quality"]
assert json.loads(field.metadata_json())["valid_time"] == field_document["valid_time"]

radius = 6_378_137.0
def project_x(longitude):
    return radius * math.radians(longitude)
def project_y(latitude):
    radians = math.radians(latitude)
    return radius * math.log(math.tan(math.pi / 4.0 + radians / 2.0))

geographic_field_json = json.dumps({
    "name": "reflectivity",
    "values": [10.0, 20.0, 30.0, 40.0],
    "shape": [2, 2],
    "quality": [4, 8, 16, 32],
    "units": "dBZ",
    "valid_time": "2026-09-24T00:00:00Z",
    "grid": {"shape": [2, 2], "crs": "EPSG:4326", "x": [0.0, 1.0], "y": [0.0, 1.0], "affine": None},
    "provenance": ["wheel-regrid-smoke"],
})
web_mercator_field_json = json.dumps({
    "name": "reflectivity",
    "values": [10.0, 20.0, 30.0, 40.0],
    "shape": [2, 2],
    "quality": [4, 8, 16, 32],
    "units": "dBZ",
    "valid_time": "2026-09-24T00:00:00Z",
    "grid": {
        "shape": [2, 2],
        "crs": "EPSG:3857",
        "x": [project_x(0.0), project_x(1.0)],
        "y": [project_y(0.0), project_y(1.0)],
        "affine": None,
    },
    "provenance": ["wheel-regrid-smoke"],
})
web_mercator_target = {
    "shape": [1, 1],
    "crs": "EPSG:3857",
    "x": [project_x(0.25)],
    "y": [project_y(0.25)],
    "affine": None,
}
geographic_target = {
    "shape": [1, 1],
    "crs": "EPSG:4326",
    "x": [0.25],
    "y": [0.25],
    "affine": None,
}
async def regrid_smoke(field_json, target):
    engine = radiust._core.Engine(json.dumps({"runtime": {"allow_network": False}}))
    field = radiust._core.RadarField(field_json)
    return await engine.regrid(field, json.dumps(target), "nearest")

for field_json, target, source_crs, target_crs in (
    (geographic_field_json, web_mercator_target, "EPSG:4326", "EPSG:3857"),
    (web_mercator_field_json, geographic_target, "EPSG:3857", "EPSG:4326"),
):
    regridded = asyncio.run(regrid_smoke(field_json, target))
    assert list(struct.unpack("<f", regridded.values_le_bytes())) == [10.0]
    assert list(struct.unpack("<H", regridded.quality_le_bytes())) == [4]
    metadata = json.loads(regridded.metadata_json())
    assert metadata["grid"]["crs"] == target_crs
    assert (
        f"operation=crs_transform,method=web_mercator,source_crs={source_crs},target_crs={target_crs}"
        in metadata["provenance"]
    )

fixture_root = Path(os.environ["RADIUST_TEST_FIXTURE_ROOT"])
installed_cli = Path(sys.executable).parent / "radiust"
cli_env = {key: value for key, value in os.environ.items() if not key.startswith("RADIUST_")}
cli_env["RADIUST_RUNTIME__ALLOW_NETWORK"] = "false"
cat_cases = (
    (
        fixture_root / "tests/fixtures/rust-migration/output/geotiff/field.tif",
        (),
        "format=GeoTIFF variable=reflectivity",
    ),
    (
        fixture_root / "tests/fixtures/rust-migration/output/zarr/field.zarr",
        ("--variable", "reflectivity"),
        "format=Zarr v2 variable=reflectivity",
    ),
)
for data_path, selectors, expected_summary in cat_cases:
    preview = subprocess.run(
        [str(installed_cli), "cat", "--file", str(data_path), *selectors, "--renderer", "text"],
        check=False,
        capture_output=True,
        text=True,
        env=cli_env,
    )
    assert preview.returncode == 0, (data_path, preview.stderr)
    assert expected_summary in preview.stdout, (data_path, preview.stdout)

batch_frame = radiust._core.FrameRef(json.dumps({
    "source": "tw",
    "product": "grid",
    "station": "CV1_3600",
    "valid_time": "2026-09-24T00:00:00Z",
    "logical_id": "wheel-batch-smoke",
}))
async def run_batch_smoke():
    engine = radiust._core.Engine(json.dumps({"runtime": {"allow_network": False}}))
    return await engine.fetch_many_decoded([batch_frame], "collect", True, None)

batch_report = asyncio.run(run_batch_smoke())
assert batch_report.total == batch_report.planned == 1
assert batch_report.item(0).status == "planned"
assert batch_report.item(0).data() is None

if mode == "science":
    import numpy as np
    import xarray as xr

    converted = to_xarray(field)
    assert isinstance(converted, xr.DataArray)
    assert converted.dtype == np.float32
    assert converted.attrs["units"] == "dBZ"
    assert converted.attrs["valid_time"] == field_document["valid_time"]
    assert converted.coords["quality"].dtype == np.uint16
    np.testing.assert_array_equal(converted.values, [[1.5, 2.5], [3.5, 4.5]])
    np.testing.assert_array_equal(converted.coords["quality"].values, [[0, 1], [2, 3]])
    np.testing.assert_array_equal(converted.coords["x"].values, [100.0, 101.0])
    np.testing.assert_array_equal(converted.coords["y"].values, [20.0, 21.0])
'''

OPTIONAL_SCIENCE_SMOKE_SCRIPT = r'''
import json
import tempfile
from importlib import resources
from pathlib import Path

import numpy as np
import xarray as xr
from radiust import _core
from radiust.outputs.zarr import write_zarr

field = _core.RadarField(json.dumps({
    "name": "reflectivity",
    "values": [1.5, 2.5, 3.5, 4.5],
    "shape": [2, 2],
    "quality": [0, 1, 2, 3],
    "units": "dBZ",
    "valid_time": "2026-09-24T00:00:00Z",
    "grid": {
        "shape": [2, 2],
        "crs": "EPSG:4326",
        "x": [100.0, 101.0],
        "y": [20.0, 21.0],
        "affine": None,
    },
    "provenance": ["wheel-zarr-smoke"],
}))
with tempfile.TemporaryDirectory() as root:
    output = Path(root) / "field.zarr"
    write_zarr(field, output)
    reopened = xr.open_zarr(output, consolidated=True).load()
    np.testing.assert_array_equal(reopened.reflectivity.values, [[1.5, 2.5], [3.5, 4.5]])
    assert reopened.reflectivity.coords["quality"].dtype == np.uint16
    np.testing.assert_array_equal(reopened.reflectivity.coords["quality"].values, [[0, 1], [2, 3]])

resource_root = resources.files("radiust.resources")
catalog_data = json.loads(resource_root.joinpath("catalog.json").read_text(encoding="utf-8"))
catalog = catalog_data.get("sources", catalog_data)
assert len(catalog) == 25
source_resources = resource_root.joinpath("sources")
source_resource_ids = {
    item.name.removesuffix(".json")
    for item in source_resources.iterdir()
    if item.name.endswith(".json")
}
assert {item["id"] for item in catalog if item["id"] != "rdcap"} <= source_resource_ids
palette = json.loads(
    resource_root.joinpath("palettes", "rdcap_reflectivity.json").read_text(encoding="utf-8")
)
assert palette["id"] == "rdcap-reflectivity-v1"
assert len(palette["classes"]) == 15
'''


def _compatible_wheels(candidates: list[Path]) -> list[Path]:
    supported = set(sys_tags())
    compatible: list[Path] = []
    for candidate in candidates:
        try:
            _name, _version, _build, tags = parse_wheel_filename(candidate.name)
        except InvalidWheelFilename:
            continue
        if supported.intersection(tags):
            compatible.append(candidate)
    return compatible


def _xarray_extras(wheel: Path) -> set[str]:
    with ZipFile(wheel) as archive:
        metadata_paths = [name for name in archive.namelist() if name.endswith(".dist-info/METADATA")]
        if len(metadata_paths) != 1:
            pytest.fail(f"expected one wheel METADATA file, found {len(metadata_paths)}")
        metadata = BytesParser(policy=default).parsebytes(archive.read(metadata_paths[0]))

    extras = set(metadata.get_all("Provides-Extra", []))
    result: set[str] = set()
    for line in metadata.get_all("Requires-Dist", []):
        requirement = Requirement(line)
        if requirement.name.lower() != "xarray" or requirement.marker is None:
            continue
        for extra in extras:
            if requirement.marker.evaluate({"extra": extra}):
                result.add(extra)
    return result


def _install_extras(wheel: Path, requested: str) -> list[str]:
    if requested == "core":
        return []
    if requested not in {"zarr", "all"}:
        return [requested]

    xarray_extras = _xarray_extras(wheel)
    if requested == "zarr" and not xarray_extras:
        pytest.fail("wheel does not declare xarray through an optional extra")
    if not xarray_extras:
        return [requested]
    science_extra = next(
        (name for name in ("science", "zarr") if name in xarray_extras),
        sorted(xarray_extras)[0],
    )
    extras = [requested]
    if science_extra not in extras:
        extras.append(science_extra)
    return extras


def _clean_environment(*, extra: str | None = None, mode: str | None = None) -> dict[str, str]:
    env = {key: value for key, value in os.environ.items() if not key.startswith("RADIUST_")}
    for key in ("PYTHONPATH", "PYTHONHOME", "VIRTUAL_ENV"):
        env.pop(key, None)
    if extra is not None:
        env["RADIUST_TEST_WHEEL_EXTRA"] = extra
    if mode is not None:
        env["RADIUST_WHEEL_SMOKE_MODE"] = mode
    return env


def _create_venv_and_install(tmp_path: Path, wheel: Path, extras: list[str]) -> tuple[Path, Path]:
    venv = tmp_path / "venv"
    venv_python = venv / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    env = _clean_environment()
    uv = shutil.which("uv")
    if uv:
        subprocess.run(
            [uv, "venv", "--python", sys.executable, str(venv)],
            check=True,
            capture_output=True,
            text=True,
            env=env,
        )
    else:
        subprocess.run(
            [sys.executable, "-m", "venv", str(venv)],
            check=True,
            capture_output=True,
            text=True,
            env=env,
        )
        pip_check = subprocess.run(
            [str(venv_python), "-m", "pip", "--version"],
            capture_output=True,
            text=True,
            env=env,
        )
        if pip_check.returncode != 0:
            pytest.skip("wheel smoke needs uv or a venv with pip available")

    requirement = f"radiust[{','.join(extras)}] @ {wheel.as_uri()}" if extras else f"radiust @ {wheel.as_uri()}"
    constraints = os.environ.get("RADIUST_TEST_WHEEL_CONSTRAINTS")
    if uv:
        install_command = [uv, "pip", "install", "--python", str(venv_python)]
    else:
        install_command = [str(venv_python), "-m", "pip", "install"]
    if constraints:
        install_command.extend(["--constraint", str(Path(constraints).resolve())])
    install_command.append(requirement)
    subprocess.run(install_command, check=True, capture_output=True, text=True, env=env)
    return venv, venv_python


def _native_cli(root: Path) -> Path | None:
    configured = os.environ.get("RADIUST_NATIVE_CLI")
    if configured:
        candidate = Path(configured).expanduser().resolve()
        if not candidate.is_file() or not os.access(candidate, os.X_OK):
            pytest.fail(f"RADIUST_NATIVE_CLI is not an executable file: {candidate}")
        return candidate
    for candidate in (root / "target" / "release" / "radiust", root / "target" / "debug" / "radiust"):
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate
    return None


def _assert_console_cli_matches_native(venv: Path, tmp_path: Path, root: Path) -> None:
    scripts = venv / ("Scripts" if os.name == "nt" else "bin")
    console = scripts / ("radiust.exe" if os.name == "nt" else "radiust")
    assert console.is_file(), f"wheel did not install its radiust console command at {console}"
    native = _native_cli(root)
    env = _clean_environment()
    installed_cat_help = subprocess.run(
        [str(console), "cat", "--help"], cwd=tmp_path, env=env, capture_output=True, text=True
    )
    assert installed_cat_help.returncode == 0, installed_cat_help.stderr
    assert "--legacy-display" in installed_cat_help.stdout
    installed_replay_help = subprocess.run(
        [str(console), "replay", "--help"], cwd=tmp_path, env=env, capture_output=True, text=True
    )
    assert installed_replay_help.returncode == 0, installed_replay_help.stderr
    assert "raw manifest" in installed_replay_help.stdout.lower()
    if native is not None:
        native_cat_help = subprocess.run(
            [str(native), "cat", "--help"], cwd=tmp_path, env=env, capture_output=True, text=True
        )
        assert native_cat_help.returncode == installed_cat_help.returncode
        assert native_cat_help.stdout == installed_cat_help.stdout
        assert native_cat_help.stderr == installed_cat_help.stderr
        native_replay_help = subprocess.run(
            [str(native), "replay", "--help"], cwd=tmp_path, env=env, capture_output=True, text=True
        )
        assert native_replay_help.returncode == installed_replay_help.returncode
        assert native_replay_help.stdout == installed_replay_help.stdout
        assert native_replay_help.stderr == installed_replay_help.stderr
    commands = (
        (["list", "sources", "--json"], 0),
        (["list", "sources", "--json", "--wheel-smoke-invalid-option"], 2),
    )
    for arguments, expected_code in commands:
        installed = subprocess.run(
            [str(console), *arguments], cwd=tmp_path, env=env, capture_output=True, text=True
        )
        assert installed.returncode == expected_code, (
            f"wheel console command {arguments!r} returned {installed.returncode}: "
            f"{installed.stdout}\n{installed.stderr}"
        )
        try:
            installed_json = json.loads(installed.stdout)
        except json.JSONDecodeError as error:
            pytest.fail(
                f"wheel console command {arguments!r} did not emit JSON: "
                f"{installed.stdout!r}; stderr={installed.stderr!r} ({error})"
            )
        assert installed.stderr == ""
        if expected_code == 0:
            assert installed_json["command"] == "list"
            assert installed_json["error"] is None
        else:
            assert installed_json["error"] is not None

        if native is None:
            continue
        reference = subprocess.run(
            [str(native), *arguments], cwd=tmp_path, env=env, capture_output=True, text=True
        )
        assert reference.returncode == installed.returncode
        try:
            reference_json = json.loads(reference.stdout)
        except json.JSONDecodeError as error:
            pytest.fail(
                f"native CLI command {arguments!r} did not emit JSON: "
                f"{reference.stdout!r}; stderr={reference.stderr!r} ({error})"
            )
        assert reference_json == installed_json
        assert reference.stderr == installed.stderr


def test_wheel_candidate_filter_ignores_incompatible_platform_artifacts(tmp_path: Path, monkeypatch) -> None:
    import packaging.tags

    compatible = tmp_path / "radiust-0.1.0-cp312-cp312-manylinux_2_17_aarch64.whl"
    incompatible = tmp_path / "radiust-0.1.0-cp312-cp312-macosx_11_0_arm64.whl"
    compatible.touch()
    incompatible.touch()
    supported = packaging.tags.parse_tag("cp312-cp312-manylinux_2_17_aarch64")
    monkeypatch.setattr(sys.modules[__name__], "sys_tags", lambda: supported)

    assert _compatible_wheels([incompatible, compatible]) == [compatible]


def test_installed_wheel_smoke_when_wheel_is_supplied(tmp_path: Path) -> None:
    root = Path(__file__).parents[2]
    wheel_dir = root / "validation-results" / "wheel"
    supplied = os.environ.get("PYTEST_RADIUST_WHEEL") or os.environ.get("RADIUST_WHEEL")
    if supplied:
        wheels = [Path(supplied)]
    else:
        candidates = (
            sorted(wheel_dir.rglob("*.whl"), key=lambda candidate: candidate.stat().st_mtime_ns)
            if wheel_dir.exists()
            else []
        )
        wheels = _compatible_wheels(candidates)
    if not wheels:
        pytest.skip("clean-wheel CI supplies a compatible artifact for this source-tree-outside test")

    extra = os.environ.get("RADIUST_TEST_WHEEL_EXTRA", "core")
    if extra not in {*EXTRA_MODULES, "all"}:
        pytest.fail(f"unsupported wheel extra test selection: {extra}")

    wheel = wheels[-1].resolve()
    extras = _install_extras(wheel, extra)
    mode = "core" if extra == "core" else "science" if extra == "zarr" else "extra"
    runtime_extra = "all" if "all" in extras else extra
    venv, venv_python = _create_venv_and_install(tmp_path, wheel, extras)
    env = _clean_environment(extra=runtime_extra, mode=mode)
    env["RADIUST_TEST_FIXTURE_ROOT"] = str(root)
    smoke = subprocess.run(
        [str(venv_python), "-c", SMOKE_SCRIPT],
        capture_output=True,
        text=True,
        cwd=tmp_path,
        env=env,
    )
    assert smoke.returncode == 0, f"installed wheel smoke failed:\n{smoke.stdout}\n{smoke.stderr}"
    if mode == "science" or "all" in extras:
        science_smoke = subprocess.run(
            [str(venv_python), "-c", OPTIONAL_SCIENCE_SMOKE_SCRIPT],
            capture_output=True,
            text=True,
            cwd=tmp_path,
            env=env,
        )
        assert science_smoke.returncode == 0, (
            f"optional-science wheel smoke failed:\n{science_smoke.stdout}\n{science_smoke.stderr}"
        )
    _assert_console_cli_matches_native(venv, tmp_path, root)
