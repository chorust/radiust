from __future__ import annotations

import asyncio
import json
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pytest
from radiust import (
    AsyncClient,
    Client,
    FrameRef,
    OperationCancelled,
    StorageError,
    to_xarray,
)

REPO_ROOT = Path(__file__).parents[2]
GRAY_CODES = REPO_ROOT / "tests/fixtures/gray-dbz/local/225-codes.png"
GRAY_ALPHA16 = REPO_ROOT / "tests/fixtures/gray-dbz/local/gray-alpha-16.png"
LEGACY_NATIVE = REPO_ROOT / "tests/fixtures/rust-migration/output/netcdf/field.nc"


def _write_path(report: dict[str, object]) -> Path:
    assert report["status"] == "written", report
    return Path(str(report["output_uri"]))


def test_sync_local_gray_roundtrips_as_numeric_dbz_without_a_frame_ref(tmp_path: Path) -> None:
    with Client() as client:
        original = client.decode_gray_file(GRAY_CODES)
        source_report = client.write(
            original,
            output=tmp_path / "source",
            format="netcdf",
            name="gray-dbz.nc",
        )
        numeric = client.read_dbz(_write_path(source_report))
        assert numeric.data_kind == "pixel_dbz"
        numeric_input = json.loads(numeric.input_json)
        assert numeric_input["kind"] == "numeric_file"
        assert numeric_input["identity"]["kind"] == "local_numeric"
        assert numeric_input["upstream_provenance"]["input_identity"]["identity"]["kind"] == "local_gray"

        copied_report = client.write(
            numeric,
            output=tmp_path / "copy",
            format="zarr",
            name="copied.zarr",
        )
        copied = client.read_dbz(_write_path(copied_report))
        actual = to_xarray(copied)
        expected = to_xarray(original)

        alpha16 = client.decode_gray_file(GRAY_ALPHA16)
        alpha16_report = client.write(
            alpha16,
            output=tmp_path / "alpha16",
            format="netcdf",
            name="alpha16.nc",
        )
        alpha16_raster = to_xarray(client.read_dbz(_write_path(alpha16_report)))

    np.testing.assert_array_equal(actual.values, expected.values)
    np.testing.assert_array_equal(actual.coords["quality"], expected.coords["quality"])
    np.testing.assert_array_equal(actual.coords["alpha"], expected.coords["alpha"])
    assert actual.attrs["units"] == "dBZ"
    assert actual.attrs["input_kind"] == "numeric_file"
    assert alpha16_raster.coords["alpha"].dtype == np.uint16
    np.testing.assert_array_equal(
        alpha16_raster.coords["alpha"].values.ravel(),
        np.array([0, 1, 255, 256, 65535], dtype=np.uint16),
    )


def test_async_local_pixel_write_and_numeric_reread_need_no_frame_ref(tmp_path: Path) -> None:
    async def exercise() -> tuple[object, object, object, object]:
        async with AsyncClient() as client:
            original = await client.decode_gray_file(GRAY_CODES)
            expected_values = to_xarray(original).values.copy()
            first = await client.write(
                original,
                output=tmp_path / "async-source",
                format="netcdf",
                name="gray-dbz.nc",
            )
            numeric = await client.read_dbz(_write_path(first))
            assert numeric.data_kind == "pixel_dbz"
            second = await client.write(
                numeric,
                output=tmp_path / "async-copy",
                format="zarr",
                name="copied.zarr",
            )
            reread = await client.read_dbz(_write_path(second))
            return first, second, to_xarray(reread).values.copy(), expected_values

    first, second, actual_values, expected_values = asyncio.run(exercise())
    assert first["status"] == second["status"] == "written"
    np.testing.assert_array_equal(actual_values, expected_values)
    assert Path(f"{first['output_uri']}.manifest.json").is_file()
    assert Path(f"{second['output_uri']}.manifest.json").is_file()


def test_async_numeric_read_write_and_native_dbz_reuse_keep_units_and_values(
    tmp_path: Path,
) -> None:
    async def exercise() -> tuple[object, object, object]:
        async with AsyncClient() as client:
            source = await client.read_dbz(LEGACY_NATIVE, variable="reflectivity")
            source_values = to_xarray(source.native_field).values.copy()
            report = await client.write(
                source,
                output=tmp_path / "native-copy",
                format="netcdf",
                name="reflectivity.nc",
            )
            reread = await client.read_dbz(Path(str(report["output_uri"])))
            reread_values = to_xarray(reread.native_field).values.copy()
            report2 = await client.write(
                reread,
                output=tmp_path / "native-copy-2",
                format="zarr",
                name="reflectivity.zarr",
            )
            assert report["status"] == report2["status"] == "written"
            return source, reread, (source_values, reread_values)

    source, reread, arrays = asyncio.run(exercise())
    source_values, reread_values = arrays
    np.testing.assert_array_equal(source_values, reread_values)
    assert source.data_kind == reread.data_kind == "native"
    assert json.loads(source.mode_info_json)["method"] == "file_native"
    assert json.loads(reread.input_json)["read_receipt"]["units"] == "dBZ"


def test_numeric_file_identity_rejects_an_unrelated_frame_reference(tmp_path: Path) -> None:
    frame = FrameRef(
        "my",
        "composite",
        datetime(2026, 10, 3, tzinfo=timezone.utc),
        station="east",
    )
    with Client() as client:
        source = client.decode_gray_file(GRAY_CODES)
        report = client.write(
            source, output=tmp_path, format="netcdf", name="source.nc"
        )
        numeric = client.read_dbz(Path(str(report["output_uri"])))
        with pytest.raises(StorageError):
            client.write(
                numeric,
                output=tmp_path,
                format="netcdf",
                name="must-not-commit.nc",
                ref=frame,
            )
    assert not (tmp_path / "must-not-commit.nc").exists()


def test_cancelled_raster_write_keeps_cancelled_error_and_does_not_commit(
    tmp_path: Path,
) -> None:
    output = tmp_path / "cancelled"
    with Client() as client:
        raster = client.decode_gray_file(GRAY_CODES)
        client.cancel()
        with pytest.raises(OperationCancelled) as caught:
            client.write(
                raster,
                output=output,
                format="netcdf",
                name="must-not-commit.nc",
            )
    assert caught.value.code == "cancelled"
    assert caught.value.stage == "commit"
    assert not (output / "must-not-commit.nc").exists()
    assert not (output / "manifest.json").exists()


def test_async_cancelled_raster_write_keeps_cancelled_error_and_does_not_commit(
    tmp_path: Path,
) -> None:
    output = tmp_path / "async-cancelled"

    async def exercise() -> OperationCancelled:
        async with AsyncClient() as client:
            raster = await client.decode_gray_file(GRAY_CODES)
            client.cancel()
            with pytest.raises(OperationCancelled) as caught:
                await client.write(
                    raster,
                    output=output,
                    format="netcdf",
                    name="must-not-commit.nc",
                )
            return caught.value

    cancelled = asyncio.run(exercise())
    assert cancelled.code == "cancelled"
    assert cancelled.stage == "commit"
    assert not (output / "must-not-commit.nc").exists()
    assert not (output / "manifest.json").exists()
