"""Synthetic display mechanics; no tests here constitute a provider golden."""

import hashlib
import json
from dataclasses import asdict
from datetime import datetime, timezone
from io import BytesIO
from types import SimpleNamespace

import numpy as np
import pytest
from PIL import Image
from radiust.display.engine import DisplayBlockedError, render_legacy_display
from radiust.display.raw import preview_raw
from radiust.display.registry import LegacyDisplayRegistry, default_registry
from radiust.display.rules import (
    DisplayEvidence,
    LegacyDisplayRule,
    merge_ordered_steps,
    rule_fingerprint,
)
from radiust.models import Artifact, FrameRef
from radiust.raw import RawFrame

HASH = hashlib.sha256(b"synthetic-pixels").hexdigest()


def _bound_rule(steps, *, constraints=None):
    data = {
        "source": "synthetic", "product": "composite", "path_id": "synthetic/composite",
        "rule_version": "synthetic-v1", "encoding_version": "旧项目-gray-dbz-v1",
        "legacy_reference": "synthetic contract example only; not historical source evidence",
        "ordered_steps": steps, "input_constraints": constraints or {"formats": ["PNG"], "max_pixels": 64},
        "validation_status": "passed",
    }
    data["config_hash"] = rule_fingerprint(data)
    rule = LegacyDisplayRule.from_mapping(data)
    evidence = DisplayEvidence.from_mapping({
        "path_id": rule.path_id, "status": "passed", "rule_version": rule.rule_version,
        "config_hash": rule.config_hash, "input_hashes": [HASH], "output_hash": HASH,
        "baseline_identity": "synthetic fixture", "baseline_hash": HASH,
        "crop": [0, 0, 1, 1], "shape": [1, 1], "pixel_diff_count": 0,
        "alpha_comparison": "identical", "background_comparison": "identical",
        "missing_comparison": "identical", "intentional_differences": [],
        "review_conclusion": "synthetic equality", "blocked_reasons": [],
        "sample_provenance": "synthetic only, not a real display migration",
        "scientific_status_unchanged": True,
    })
    return rule, evidence


def _rgb(rows):
    return np.asarray(rows, dtype=np.uint8)


def test_palette_gray_0_to_224_and_alpha_zero_is_not_opaque_black():
    rgba = _rgb([[[0, 0, 0, 255], [0, 0, 0, 0], [1, 2, 3, 255], [3, 2, 1, 255]]])
    snapshot = rgba.copy()
    rule, evidence = _bound_rule([
        {"op": "zero_palette", "colors": [[0, 0, 0]], "basis": "synthetic"},
        {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5},
                                     {"rgb": [3, 2, 1], "dbz": 100}], "basis": "synthetic"},
        {"op": "gray_encode", "basis": "synthetic; 16/5 and clamp 0..224"},
    ])
    result = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert result.tolist() == [[[0, 0, 0, 255], [0, 0, 0, 0],
                                [16, 16, 16, 255], [224, 224, 224, 255]]]
    np.testing.assert_array_equal(rgba, snapshot)
    assert not np.shares_memory(rgba, result)


def test_legacy_tile_channel_encodings_match_old_byte_order_and_alpha_behavior():
    rain_rgba = np.zeros((1, 9, 4), dtype=np.uint8)
    rain_rgba[0, :, 0] = [0, 32, 33, 127, 128, 159, 160, 161, 255]
    rain_rgba[0, :, 3] = [255, 0, 255, 255, 255, 0, 255, 255, 255]
    rain_snapshot = rain_rgba.copy()
    rain_rule, rain_evidence = _bound_rule([
        {"op": "legacy_channel_decode", "method": "rainviewer_v2_red", "channel": 0,
         "basis": "pinned old RainViewer v2 red-byte decoder"},
        {"op": "gray_encode", "method": "legacy_tile_uint8_then_clip_224",
         "arithmetic_dtype": "float32", "basis": "old uint8 cast precedes 0..224 clip"},
    ])
    rain = render_legacy_display(rain_rgba, rain_rule, rain_evidence, input_format="PNG")
    np.testing.assert_array_equal(rain[0, :, 0], [0, 0, 3, 48, 0, 0, 0, 3, 48])
    np.testing.assert_array_equal(rain[:, :, 0], rain[:, :, 1])
    np.testing.assert_array_equal(rain[:, :, 0], rain[:, :, 2])
    assert np.all(rain[:, :, 3] == 255)
    np.testing.assert_array_equal(rain_rgba, rain_snapshot)

    windy_rgba = np.zeros((1, 8, 4), dtype=np.uint8)
    windy_rgba[0, :, 1] = [0, 1, 2, 3, 159, 160, 161, 255]
    windy_rgba[0, :, 3] = [255, 0, 255, 255, 255, 255, 0, 255]
    windy_rule, windy_evidence = _bound_rule([
        {"op": "legacy_channel_decode", "method": "windy_v2_green", "channel": 1,
         "basis": "pinned old Windy v2 green-byte decoder"},
        {"op": "gray_encode", "method": "legacy_tile_uint8_then_clip_224",
         "arithmetic_dtype": "float64", "basis": "old uint8 cast precedes 0..224 clip"},
    ])
    windy = render_legacy_display(windy_rgba, windy_rule, windy_evidence, input_format="PNG")
    np.testing.assert_array_equal(windy[0, :, 0], [0, 0, 3, 3, 224, 0, 0, 150])
    assert np.all(windy[:, :, 0] == windy[:, :, 1])
    assert np.all(windy[:, :, 0] == windy[:, :, 2])
    assert np.all(windy[:, :, 3] == 255)


def test_legacy_tile_channel_decoder_rejects_unmapped_channel_identity():
    rule, evidence = _bound_rule([
        {"op": "legacy_channel_decode", "method": "rainviewer_v2_red", "channel": 1,
         "basis": "pinned old RainViewer v2 decoder requires red"},
        {"op": "gray_encode", "method": "legacy_tile_uint8_then_clip_224",
         "arithmetic_dtype": "float32", "basis": "synthetic"},
    ])
    with pytest.raises(DisplayBlockedError, match="method or channel"):
        render_legacy_display(_rgb([[[1, 2, 3, 255]]]), rule, evidence, input_format="PNG")


def test_unrecognized_rgb_is_never_reinterpreted_as_dbz_or_silently_fallback():
    rule, evidence = _bound_rule([
        {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "test"},
        {"op": "gray_encode", "basis": "test"},
    ])
    with pytest.raises(DisplayBlockedError, match="unmapped"):
        render_legacy_display(_rgb([[[100, 100, 100, 255]]]), rule, evidence, input_format="PNG")
    with pytest.raises(DisplayBlockedError, match="format"):
        render_legacy_display(_rgb([[[1, 2, 3, 255]]]), rule, evidence, input_format="WMS")
    with pytest.raises(DisplayBlockedError, match="evidence"):
        render_legacy_display(_rgb([[[1, 2, 3, 255]]]), rule, None, input_format="PNG")


def test_unmatched_preview_falls_back_but_matched_failed_rule_never_falls_back(tmp_path):
    rule, evidence = _bound_rule([
        {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "synthetic"},
        {"op": "gray_encode", "basis": "synthetic"},
    ])
    buffer = BytesIO()
    Image.new("RGBA", (1, 1), (9, 8, 7, 255)).save(buffer, format="PNG")
    payload = buffer.getvalue()
    ref = FrameRef("synthetic", "composite", datetime(2026, 9, 22, tzinfo=timezone.utc))
    index = tmp_path / "index.json"
    index.write_text(json.dumps({"schema_version": 1, "paths": []}), encoding="utf-8")
    with RawFrame(ref, (Artifact("synthetic.png", "data", "image/png", payload),)) as raw:
        unchanged = preview_raw(raw, registry=LegacyDisplayRegistry(tmp_path), apply_legacy=True)
        assert unchanged.display_mode == "original"
        assert unchanged.rgba[0, 0].tolist() == [9, 8, 7, 255]
        assert "no validated legacy display rule" in (unchanged.reason or "")

        (tmp_path / "rule.json").write_text(json.dumps(asdict(rule)), encoding="utf-8")
        (tmp_path / "evidence.json").write_text(json.dumps(asdict(evidence)), encoding="utf-8")
        index.write_text(json.dumps({"schema_version": 1, "paths": [{
            "source": rule.source, "product": rule.product, "path_id": rule.path_id,
            "status": "passed", "rule_version": rule.rule_version,
            "config_hash": rule.config_hash, "rule_file": "rule.json",
            "evidence_file": "evidence.json",
        }]}), encoding="utf-8")
        with pytest.raises(DisplayBlockedError, match="unmapped"):
            preview_raw(raw, registry=LegacyDisplayRegistry(tmp_path), apply_legacy=True)
        assert raw.bytes() == payload


def test_ordered_background_crop_resize_preserves_missing_and_dimensions():
    rgba = _rgb([[[255, 255, 255, 255], [1, 2, 3, 255], [1, 2, 3, 255]],
                 [[255, 255, 255, 255], [1, 2, 3, 255], [1, 2, 3, 255]]])
    rule, evidence = _bound_rule([
        {"op": "background_mask", "colors": [[255, 255, 255]], "basis": "test"},
        {"op": "crop", "bounds": [0, 0, 2, 2], "basis": "test"},
        {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "test"},
        {"op": "gray_encode", "basis": "test"},
        {"op": "resize", "shape": [4, 4], "method": "nearest", "basis": "test"},
    ])
    gray = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert gray.shape == (4, 4, 4)
    assert np.all(gray[:, :2, 3] == 0)
    assert np.all(gray[:, 2:, 3] == 255)
    assert np.all(gray[:, 2:, 0] == 16)


def test_rule_identity_and_resource_limit_fail_closed():
    rule, evidence = _bound_rule([
        {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "test"},
        {"op": "gray_encode", "basis": "test"},
    ], constraints={"formats": ["PNG"], "max_pixels": 1})
    assert rule.matches("synthetic", "composite", "synthetic/composite", "synthetic-v1")
    assert not rule.matches("synthetic", "composite", "synthetic/composite", "synthetic-v2")
    assert not rule.matches("synthetic", "different", "synthetic/composite", "synthetic-v1")
    with pytest.raises(DisplayBlockedError, match="limit"):
        render_legacy_display(np.zeros((2, 2, 4), dtype=np.uint8), rule, evidence, input_format="PNG")
    other, _ = _bound_rule([{"op": "gray_encode", "basis": "test"}])
    with pytest.raises(DisplayBlockedError, match="evidence"):
        render_legacy_display(_rgb([[[1, 2, 3, 255]]]), other, evidence, input_format="PNG")

    dimension_rule, dimension_evidence = _bound_rule([
        {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "test"},
        {"op": "gray_encode", "basis": "test"},
    ], constraints={"formats": ["PNG"], "max_pixels": 64, "decoded_size": [1, 1]})
    with pytest.raises(DisplayBlockedError, match="dimensions"):
        render_legacy_display(_rgb([[[1, 2, 3, 255], [1, 2, 3, 255]]]),
                              dimension_rule, dimension_evidence, input_format="PNG")


def test_product_display_overrides_are_explicit_and_order_preserving():
    base = [
        {"step_id": "background", "op": "background_mask", "colors": [[255, 255, 255]], "basis": "base"},
        {"step_id": "palette", "op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "base"},
        {"step_id": "encode", "op": "gray_encode", "basis": "base"},
    ]
    merged = merge_ordered_steps(base, [
        {"action": "replace", "target": "palette", "basis": "product evidence",
         "step": {"step_id": "palette", "op": "palette", "entries": [{"rgb": [3, 2, 1], "dbz": 5}],
                  "basis": "product"}},
        {"action": "insert_before", "target": "encode", "basis": "product sequence evidence",
         "step": {"step_id": "crop", "op": "crop", "bounds": [0, 0, 1, 1], "basis": "product"}},
    ])
    assert [step["step_id"] for step in merged] == ["background", "palette", "crop", "encode"]
    assert merged[1]["override_basis"] == "product evidence"
    assert base[1]["entries"][0]["rgb"] == [1, 2, 3]
    assert merge_ordered_steps(base, [{"action": "remove", "target": "background",
                                       "basis": "product removes base background"}])[0]["step_id"] == "palette"
    for changes in (
        [{"action": "remove", "target": "missing", "basis": "product"}],
        [{"action": "replace", "target": "palette", "basis": "product"}],
        [{"action": "insert_after", "target": "palette", "basis": "product",
          "step": {"step_id": "encode", "op": "gray_encode", "basis": "product"}}],
        [{"action": "remove", "target": "palette"}],
    ):
        with pytest.raises(ValueError):
            merge_ordered_steps(base, changes)


def test_color_preprocess_threshold_and_range_mask_are_executed_in_order():
    rgba = _rgb([[[255, 255, 255, 255], [2, 2, 2, 255], [4, 4, 4, 255], [1, 1, 1, 255]]])
    rule, evidence = _bound_rule([
        {"op": "color_preprocess", "replacements": [{"from": [2, 2, 2], "to": [1, 2, 3]}], "basis": "test"},
        {"op": "background_mask", "colors": [[255, 255, 255]], "basis": "test"},
        {"op": "palette", "entries": [
            {"rgb": [1, 2, 3], "dbz": 5}, {"rgb": [4, 4, 4], "dbz": 100},
            {"rgb": [1, 1, 1], "dbz": -5}], "basis": "test"},
        {"op": "threshold", "min_dbz": 0, "max_dbz": 70,
         "below": "mask", "above": "cap", "basis": "test"},
        {"op": "range_mask", "bounds": [1, 0, 4, 1], "basis": "test"},
        {"op": "gray_encode", "basis": "test"},
    ])
    got = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert got[0, :, 3].tolist() == [0, 255, 255, 0]
    assert got[0, :, 0].tolist() == [0, 16, 224, 0]


def test_disk_mask_and_four_equal_neighbor_gap_repair_require_exact_methods():
    rgba = np.tile(np.asarray([[[1, 2, 3, 255]]], dtype=np.uint8), (5, 5, 1))
    rgba[2, 2, 3] = 0
    rule, evidence = _bound_rule([
        {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "test"},
        {"op": "gap_repair", "method": "four_equal_neighbors", "max_passes": 1, "basis": "test"},
        {"op": "disk_mask", "center": [2, 2], "radius": 1, "basis": "test"},
        {"op": "gray_encode", "basis": "test"},
    ])
    got = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert got[2, 2].tolist() == [16, 16, 16, 255]
    assert got[0, 0, 3] == 0
    rule_bad, evidence_bad = _bound_rule([
        {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "test"},
        {"op": "gap_repair", "method": "interpolate", "max_passes": 1, "basis": "test"},
        {"op": "gray_encode", "basis": "test"},
    ])
    with pytest.raises(DisplayBlockedError, match="unsupported gap repair"):
        render_legacy_display(rgba, rule_bad, evidence_bad, input_format="PNG")


def test_old_vq_palette_zero_mask_and_cookbook_encoding_are_exact():
    rgba = _rgb([[[17, 0, 0, 0], [0, 0, 17, 255], [0, 0, 0, 255], [255, 255, 255, 255]]])
    rule, evidence = _bound_rule([
        {"op": "legacy_palette", "colors": [[16, 0, 0], [0, 0, 16]],
         "zero_colors": [[255, 255, 255]], "value_threshold": 1, "zero_threshold": 0,
         "cookbook": [0, 5, 10], "invalid_fraction_limit": 1,
         "basis": "synthetic snapshot of the old scipy.vq palette contract"},
        {"op": "gap_repair", "method": "zero_invalid", "basis": "old inpaint-disabled path"},
        {"op": "gray_encode", "method": "legacy_cookbook", "basis": "old ×3.2 uint8 encoding"},
    ])
    result = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert result[0, :, 0].tolist() == [16, 32, 0, 0]
    assert result[0, :, 3].tolist() == [255, 255, 255, 255]


def test_old_cookbook_may_omit_an_unused_last_palette_code_but_never_index_past_end():
    rgba = _rgb([[[1, 2, 3, 255]]])
    rule, evidence = _bound_rule([
        {"op": "legacy_palette", "colors": [[1, 2, 3], [4, 5, 6]], "zero_colors": [],
         "value_threshold": 0, "zero_threshold": 0, "cookbook": [0, 5],
         "invalid_fraction_limit": 0, "basis": "pinned old config has one fewer cookbook value"},
        {"op": "gap_repair", "method": "zero_invalid", "basis": "old inpaint-disabled path"},
        {"op": "gray_encode", "method": "legacy_cookbook", "basis": "old ×3.2 uint8 encoding"},
    ])
    got = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert got[0, 0].tolist() == [16, 16, 16, 255]

    rgba[:, :, :3] = [4, 5, 6]
    with pytest.raises(DisplayBlockedError, match="exceeds"):
        render_legacy_display(rgba, rule, evidence, input_format="PNG")


def test_old_zero_palette_forces_zero_after_nonzero_cookbook_entry_zero():
    rgba = _rgb([[[10, 0, 0, 255], [255, 255, 255, 255]]])
    rule, evidence = _bound_rule([
        {"op": "legacy_palette", "colors": [[10, 0, 0]], "zero_colors": [[255, 255, 255]],
         "value_threshold": 255, "zero_threshold": 0, "cookbook": [5, 10],
         "invalid_fraction_limit": 1, "basis": "old post-cookbook zero mask"},
        {"op": "gap_repair", "method": "zero_invalid", "basis": "old inpaint-disabled path"},
        {"op": "gray_encode", "method": "legacy_cookbook", "basis": "old ×3.2 uint8 encoding"},
    ])
    result = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert result[0, :, 0].tolist() == [32, 0]


def test_old_opencv_inpaint_and_height_center_disk_match_the_legacy_order():
    pytest.importorskip("cv2")
    rgba = np.tile(np.asarray([[[10, 0, 0, 255]]], dtype=np.uint8), (3, 3, 1))
    rgba[1, 1, :3] = [200, 200, 200]
    rule, evidence = _bound_rule([
        {"op": "legacy_palette", "colors": [[10, 0, 0]], "zero_colors": [],
         "value_threshold": 0, "zero_threshold": 0, "cookbook": [0, 5],
         "invalid_fraction_limit": 1, "basis": "synthetic"},
        {"op": "disk_mask", "method": "legacy_height_center_disk", "basis": "old get_disc_mask"},
        {"op": "gap_repair", "method": "opencv_inpaint", "radius": 1,
         "inpaint_method": "NS", "basis": "old cv2.INPAINT_NS"},
        {"op": "gray_encode", "method": "legacy_cookbook", "basis": "old ×3.2 uint8 encoding"},
    ])
    result = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert result[:, :, 0].tolist() == [[0, 16, 0], [16, 16, 16], [0, 16, 0]]
    assert np.all(result[:, :, 3] == 255)


def test_old_luminance_alpha_encoding_matches_fr_pt_conversion_contract():
    rgba = _rgb([
        [[255, 0, 0, 255], [0, 255, 0, 128], [0, 0, 255, 0]],
        [[255, 255, 255, 255], [0, 0, 0, 255], [37, 81, 193, 73]],
    ])
    rule, evidence = _bound_rule([
        {"op": "legacy_luminance_alpha", "basis": "pinned FR/PT _png_to_map implementation"},
    ])
    result = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    arr = rgba.astype(np.float32)
    alpha = arr[:, :, 3] / 255.0
    lum = 0.299 * arr[:, :, 0] + 0.587 * arr[:, :, 1] + 0.114 * arr[:, :, 2]
    expected = np.clip(lum * alpha / 255.0 * 224.0, 0.0, 224.0).astype(np.uint8)
    assert result[:, :, 0].tolist() == expected.tolist()
    assert np.array_equal(result[:, :, 0], result[:, :, 1])
    assert np.array_equal(result[:, :, 0], result[:, :, 2])
    assert np.all(result[:, :, 3] == 255)


def test_legacy_crop_reproduces_pillow_padding_with_explicit_palette_color():
    rgba = _rgb([[[9, 8, 7, 255], [9, 8, 7, 255]],
                 [[9, 8, 7, 255], [9, 8, 7, 255]]])
    rule, evidence = _bound_rule([
        {"op": "legacy_crop", "bounds": [-1, 0, 3, 3], "pad_rgb": [3, 244, 4],
         "basis": "old Pillow P-mode crop pads with palette index zero"},
        {"op": "legacy_palette", "colors": [[3, 244, 4], [9, 8, 7]], "zero_colors": [],
         "value_threshold": 0, "zero_threshold": 0, "cookbook": [0, 5, 10],
         "invalid_fraction_limit": 1, "basis": "synthetic exact palette"},
        {"op": "gap_repair", "method": "zero_invalid", "basis": "old inpaint-disabled path"},
        {"op": "gray_encode", "method": "legacy_cookbook", "basis": "old ×3.2 uint8 encoding"},
    ])
    result = render_legacy_display(rgba, rule, evidence, input_format="PNG")
    assert result.shape == (3, 4, 4)
    assert result[:, 0, 0].tolist() == [16, 16, 16]
    assert result[:, 1:, 0].tolist() == [[32, 32, 16], [32, 32, 16], [16, 16, 16]]
    assert np.all(result[:, :, 3] == 255)


def test_packaged_old_display_rules_have_path_bound_passed_evidence():
    registry = default_registry()
    cases = (
        ("au", "composite", None), ("ca", "rain", "CASFT"),
        ("es", "composite", "ESCOMP"), ("fr", "composite", "FRCOMP"),
        ("id", "composite", None), ("kr", "composite", None),
        ("my", "composite", "east"), ("my", "composite", "peninsular"),
        ("nz", "rain", "NZAU2"), ("pt", "composite", "PTST2"),
        ("sg", "composite", None), ("th", "composite", "kkn240Loop"),
        ("th_royalrain", "cappi", "takhli"), ("tw", "observation", "CV1_3600"),
        ("vn", "cmax", None),
    )
    for source, product, station in cases:
        ref = FrameRef(source, product, datetime(2026, 1, 1, tzinfo=timezone.utc), station=station)
        rule, evidence, reason = registry.select(SimpleNamespace(ref=ref))
        assert reason == ""
        assert rule is not None and rule.validation_status == "passed"
        assert evidence is not None and evidence.status == "passed"
        assert evidence.validates(rule)
