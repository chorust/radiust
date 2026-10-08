"""Rust-backed canonical and historical gray decoder contracts."""

from __future__ import annotations

import numpy as np
import pytest
from radiust.decoders.gray_dbz import GrayDbzDecoder, LegacyGrayDbzDecoder
from radiust.errors import DecodeError, UnknownColorError


def test_canonical_decoder_is_strict_and_does_not_narrow_float_input():
    decoder = GrayDbzDecoder()
    values, quality = decoder.decode(np.asarray([[0, 16, 224]], dtype=np.uint8))
    np.testing.assert_array_equal(values, np.asarray([[0.0, 5.0, 70.0]], dtype=np.float32))
    assert quality.shape == values.shape
    with pytest.raises(DecodeError):
        decoder.decode(np.asarray([[1.5]], dtype=np.float64))
    with pytest.raises(DecodeError):
        decoder.decode(np.asarray([[225]], dtype=np.uint8))


def test_legacy_decoder_keeps_constructor_tuple_error_and_float_narrowing_contract():
    decoder = LegacyGrayDbzDecoder(strict=False, max_gray=10)
    values, quality = decoder.decode(np.asarray([[1.9, 11.0]], dtype=np.float64))
    assert values.dtype == np.float32
    assert values[0, 0] == pytest.approx(5.0 / 16.0)
    assert np.isnan(values[0, 1])
    assert quality[0, 1] != 0
    with pytest.raises(UnknownColorError):
        LegacyGrayDbzDecoder(max_gray=10).decode(np.asarray([[11]], dtype=np.uint8))


def test_transparent_pixels_remain_missing_in_both_profiles():
    pixels = np.asarray([[[225, 0, 225, 0], [16, 16, 16, 255]]], dtype=np.uint8)
    for decoder in (GrayDbzDecoder(), LegacyGrayDbzDecoder()):
        values, quality = decoder.decode(pixels)
        assert np.isnan(values[0, 0])
        assert values[0, 1] == pytest.approx(5.0)
        assert quality[0, 0] != 0
