from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest
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
from datetime import datetime, timezone
from importlib import resources
import importlib.util
import json
from pathlib import Path
import os

import h5py
import numpy as np
import radiust
from radiust import Client, Query
import radiust.registry as registry_module
from radiust.sources.base import FixtureSource
from radiust.sources.th import ThSource

assert set(ThSource.RADARS) == {"cmp1", "kkn240Loop"}
assert importlib.util.find_spec("radiust.sources._th_time") is not None

extra = os.environ["RADIUST_TEST_WHEEL_EXTRA"]
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
enabled = {"dateutil"} | (set(modules) if extra == "all" else {
    module for module, group in modules.items() if group == extra
})
for module in modules:
    assert (importlib.util.find_spec(module) is not None) == (module in enabled), (extra, module)

catalog_text = resources.files("radiust.resources").joinpath("catalog.json").read_text(encoding="utf-8")
catalog_data = json.loads(catalog_text)
catalog = catalog_data.get("sources", catalog_data)
assert len(catalog) == 24
resource_root = resources.files("radiust.resources")
source_resources = resource_root.joinpath("sources")
source_resource_ids = {
    item.name.removesuffix(".json")
    for item in source_resources.iterdir()
    if item.name.endswith(".json")
}
catalog_ids = {item["id"] for item in catalog}
assert catalog_ids <= source_resource_ids
assert {"br_cptec", "br_sipam"} <= source_resource_ids
th_resource = json.loads(source_resources.joinpath("th.json").read_text(encoding="utf-8"))
assert th_resource["time_semantics"] == "rendered_footer_ocr_utc"
assert th_resource["timestamp_extractor"] == "system_tesseract"
sg_palette = json.loads(
    resource_root.joinpath("palettes").joinpath("sg_rain_intensity.json").read_text(encoding="utf-8")
)
assert len(sg_palette["categories"]) == 5
assert sum(len(category["colors"]) for category in sg_palette["categories"]) == 33
assert sg_palette["numeric_rainfall_rate_available"] is False
pt_palette = json.loads(
    resource_root.joinpath("palettes").joinpath("pt_rain_intensity.json").read_text(encoding="utf-8")
)
assert pt_palette["id"] == "ipma-rain-intensity-ordinal-category"
assert pt_palette["value_kind"] == "ordinal_category"
assert pt_palette["numeric_rainfall_rate_available"] is False
assert [category["value"] for category in pt_palette["categories"]] == list(range(17))
assert radiust.__version__
assert radiust._core.version()

# Installed-wheel behavior tests explicitly inject the packaged offline
# fixture; the public registry always returns a provider adapter.
fixture_source = FixtureSource(
    registry_module.registry.get_info("my"),
    registry_module.SourceRegistry._fixture_path("my"),
)
original_get = registry_module.registry.get
registry_module.registry.get = lambda source_id: fixture_source if source_id == "my" else original_get(source_id)

query = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))
config = {"runtime": {"allow_network": False}, "cache": {"enabled": False}}
with Client(config=config) as client:
    field = client.fetch(query)
    report = client.download(query, output="netcdf")
    assert report.counts["written"] == 1
    nc_path = next(Path("netcdf").rglob("*.nc"))
    with h5py.File(nc_path, "r") as dataset:
        assert dataset["reflectivity"].shape == field.data.shape == (640, 826)
        assert dataset["reflectivity"].attrs["units"] == "dBZ"
        np.testing.assert_allclose(dataset["reflectivity"][()], field.data.values, equal_nan=True)
        assert dataset["quality"].dtype == np.dtype("uint16")
        np.testing.assert_array_equal(dataset["quality"][()], field.quality.values)
        assert dataset.attrs["Conventions"] == "CF-1.8"
    assert list(Path("netcdf").rglob("*.manifest.json"))

    if "geotiff" in enabled:
        import rasterio

        geo_report = client.download(query, output="geotiff", format="geotiff")
        assert geo_report.counts["written"] == 1
        rasters = list(Path("geotiff").rglob("*.tif"))
        data_path = next(path for path in rasters if not path.stem.endswith("_quality"))
        quality_path = next(path for path in rasters if path.stem.endswith("_quality"))
        expected = field.data.values
        if np.median(np.diff(field.grid.latitude)) > 0:
            expected = expected[::-1, :]
        with rasterio.open(data_path) as dataset:
            assert dataset.crs.to_epsg() == 4326
            assert dataset.dtypes == ("float32",)
            np.testing.assert_allclose(dataset.read(1), expected, equal_nan=True)
        with rasterio.open(quality_path) as dataset:
            assert dataset.dtypes == ("uint16",)
            np.testing.assert_array_equal(dataset.read(1), field.quality.values if np.median(np.diff(field.grid.latitude)) < 0 else field.quality.values[::-1, :])

    if "zarr" in enabled:
        import xarray as xr

        zarr_report = client.download(query, output="zarr", format="zarr")
        assert zarr_report.counts["written"] == 1
        store = next(path for path in Path("zarr").rglob("*.zarr") if path.is_dir())
        with xr.open_zarr(store, consolidated=True) as reopened:
            reopened.load()
            assert reopened["reflectivity"].shape == field.data.shape
            np.testing.assert_allclose(reopened["reflectivity"].values, field.data.values, equal_nan=True)
            assert reopened["quality"].dtype == np.dtype("uint16")
            np.testing.assert_array_equal(reopened["quality"].values, field.quality.values)
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
    wheel_dir = Path(__file__).parents[2] / "validation-results" / "wheel"
    supplied = os.environ.get("RADIUST_WHEEL")
    if supplied:
        wheels = [Path(supplied)]
    else:
        candidates = sorted(wheel_dir.glob("*.whl")) if wheel_dir.exists() else []
        wheels = _compatible_wheels(candidates)
    if not wheels:
        pytest.skip("clean-wheel CI supplies a compatible artifact for this source-tree-outside test")

    extra = os.environ.get("RADIUST_TEST_WHEEL_EXTRA", "core")
    if extra not in {*EXTRA_MODULES, "all"}:
        pytest.fail(f"unsupported wheel extra test selection: {extra}")

    wheel = wheels[-1].resolve()
    venv = tmp_path / "venv"
    venv_python = venv / "bin" / "python"
    subprocess.run(["uv", "venv", "--python", sys.executable, str(venv)], check=True, capture_output=True, text=True)
    requirement = f"radiust[{extra}] @ {wheel.as_uri()}" if extra != "core" else f"radiust @ {wheel.as_uri()}"
    install_command = ["uv", "pip", "install", "--python", str(venv_python)]
    constraints = os.environ.get("RADIUST_TEST_WHEEL_CONSTRAINTS")
    if constraints:
        install_command.extend(["--constraint", str(Path(constraints).resolve())])
    install_command.append(requirement)
    subprocess.run(
        install_command,
        check=True,
        capture_output=True,
        text=True,
    )

    env = {key: value for key, value in os.environ.items() if not key.startswith("RADIUST_")}
    env.pop("PYTHONPATH", None)
    env.pop("PYTHONHOME", None)
    env.pop("VIRTUAL_ENV", None)
    env["RADIUST_TEST_WHEEL_EXTRA"] = extra
    smoke = subprocess.run(
        [str(venv_python), "-c", SMOKE_SCRIPT],
        capture_output=True,
        text=True,
        cwd=tmp_path,
        env=env,
    )
    assert smoke.returncode == 0, f"installed wheel smoke failed:\n{smoke.stdout}\n{smoke.stderr}"
