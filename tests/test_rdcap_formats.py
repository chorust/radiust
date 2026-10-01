"""Independent Python readback of the Rust RDCAP output contract."""

from __future__ import annotations

import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pytest
import rasterio
import xarray as xr
from PIL import Image

REPOSITORY_ROOT = Path(__file__).resolve().parents[1]
FIXTURE_ROOT = REPOSITORY_ROOT / "tests/fixtures/sources/rdcap"
MANIFEST_PATH = FIXTURE_ROOT / "manifest.json"
QUALITY_FLAG_MASKS = [1, 2, 4, 8, 16, 32, 64]
QUALITY_FLAG_MEANINGS = (
    "missing outside_coverage unknown_color recovered interpolated "
    "below_detection source_annotation"
)


def _fixture_path(file_record: dict[str, object]) -> Path:
    path = REPOSITORY_ROOT / str(file_record["path"])
    content = path.read_bytes()
    assert len(content) == file_record["bytes"], f"fixture size changed: {path}"
    assert hashlib.sha256(content).hexdigest() == file_record["sha256"], (
        f"fixture digest changed: {path}"
    )
    return path


def _expected_axes(grid: dict[str, object]) -> tuple[np.ndarray, np.ndarray]:
    width = int(grid["width"])
    height = int(grid["height"])
    west, pixel_width, rotation_x, north, rotation_y, pixel_height = grid["geotransform"]
    assert rotation_x == 0 and rotation_y == 0
    longitude = west + (np.arange(width) + 0.5) * pixel_width
    latitude = north + (np.arange(height) + 0.5) * pixel_height
    return longitude, latitude


def _assert_time(value: object, timestamp_ms: int) -> None:
    actual = np.asarray(value).astype("datetime64[ms]")
    np.testing.assert_equal(actual, np.datetime64(timestamp_ms, "ms"))


def _assert_iso_time(value: str, timestamp_ms: int) -> None:
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    assert int(parsed.timestamp() * 1000) == timestamp_ms


def _assert_flag_metadata(
    attributes: dict[str, object],
    *,
    masks_key: str = "flag_masks",
    meanings_key: str = "flag_meanings",
) -> None:
    np.testing.assert_array_equal(attributes[masks_key], QUALITY_FLAG_MASKS)
    assert attributes[meanings_key] == QUALITY_FLAG_MEANINGS


def _assert_wgs84_geographic_crs(crs: rasterio.crs.CRS) -> None:
    assert crs.is_geographic
    wkt = crs.to_wkt()
    assert "World Geodetic System 1984" in wkt
    assert 'UNIT["degree"' in wkt


def _assert_science_arrays(
    values: np.ndarray,
    quality: np.ndarray,
    grid: dict[str, object],
    label: str,
) -> None:
    expected_shape = (int(grid["height"]), int(grid["width"]))
    assert values.shape == expected_shape, f"{label} value shape"
    assert quality.shape == expected_shape, f"{label} quality shape"
    assert np.isin(quality, [0, 1, 65]).all(), f"{label} unexpected quality bits"

    annotation = quality == 65
    echo = quality == 0
    missing = quality == 1
    assert int(annotation.sum()) == grid["annotation_cells"], f"{label} annotation count"
    assert int(echo.sum()) == grid["echo_cells"], f"{label} echo count"
    assert int((~missing).sum()) == grid["valid_cells"], f"{label} valid count"
    assert int(missing.sum()) == expected_shape[0] * expected_shape[1] - grid["valid_cells"], (
        f"{label} missing count"
    )
    assert np.isnan(values[annotation]).all(), f"{label} annotation pixels became data"
    assert np.isnan(values[missing]).all(), f"{label} missing pixels became data"
    assert np.isfinite(values[echo]).all(), f"{label} echo pixels must be finite"

    weak_values = values[np.isfinite(values) & (values < 5.0)]
    assert weak_values.size > 0, f"{label} lost below-legend weak reflectivity values"
    assert np.isfinite(values).sum() == grid["echo_cells"], f"{label} finite value count"


def _assert_rchl_comparison(
    values: np.ndarray,
    quality: np.ndarray,
    grid: dict[str, object],
    comparison: dict[str, object],
) -> None:
    longitude, latitude = _expected_axes(grid)
    assert comparison["timestamp_ms"] == grid["timestamp_ms"]
    for point in comparison["points"]:
        column = int(np.argmin(np.abs(longitude - point["lng"])))
        row = int(np.argmin(np.abs(latitude - point["lat"])))
        assert abs(longitude[column] - point["lng"]) <= abs(grid["geotransform"][1]) / 2 + 1e-8
        assert abs(latitude[row] - point["lat"]) <= abs(grid["geotransform"][5]) / 2 + 1e-8

        expected = point["expected"]
        actual = values[row, column]
        if expected is None:
            assert np.isnan(actual), f"comparison point {point} should be missing"
            assert quality[row, column] == 1
        else:
            assert actual == pytest.approx(expected, abs=1e-5), f"comparison point {point} changed"
            assert quality[row, column] == 0


def _assert_rgba(
    rgba: np.ndarray,
    values: np.ndarray,
    quality: np.ndarray,
    grid_fixture: dict[str, object],
) -> None:
    expected_shape = (int(grid_fixture["height"]), int(grid_fixture["width"]))
    assert rgba.shape == (*expected_shape, 4)
    drawable = (quality == 0) & np.isfinite(values) & (values >= 5.0)
    np.testing.assert_array_equal(
        rgba[~drawable], np.zeros((int((~drawable).sum()), 4), dtype=np.uint8)
    )
    assert np.all(rgba[drawable, 3] == 255)

    thresholds = np.asarray(grid_fixture["thresholds_dbz"], dtype=np.float32)
    colors = np.asarray(
        [
            [int(color[index : index + 2], 16) for index in (1, 3, 5)]
            for color in grid_fixture["colors"]
        ],
        dtype=np.uint8,
    )
    class_indexes = np.searchsorted(thresholds, values[drawable], side="right") - 1
    class_indexes = np.clip(class_indexes, 0, len(colors) - 1)
    np.testing.assert_array_equal(rgba[drawable, :3], colors[class_indexes])


def _read_and_check_station(output_root: Path, station: dict[str, object]) -> None:
    station_id = str(station["station_id"])
    output = output_root / station_id.replace("/", "-")
    records = station["files"]
    _fixture_path(records["frame"])
    grid_path = _fixture_path(records["grid"])
    grid = json.loads(grid_path.read_text(encoding="utf-8"))
    timestamp_ms = int(station["selected_key_epoch_ms"])
    assert grid["timestamp_ms"] == timestamp_ms
    expected_longitude, expected_latitude = _expected_axes(grid)

    with xr.open_dataset(output / "reflectivity.nc", engine="h5netcdf") as dataset:
        netcdf_values = dataset["reflectivity"].values.copy()
        netcdf_quality = dataset["quality"].values.copy()
        assert dataset["reflectivity"].attrs["units"] == "dBZ"
        assert dataset["crs"].attrs["spatial_ref"] == grid["crs"]
        np.testing.assert_allclose(
            dataset["longitude"].values, expected_longitude, rtol=0, atol=1e-9
        )
        np.testing.assert_allclose(dataset["latitude"].values, expected_latitude, rtol=0, atol=1e-9)
        _assert_time(dataset["time"].values, timestamp_ms)
        assert dataset.attrs["radiust_valid_time"].startswith(
            datetime.fromtimestamp(timestamp_ms / 1000, tz=timezone.utc).strftime(
                "%Y-%m-%dT%H:%M:%S"
            )
        )
        _assert_flag_metadata(dataset["quality"].attrs)

    _assert_science_arrays(netcdf_values, netcdf_quality, grid, f"{station_id} NetCDF")

    with rasterio.open(output / "reflectivity.tif") as raster:
        geotiff_values = raster.read(1)
        assert raster.shape == netcdf_values.shape
        _assert_wgs84_geographic_crs(raster.crs)
        west, pixel_width, rotation_x, north, rotation_y, pixel_height = grid["geotransform"]
        np.testing.assert_allclose(
            tuple(raster.transform)[:6],
            (pixel_width, rotation_x, west, rotation_y, pixel_height, north),
            rtol=0,
            atol=1e-9,
        )
        raster_transform = raster.transform
    with rasterio.open(output / "reflectivity_quality.tif") as raster:
        geotiff_quality = raster.read(1)
        assert raster.transform == raster_transform
        _assert_wgs84_geographic_crs(raster.crs)
    geotiff_metadata = json.loads(
        (output / "reflectivity_provenance.json").read_text(encoding="utf-8")
    )
    assert geotiff_metadata["source_crs"] == grid["crs"]
    assert geotiff_metadata["crs"] == grid["crs"]
    _assert_iso_time(geotiff_metadata["valid_time"], timestamp_ms)
    _assert_flag_metadata(
        geotiff_metadata,
        masks_key="quality_flag_masks",
        meanings_key="quality_flag_meanings",
    )
    _assert_science_arrays(geotiff_values, geotiff_quality, grid, f"{station_id} GeoTIFF")

    with xr.open_zarr(output / "reflectivity.zarr", consolidated=True) as dataset:
        zarr_values = dataset["reflectivity"].values.copy()
        zarr_quality = dataset["quality"].values.copy()
        np.testing.assert_allclose(
            dataset["longitude"].values, expected_longitude, rtol=0, atol=1e-9
        )
        np.testing.assert_allclose(dataset["latitude"].values, expected_latitude, rtol=0, atol=1e-9)
        _assert_time(dataset["time"].values, timestamp_ms)
        assert dataset["crs"].attrs["spatial_ref"] == grid["crs"]
        assert dataset["reflectivity"].attrs["units"] == "dBZ"
        _assert_flag_metadata(dataset["quality"].attrs)
    _assert_science_arrays(zarr_values, zarr_quality, grid, f"{station_id} Zarr")

    np.testing.assert_array_equal(geotiff_quality, netcdf_quality)
    np.testing.assert_array_equal(zarr_quality, netcdf_quality)
    np.testing.assert_allclose(geotiff_values, netcdf_values, rtol=0, atol=0, equal_nan=True)
    np.testing.assert_allclose(zarr_values, netcdf_values, rtol=0, atol=0, equal_nan=True)

    with Image.open(output / "reflectivity.png") as image:
        rgba = np.asarray(image.convert("RGBA"))
    _assert_rgba(rgba, netcdf_values, netcdf_quality, grid)
    render = json.loads((output / "reflectivity.render.json").read_text(encoding="utf-8"))
    assert render["shape"] == [grid["height"], grid["width"]]
    assert render["crs"] == grid["crs"]
    assert render["palette"]["id"] == "rdcap-reflectivity-v1"
    assert render["palette"]["version"] == "1"
    assert render["frame_identity"]["station"] == station_id
    assert render["frame_identity"]["key"] == str(timestamp_ms)
    assert render["annotation_rule"]["quality"] == 65
    assert render["geometry"]["row_order"] == "north_to_south"
    np.testing.assert_allclose(
        render["geometry"]["affine"], grid["geotransform"], rtol=0, atol=1e-9
    )

    comparison_record = records.get("comparison")
    if comparison_record is not None:
        comparison = json.loads(_fixture_path(comparison_record).read_text(encoding="utf-8"))
        _assert_rchl_comparison(netcdf_values, netcdf_quality, grid, comparison)


def _check_all_missing(output_root: Path) -> None:
    output = output_root / "all-missing"
    expected_time = np.datetime64("2026-10-01T00:00:00", "ms")
    expected_time_ms = int(expected_time.astype("int64"))
    expected_values = np.full((2, 2), np.nan, dtype=np.float32)
    expected_quality = np.ones((2, 2), dtype=np.uint16)

    with Image.open(output / "all-missing.png") as image:
        rgba = np.asarray(image.convert("RGBA"))
    assert rgba.shape == (2, 2, 4)
    np.testing.assert_array_equal(rgba, np.zeros((2, 2, 4), dtype=np.uint8))
    render = json.loads((output / "all-missing.render.json").read_text(encoding="utf-8"))
    assert render["shape"] == [2, 2]
    assert render["variable"] == "reflectivity"

    with xr.open_dataset(output / "all-missing.nc", engine="h5netcdf") as dataset:
        netcdf_values = dataset["reflectivity"].values.copy()
        netcdf_quality = dataset["quality"].values.copy()
        np.testing.assert_array_equal(dataset["longitude"].values, [120.0, 121.0])
        np.testing.assert_array_equal(dataset["latitude"].values, [24.0, 23.0])
        _assert_time(dataset["time"].values, expected_time_ms)
        _assert_flag_metadata(dataset["quality"].attrs)
    np.testing.assert_array_equal(netcdf_quality, expected_quality)
    np.testing.assert_array_equal(np.isnan(netcdf_values), np.isnan(expected_values))

    with rasterio.open(output / "all-missing.tif") as raster:
        geotiff_values = raster.read(1)
        _assert_wgs84_geographic_crs(raster.crs)
        np.testing.assert_allclose(
            tuple(raster.transform)[:6],
            (1.0, 0.0, 119.5, 0.0, -1.0, 24.5),
            rtol=0,
            atol=1e-9,
        )
    with rasterio.open(output / "all-missing_quality.tif") as raster:
        geotiff_quality = raster.read(1)
    geotiff_metadata = json.loads(
        (output / "all-missing_provenance.json").read_text(encoding="utf-8")
    )
    _assert_iso_time(geotiff_metadata["valid_time"], expected_time_ms)
    _assert_flag_metadata(
        geotiff_metadata,
        masks_key="quality_flag_masks",
        meanings_key="quality_flag_meanings",
    )

    with xr.open_zarr(output / "all-missing.zarr", consolidated=True) as dataset:
        zarr_values = dataset["reflectivity"].values.copy()
        zarr_quality = dataset["quality"].values.copy()
        np.testing.assert_array_equal(dataset["longitude"].values, [120.0, 121.0])
        np.testing.assert_array_equal(dataset["latitude"].values, [24.0, 23.0])
        _assert_time(dataset["time"].values, expected_time_ms)
        _assert_flag_metadata(dataset["quality"].attrs)

    for label, values, quality in (
        ("GeoTIFF", geotiff_values, geotiff_quality),
        ("Zarr", zarr_values, zarr_quality),
    ):
        assert values.shape == (2, 2), f"all-missing {label} shape"
        np.testing.assert_array_equal(np.isnan(values), np.isnan(expected_values))
        np.testing.assert_array_equal(quality, expected_quality)
    np.testing.assert_array_equal(geotiff_quality, netcdf_quality)
    np.testing.assert_array_equal(zarr_quality, netcdf_quality)


def test_rust_rdcap_formats_are_readable_by_independent_python_libraries() -> None:
    output_value = os.environ.get("RADIUST_RDCAP_FORMATS_OUT")
    if not output_value:
        pytest.skip(
            "set RADIUST_RDCAP_FORMATS_OUT to the directory produced by the Rust RDCAP output contract"
        )

    output_root = Path(output_value)
    assert output_root.is_dir(), f"RADIUST_RDCAP_FORMATS_OUT is not a directory: {output_root}"
    manifest = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    assert manifest["status"] == "offline_research_fixture_not_live_acceptance"
    assert manifest["provenance"]["native_http_file_response_retained"] is False
    stations = manifest["stations"]
    assert {station["station_id"] for station in stations} == {
        "TWN/RCHL",
        "JPN/ISHI",
        "PHL/SUBI",
    }
    for station in stations:
        _read_and_check_station(output_root, station)
    _check_all_missing(output_root)
