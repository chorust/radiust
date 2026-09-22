from __future__ import annotations

import importlib.util

import numpy as np
import pytest
from radiust.decoders.exact import (
    QUALITY_MISSING,
    QUALITY_UNKNOWN,
    ExactPaletteDecoder,
)
from radiust.decoders.gray_dbz import LegacyGrayDbzDecoder
from radiust.decoders.nearest import NearestPaletteDecoder
from radiust.decoders.recovery import recover_with_fill
from radiust.errors import MissingDependencyError, UnknownColorError


def test_exact_decoder_separates_known_missing_and_unknown_colors():
    palette = {(0, 0, 0, 255): (0.0, 1), (255, 0, 0, 0): (None, int(QUALITY_MISSING))}
    decoder = ExactPaletteDecoder(palette)
    pixels = np.array([[[0, 0, 0, 255], [255, 0, 0, 0], [1, 2, 3, 255]]], dtype=np.uint8)

    with pytest.raises(UnknownColorError):
        decoder.decode(pixels)

    values, quality = ExactPaletteDecoder(palette, strict=False).decode(pixels)
    assert values[0, 0] == 0
    assert np.isnan(values[0, 1]) and np.isnan(values[0, 2])
    assert quality[0, 1] == QUALITY_MISSING
    assert quality[0, 2] == QUALITY_UNKNOWN


def test_nearest_decoder_requires_explicit_distance_threshold():
    decoder = NearestPaletteDecoder({(0, 0, 0): 0.0, (255, 255, 255): 1.0}, threshold=2, strict=False)
    values, quality = decoder.decode(np.array([[[1, 1, 1], [128, 128, 128]]], dtype=np.uint8))

    assert values[0, 0] == 0
    assert np.isnan(values[0, 1])
    assert quality[0, 1] != 0


def test_legacy_gray_dbz_decoder_reverses_old_project_encoding():
    encoded = np.array([[0, 16, 32, 224]], dtype=np.uint8)

    values, quality = LegacyGrayDbzDecoder().decode(encoded)

    np.testing.assert_allclose(values, [[0.0, 5.0, 10.0, 70.0]])
    np.testing.assert_array_equal(quality, np.zeros((1, 4), dtype="uint16"))


def test_legacy_gray_dbz_decoder_preserves_missing_and_rejects_non_gray_pixels():
    encoded = np.array(
        [
            [
                [16, 16, 16, 255],
                [0, 0, 0, 0],
                [16, 17, 16, 255],
                [225, 225, 225, 255],
            ]
        ],
        dtype=np.uint8,
    )

    with pytest.raises(UnknownColorError):
        LegacyGrayDbzDecoder().decode(encoded)

    values, quality = LegacyGrayDbzDecoder(strict=False).decode(encoded)

    np.testing.assert_allclose(values[0, 0], 5.0)
    assert np.isnan(values[0, 1])
    assert np.isnan(values[0, 2])
    assert np.isnan(values[0, 3])
    assert quality[0, 1] == QUALITY_MISSING
    assert quality[0, 2] == QUALITY_UNKNOWN
    assert quality[0, 3] == QUALITY_UNKNOWN


def test_recovery_is_opt_in_and_marks_changed_pixels():
    if importlib.util.find_spec("scipy") is None:
        with pytest.raises(MissingDependencyError):
            recover_with_fill(np.array([[1.0, np.nan, 3.0]], dtype="float32"), np.zeros((1, 3), dtype="uint16"))
        return
    values, quality = recover_with_fill(np.array([[1.0, np.nan, 3.0]], dtype="float32"), np.zeros((1, 3), dtype="uint16"))
    assert values[0, 1] == 2
    assert quality[0, 1] != 0
