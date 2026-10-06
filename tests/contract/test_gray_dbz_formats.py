from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path

import numpy as np
import pytest
from radiust import Client, FrameRef

from tests.support.native_raw_cache import seed_native_raw_cache

FIXTURES = Path(__file__).parents[1] / "fixtures" / "gray-dbz" / "local"
REPO_ROOT = Path(__file__).parents[2]


def _read_independently(path: Path, output_format: str):
    xr = pytest.importorskip("xarray")
    if output_format == "netcdf":
        return xr.open_dataset(path, engine="h5netcdf")
    pytest.importorskip("zarr")
    return xr.open_zarr(path)


def _write_raster(client: Client, result: object, root: Path, output_format: str) -> Path:
    report = client.write(result, output=root, format=output_format)
    assert report["status"] == "written", report
    return Path(report["output_uri"])


def test_animated_gif_frames_get_distinct_default_output_paths(tmp_path: Path) -> None:
    source = FIXTURES / "two-frame.gif"
    with Client() as client:
        first = client.decode_gray_file(source, frame_index=0)
        second = client.decode_gray_file(source, frame_index=1)
        first_path = _write_raster(client, first, tmp_path, "netcdf")
        second_path = _write_raster(client, second, tmp_path, "netcdf")

    assert first_path != second_path
    assert first_path.is_file()
    assert second_path.is_file()


def _assert_zarr_array_dimensions(path: Path, *names: str) -> None:
    for name in names:
        attrs = json.loads((path / name / ".zattrs").read_text(encoding="utf-8"))
        assert attrs["_ARRAY_DIMENSIONS"] == ["row", "column"]


def test_netcdf_and_zarr_are_independently_readable_for_codes_and_alpha16(tmp_path: Path) -> None:
    expected = np.arange(225, dtype=np.float32) * np.float32(5.0 / 16.0)
    with Client() as client:
        codes = client.decode_gray_file(FIXTURES / "225-codes.png")
        alpha16 = client.decode_gray_file(FIXTURES / "gray-alpha-16.png")
        for output_format in ("netcdf", "zarr"):
            codes_path = _write_raster(
                client, codes, tmp_path / f"codes-{output_format}", output_format
            )
            alpha_path = _write_raster(
                client, alpha16, tmp_path / f"alpha-{output_format}", output_format
            )
            with _read_independently(codes_path, output_format) as dataset:
                np.testing.assert_allclose(
                    dataset["reflectivity"].values[0], expected, rtol=0, atol=1e-6
                )
                assert dataset["reflectivity"].attrs["units"] == "dBZ"
                assert dataset["quality"].dims == ("row", "column")
                quality = dataset["quality"].values
                np.testing.assert_array_equal(quality[0], np.zeros(225, dtype=np.uint16))
                assert quality[1, 0] == 0  # opaque black is a valid zero
                np.testing.assert_array_equal(quality[1, 1:], np.ones(224, dtype=np.uint16))
                assert dataset["reflectivity"].values[1, 0] == 0
                assert np.isnan(dataset["reflectivity"].values[1, 1:]).all()
                assert dataset.attrs["time_status"] == "unknown"
                assert dataset.attrs["geometry_status"] == "unknown"
                assert "time" not in dataset.variables
                assert not any(name in dataset.variables for name in ("crs", "latitude", "longitude"))
            if output_format == "zarr":
                _assert_zarr_array_dimensions(codes_path, "reflectivity", "quality")
                _assert_zarr_array_dimensions(alpha_path, "reflectivity", "quality", "alpha")
            with _read_independently(alpha_path, output_format) as dataset:
                alpha = dataset["alpha"]
                assert alpha.dtype == np.dtype("uint16")
                assert alpha.attrs["alpha_bit_depth"] == 16
                np.testing.assert_array_equal(
                    alpha.values.ravel(), np.array([0, 1, 255, 256, 65535], dtype=np.uint16)
                )
                quality = dataset["quality"].values.ravel()
                assert quality[0] & 1
                assert np.all(quality[1:] == 0)
                assert np.isnan(dataset["reflectivity"].values.ravel()[0])


def test_nz_adjustment_and_quality_are_independent_of_the_sdk_reader(tmp_path: Path) -> None:
    fixture_path = REPO_ROOT / "tests/fixtures/sources/nz/fixture.json"
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    metadata = fixture["frames"][0]
    artifact = metadata["artifacts"][0]
    valid_time = datetime.fromisoformat(metadata["valid_time"].replace("Z", "+00:00"))
    ref = FrameRef(
        source="nz",
        product="rain",
        valid_time=valid_time,
        station=metadata["station"],
        locator={
            "url": metadata["uri"],
            "station": metadata["station"],
            "name": artifact["name"],
            "media_type": artifact["media_type"],
            "artifacts": [],
        },
        locator_version=metadata["locator_version"],
        revision=metadata["revision"],
    )
    raw_path = fixture_path.parent / artifact["path"]
    cache = tmp_path / "cache"
    seed_native_raw_cache(
        cache,
        ref,
        [(artifact["name"], artifact["media_type"], raw_path.read_bytes())],
    )
    config = {
        "runtime": {"allow_network": False, "temp_root": str(tmp_path / "temp")},
        "cache": {"enabled": True, "dir": str(cache)},
    }
    expected_clips = (
        (274, 128, 228, 16, 0),
        (198, 153, 226, 16, 0),
        (154, 182, 227, 24, 4),
        (174, 212, 225, 24, 4),
        (257, 404, 225, 16, 0),
        (288, 405, 229, 16, 0),
    )

    with Client(config=config) as client:
        result = client.fetch(ref, mode="dbz")
        for output_format in ("netcdf", "zarr"):
            path = _write_raster(
                client, result, tmp_path / f"nz-{output_format}", output_format
            )
            with _read_independently(path, output_format) as dataset:
                adjustment = dataset["encoding_adjustment"].values
                quality = dataset["quality"].values
                reflectivity = dataset["reflectivity"].values
                origin = dataset["origin_quality"].values
                assert int(np.count_nonzero(adjustment)) == 6
                assert origin.shape == reflectivity.shape
                assert dataset.attrs["time_status"] == "known"
                assert dataset.attrs["geometry_status"] == "unknown"
                for column, row, _gray_code, expected_quality, expected_origin in expected_clips:
                    assert adjustment[row, column] == 1
                    assert quality[row, column] == expected_quality
                    assert origin[row, column] == expected_origin
                    if expected_quality & 7:
                        assert np.isnan(reflectivity[row, column])
                    else:
                        assert reflectivity[row, column] == np.float32(70.0)
                stored_time = datetime.fromisoformat(
                    dataset.attrs["radiust_valid_time"].replace("Z", "+00:00")
                )
                assert stored_time == valid_time
                if "time" in dataset.variables:
                    assert np.isfinite(dataset["time"].values).all()
                assert not any(name in dataset.variables for name in ("crs", "latitude", "longitude"))
            if output_format == "zarr":
                _assert_zarr_array_dimensions(
                    path, "reflectivity", "quality", "origin_quality", "encoding_adjustment"
                )
