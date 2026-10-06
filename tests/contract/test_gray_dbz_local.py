from __future__ import annotations

import asyncio
import json
from pathlib import Path

import numpy as np
from radiust import AsyncClient, Client, to_xarray

FIXTURES = Path(__file__).parents[1] / "fixtures" / "gray-dbz" / "local"


def test_sync_decode_and_xarray_keep_values_quality_alpha_and_unknown_location() -> None:
    source = FIXTURES / "225-codes.png"
    with Client() as client:
        result = client.decode_gray_file(source)
        field = to_xarray(result)

    assert field.dims == ("row", "column")
    np.testing.assert_allclose(
        field.values[0],
        np.arange(225, dtype=np.float32) * (5.0 / 16.0),
        rtol=0,
        atol=1e-6,
    )
    np.testing.assert_array_equal(field.coords["quality"].values[0], np.zeros(225, dtype=np.uint16))
    assert field.attrs["units"] == "dBZ"
    assert field.attrs["time_status"] == "unknown"
    assert field.attrs["geolocation"] == "unknown"
    assert field.attrs["encoding"] == "gray-dbz-v1"


def test_async_and_sync_decode_are_equivalent_without_network() -> None:
    source = FIXTURES / "gray-alpha-16.png"

    async def decode() -> object:
        async with AsyncClient() as client:
            return to_xarray(await client.decode_gray_file(source))

    async_field = asyncio.run(decode())
    with Client() as client:
        sync_field = to_xarray(client.decode_gray_file(source))

    np.testing.assert_allclose(
        async_field.values,
        sync_field.values,
        rtol=0,
        atol=0,
        equal_nan=True,
    )
    np.testing.assert_array_equal(
        async_field.coords["quality"].values, sync_field.coords["quality"].values
    )
    alpha = async_field.coords["alpha"]
    assert alpha.dtype == np.uint16
    np.testing.assert_array_equal(alpha.values[0], np.array([0, 1, 255, 256, 65535], dtype=np.uint16))


def test_local_decode_errors_keep_structured_stage_and_code() -> None:
    from radiust import DecodeError

    with Client() as client:
        try:
            client.decode_gray_file(FIXTURES / "visible-out-of-range.png")
        except DecodeError as exc:
            assert exc.context.stage == "decode"
            assert exc.context.code == "invalid_gray_encoding"
        else:
            raise AssertionError("visible gray above 224 must fail")


def test_core_array_entry_preserves_alpha_dtype_and_uses_the_rust_decoder() -> None:
    async def decode() -> object:
        async with AsyncClient() as client:
            return await client._session.decode_gray_values(
                5,
                1,
                [16.0] * 5,
                alpha_json=json.dumps(
                    {"bit_depth": "u16", "values": [0, 1, 255, 256, 65535]}
                ),
            )

    field = to_xarray(asyncio.run(decode()))
    np.testing.assert_allclose(field.values, np.array([[np.nan, 5, 5, 5, 5]], dtype=np.float32), equal_nan=True)
    alpha = field.coords["alpha"]
    assert alpha.dtype == np.uint16
    np.testing.assert_array_equal(alpha.values[0], np.array([0, 1, 255, 256, 65535], dtype=np.uint16))
    assert field.coords["quality"].values[0, 0] == 1
