"""Fail-closed, presentation-only execution of explicitly evidenced rules.

Source transformations are declared in versioned resources and replayed against
source-bound display evidence. This module never infers a historical palette or
turns a display value into scientific data.
"""

from __future__ import annotations

import math
from collections.abc import Mapping
from typing import Any

import numpy as np
from PIL import Image

from .rules import DisplayEvidence, LegacyDisplayRule


class DisplayBlockedError(ValueError):
    """Legacy rendering is unavailable or the declared conversion cannot run."""


def _colors(step: Mapping[str, Any], name: str = "colors") -> set[tuple[int, int, int]]:
    colors = step.get(name)
    if not isinstance(colors, list) or not colors:
        raise DisplayBlockedError(f"{name} requires a nonempty source-supported color list")
    parsed: set[tuple[int, int, int]] = set()
    for item in colors:
        if (not isinstance(item, (list, tuple)) or len(item) != 3 or
                any(type(channel) is not int or not 0 <= channel <= 255 for channel in item)):
            raise DisplayBlockedError(f"{name} contains an invalid RGB color")
        key = tuple(item)
        if key in parsed:
            raise DisplayBlockedError(f"{name} repeats an RGB color")
        parsed.add(key)
    return parsed


def _geometry(step: Mapping[str, Any], key: str, shape: tuple[int, int]) -> tuple[int, int, int, int]:
    bounds = step.get(key)
    height, width = shape
    if (not isinstance(bounds, (list, tuple)) or len(bounds) != 4 or
            any(type(v) is not int for v in bounds)):
        raise DisplayBlockedError(f"{key} must specify integer pixel bounds")
    left, top, right, bottom = bounds
    if not (0 <= left < right <= width and 0 <= top < bottom <= height):
        raise DisplayBlockedError(f"{key} exceeds the image bounds")
    return left, top, right, bottom


def _positive_int(value: object, label: str) -> int:
    if type(value) is not int or value < 1:
        raise DisplayBlockedError(f"{label} must be a positive integer")
    return value


def _finite_number(value: object, label: str) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        raise DisplayBlockedError(f"{label} must be a finite number")
    return float(value)


def _rgb_equals(rgba: np.ndarray, color: tuple[int, int, int]) -> np.ndarray:
    return np.all(rgba[:, :, :3] == color, axis=2)


def _color_sequence(value: object, *, allow_empty: bool = False) -> tuple[tuple[int, int, int], ...]:
    if not isinstance(value, list) or (not value and not allow_empty):
        raise DisplayBlockedError("legacy palette requires an explicit RGB color list")
    colors: list[tuple[int, int, int]] = []
    for item in value:
        if (not isinstance(item, (list, tuple)) or len(item) != 3 or
                any(type(channel) is not int or not 0 <= channel <= 255 for channel in item)):
            raise DisplayBlockedError("legacy palette contains an invalid RGB color")
        colors.append(tuple(item))
    if len(set(colors)) != len(colors):
        raise DisplayBlockedError("legacy palette repeats an RGB color")
    return tuple(colors)


def _nearest_rgb(rgba: np.ndarray, colors: tuple[tuple[int, int, int], ...]) -> tuple[np.ndarray, np.ndarray]:
    """Return first-nearest palette indices and exact squared RGB distances.

    The old SciPy VQ inputs are 8-bit RGB values cast to float32. Their squared
    Euclidean distances are integer sums below 2**18, so int32 arithmetic is
    exact and preserves SciPy's first-entry tie behavior without requiring the
    optional SciPy extra at display time.
    """
    if not colors:
        raise DisplayBlockedError("legacy palette is empty")
    pixels = rgba[:, :, :3].reshape(-1, 3).astype(np.int16, copy=False)
    codebook = np.asarray(colors, dtype=np.int16)
    indices = np.empty(len(pixels), dtype=np.uint8)
    distances = np.empty(len(pixels), dtype=np.int32)
    chunk_size = 16_384
    for start in range(0, len(pixels), chunk_size):
        chunk = pixels[start:start + chunk_size].astype(np.int32, copy=False)
        delta = chunk[:, None, :] - codebook[None, :, :].astype(np.int32, copy=False)
        squared = np.sum(delta * delta, axis=2, dtype=np.int32)
        nearest = np.argmin(squared, axis=1)
        rows = np.arange(len(chunk))
        indices[start:start + len(chunk)] = nearest.astype(np.uint8)
        distances[start:start + len(chunk)] = squared[rows, nearest]
    shape = rgba.shape[:2]
    return indices.reshape(shape), distances.reshape(shape)


def render_legacy_display(
    source_rgba: np.ndarray,
    rule: LegacyDisplayRule,
    evidence: DisplayEvidence | None,
    *,
    input_format: str,
    limits: Mapping[str, object] | None = None,
) -> np.ndarray:
    """Run only the declared sequence; return detached RGBA without mutating raw.

    The caller must independently establish the raw/frame provenance and
    reproduce the evidence before registering a rule for automatic CLI use.
    """
    if rule.validation_status != "passed" or evidence is None or not evidence.validates(rule):
        raise DisplayBlockedError("legacy display evidence is absent or does not match this rule")
    constraints = rule.input_constraints
    formats = constraints.get("formats")
    if not isinstance(formats, (list, tuple)) or input_format not in formats or input_format not in {"PNG", "GIF", "WEBP"}:
        raise DisplayBlockedError("legacy display input format is not explicitly supported")
    if (not isinstance(source_rgba, np.ndarray) or source_rgba.dtype != np.uint8 or
            source_rgba.ndim != 3 or source_rgba.shape[-1] != 4):
        raise DisplayBlockedError("legacy display requires an RGBA uint8 image")
    limits = limits or {}
    pixel_limit = min(_positive_int(constraints.get("max_pixels"), "rule max_pixels"),
                      _positive_int(limits.get("max_pixels", 100_000_000), "runtime max_pixels"))
    temp_limit = _positive_int(limits.get("max_temp_bytes", 10 * 1024**3), "runtime max_temp_bytes")
    h, w, _ = source_rgba.shape
    if h * w > pixel_limit or h * w * 32 > temp_limit:
        raise DisplayBlockedError("legacy display pixel or temporary memory limit exceeded")
    decoded_size = constraints.get("decoded_size")
    if decoded_size is not None:
        if (not isinstance(decoded_size, (list, tuple)) or len(decoded_size) != 2 or
                any(type(value) is not int or value <= 0 for value in decoded_size)):
            raise DisplayBlockedError("rule decoded_size must contain positive width and height")
        if (w, h) != tuple(decoded_size):
            raise DisplayBlockedError("legacy display input dimensions differ from the verified source sample")

    rgba = np.ascontiguousarray(source_rgba).copy()
    values = np.full((h, w), np.nan, dtype=np.float64)
    mapped = np.zeros((h, w), dtype=bool)
    zero_colors: set[tuple[int, int, int]] = set()
    has_palette = False
    encoded = False
    legacy_codes: np.ndarray | None = None
    legacy_invalid: np.ndarray | None = None
    legacy_keep_mask: np.ndarray | None = None
    legacy_cookbook: np.ndarray | None = None
    legacy_channel_method: str | None = None

    for step in rule.ordered_steps:
        op = step["op"]
        if op == "color_preprocess":
            if has_palette or encoded:
                raise DisplayBlockedError("color preprocess must precede the palette")
            changes = step.get("replacements")
            if not isinstance(changes, list) or not changes:
                raise DisplayBlockedError("color preprocess requires explicit replacements")
            for item in changes:
                source = _colors({"colors": [item.get("from")]}) if isinstance(item, Mapping) else None
                dest = _colors({"colors": [item.get("to")]}) if isinstance(item, Mapping) else None
                if source is None or dest is None:
                    raise DisplayBlockedError("invalid color replacement")
                mask = _rgb_equals(rgba, next(iter(source))) & (rgba[:, :, 3] != 0)
                rgba[mask, :3] = next(iter(dest))
        elif op == "background_mask":
            if encoded:
                raise DisplayBlockedError("background mask must precede gray encoding")
            mask = np.zeros(rgba.shape[:2], dtype=bool)
            for color in _colors(step):
                mask |= _rgb_equals(rgba, color)
            rgba[mask, 3] = 0
            values[mask] = np.nan
            mapped[mask] = False
        elif op == "zero_palette":
            if has_palette or encoded:
                raise DisplayBlockedError("zero palette must precede the value palette")
            additional = _colors(step)
            if additional & zero_colors:
                raise DisplayBlockedError("duplicate source zero color")
            zero_colors |= additional
        elif op == "palette":
            if has_palette or encoded:
                raise DisplayBlockedError("exactly one evidence-bound palette is allowed")
            entries = step.get("entries")
            if not isinstance(entries, list) or not entries:
                raise DisplayBlockedError("palette requires explicit source RGB to dBZ entries")
            table: dict[tuple[int, int, int], float] = {key: 0.0 for key in zero_colors}
            for entry in entries:
                if not isinstance(entry, Mapping):
                    raise DisplayBlockedError("palette entry is invalid")
                key = next(iter(_colors({"colors": [entry.get("rgb")]})))
                if key in table:
                    raise DisplayBlockedError("palette contains duplicate or conflicting RGB entry")
                table[key] = _finite_number(entry.get("dbz"), "palette dBZ")
            for color, dbz in table.items():
                mask = _rgb_equals(rgba, color) & (rgba[:, :, 3] != 0)
                values[mask] = dbz
                mapped[mask] = True
            if np.any((rgba[:, :, 3] != 0) & ~mapped):
                raise DisplayBlockedError("unmapped opaque RGB pixels: no inferred luminance or WMS conversion")
            has_palette = True
        elif op == "legacy_palette":
            if has_palette or encoded:
                raise DisplayBlockedError("exactly one legacy palette operation is allowed")
            colors = _color_sequence(step.get("colors"))
            zero_colors_list = _color_sequence(step.get("zero_colors"), allow_empty=True)
            value_threshold = _finite_number(step.get("value_threshold"), "value_threshold")
            zero_threshold = _finite_number(step.get("zero_threshold"), "zero_threshold")
            if value_threshold < 0 or zero_threshold < 0:
                raise DisplayBlockedError("legacy palette thresholds cannot be negative")
            cookbook = step.get("cookbook")
            if (not isinstance(cookbook, list) or len(cookbook) < len(colors) or
                    len(colors) > 255):
                raise DisplayBlockedError("legacy cookbook must cover the declared palette")
            legacy_cookbook = np.asarray(
                [_finite_number(item, "cookbook value") for item in cookbook], dtype=np.float32
            )
            nearest, distance_squared = _nearest_rgb(rgba, colors)
            legacy_codes = nearest.astype(np.uint16) + 1
            legacy_invalid = distance_squared > value_threshold * value_threshold
            if zero_colors_list:
                _, zero_distance_squared = _nearest_rgb(rgba, zero_colors_list)
                legacy_keep_mask = zero_distance_squared > zero_threshold * zero_threshold
            else:
                legacy_keep_mask = np.ones(rgba.shape[:2], dtype=bool)
            legacy_invalid &= legacy_keep_mask
            legacy_codes[~legacy_keep_mask] = 0
            invalid_limit = _finite_number(step.get("invalid_fraction_limit", 0.3),
                                           "invalid_fraction_limit")
            if not 0 <= invalid_limit <= 1:
                raise DisplayBlockedError("invalid_fraction_limit must be between zero and one")
            if np.count_nonzero(legacy_invalid) / legacy_invalid.size > invalid_limit:
                raise DisplayBlockedError("legacy palette rejected this image: too many unmatched pixels")
            has_palette = True
        elif op == "legacy_channel_decode":
            if has_palette or encoded or legacy_codes is not None or legacy_channel_method is not None:
                raise DisplayBlockedError("legacy channel decode must be the only source-value decoder")
            method = step.get("method")
            channel = step.get("channel")
            expected_channel = {"rainviewer_v2_red": 0, "windy_v2_green": 1}.get(method)
            if expected_channel is None or type(channel) is not int or channel != expected_channel:
                raise DisplayBlockedError("legacy channel decode method or channel is unsupported")
            channel_values = rgba[:, :, channel]
            if method == "rainviewer_v2_red":
                # Exact RainViewer v2 byte transform from the pinned old tile
                # downloader: subtract the 128 flag, discard the 0..32 band,
                # then remove the remaining 32 offset. The old code operated
                # on float32 channel values.
                decoded = channel_values.astype(np.float32)
                decoded[decoded >= 128] -= 128
                decoded[decoded <= 32] = 0
                decoded[decoded >= 32] -= 32
            else:
                # Windy's old green-channel payload stores twice dBZ and casts
                # to uint8 before its separate gray conversion.
                decoded = (channel_values.astype(np.float64) / 2).astype(np.uint8)
            values[:] = decoded
            mapped[:] = True  # The old tile converter discarded input alpha.
            legacy_channel_method = method
            has_palette = True
        elif op == "threshold":
            if not has_palette or encoded:
                raise DisplayBlockedError("threshold needs decoded palette values")
            low = _finite_number(step.get("min_dbz"), "min_dbz")
            high = _finite_number(step.get("max_dbz"), "max_dbz")
            if low > high or step.get("below") not in {"mask", "zero"} or step.get("above") not in {"mask", "cap"}:
                raise DisplayBlockedError("threshold requires explicit bounds and below/above behavior")
            below = mapped & (values < low)
            above = mapped & (values > high)
            if step["below"] == "zero":
                values[below] = 0
            else:
                rgba[below, 3] = 0
                mapped[below] = False
            if step["above"] == "cap":
                values[above] = high
            else:
                rgba[above, 3] = 0
                mapped[above] = False
        elif op in {"range_mask", "disk_mask"}:
            height, width = rgba.shape[:2]
            yy, xx = np.ogrid[:height, :width]
            if op == "range_mask":
                left, top, right, bottom = _geometry(step, "bounds", (height, width))
                outside = (xx < left) | (xx >= right) | (yy < top) | (yy >= bottom)
            else:
                if legacy_codes is not None and step.get("method") == "legacy_height_center_disk":
                    radius = (height - 1) / 2
                    inside = (xx - radius) ** 2 + (yy - radius) ** 2 <= radius ** 2
                    legacy_keep_mask &= inside
                    legacy_codes[~legacy_keep_mask] = 0
                    continue
                center = step.get("center")
                if not isinstance(center, (list, tuple)) or len(center) != 2:
                    raise DisplayBlockedError("disk mask requires explicit pixel center")
                cx = _finite_number(center[0], "disk cx")
                cy = _finite_number(center[1], "disk cy")
                radius = _finite_number(step.get("radius"), "disk radius")
                if radius <= 0:
                    raise DisplayBlockedError("disk radius must be positive")
                outside = (xx - cx)**2 + (yy - cy)**2 > radius**2
            rgba[outside, 3] = 0
            values[outside] = np.nan
            mapped[outside] = False
        elif op == "crop":
            left, top, right, bottom = _geometry(step, "bounds", rgba.shape[:2])
            rgba = rgba[top:bottom, left:right].copy()
            values = values[top:bottom, left:right].copy()
            mapped = mapped[top:bottom, left:right].copy()
        elif op == "legacy_crop":
            bounds = step.get("bounds")
            fill = step.get("pad_rgb")
            if (not isinstance(bounds, (list, tuple)) or len(bounds) != 4 or
                    any(type(value) is not int for value in bounds)):
                raise DisplayBlockedError("legacy crop requires exact integer Pillow crop bounds")
            left, top, right, bottom = bounds
            out_width, out_height = right - left, bottom - top
            if out_width <= 0 or out_height <= 0:
                raise DisplayBlockedError("legacy crop bounds must have positive extent")
            if (not isinstance(fill, (list, tuple)) or len(fill) != 3 or
                    any(type(channel) is not int or not 0 <= channel <= 255 for channel in fill)):
                raise DisplayBlockedError("legacy crop requires the old image mode's exact RGB pad color")
            if out_width * out_height > pixel_limit or out_width * out_height * 32 > temp_limit:
                raise DisplayBlockedError("legacy crop exceeds pixel or temporary memory limit")
            height, width = rgba.shape[:2]
            cropped = np.empty((out_height, out_width, 4), dtype=np.uint8)
            cropped[:, :, :3] = fill
            cropped[:, :, 3] = 255
            source_left, source_top = max(left, 0), max(top, 0)
            source_right, source_bottom = min(right, width), min(bottom, height)
            if source_left < source_right and source_top < source_bottom:
                dest_left, dest_top = source_left - left, source_top - top
                cropped[dest_top:dest_top + source_bottom - source_top,
                        dest_left:dest_left + source_right - source_left] = (
                    rgba[source_top:source_bottom, source_left:source_right]
                )
            rgba = cropped
            values = np.full((out_height, out_width), np.nan, dtype=np.float64)
            mapped = np.zeros((out_height, out_width), dtype=bool)
        elif op == "gap_repair":
            if legacy_codes is not None:
                if not has_palette or encoded or legacy_invalid is None:
                    raise DisplayBlockedError("legacy gap repair needs a preceding legacy palette")
                method = step.get("method")
                if method == "zero_invalid":
                    legacy_codes[legacy_invalid] = 0
                elif method == "opencv_inpaint":
                    try:
                        import cv2
                    except ImportError as exc:
                        raise DisplayBlockedError(
                            "legacy OpenCV inpaint requires the radiust recovery extra"
                        ) from exc
                    radius = _finite_number(step.get("radius"), "inpaint radius")
                    if radius <= 0:
                        raise DisplayBlockedError("inpaint radius must be positive")
                    flags = {"NS": cv2.INPAINT_NS, "TELEA": cv2.INPAINT_TELEA}
                    method_name = step.get("inpaint_method")
                    if method_name not in flags:
                        raise DisplayBlockedError("legacy OpenCV inpaint method is unsupported")
                    try:
                        legacy_codes = cv2.inpaint(
                            legacy_codes.astype(np.uint8), legacy_invalid.astype(np.uint8),
                            radius, flags[method_name],
                        ).astype(np.uint16)
                    except cv2.error as exc:
                        raise DisplayBlockedError("legacy OpenCV inpaint failed") from exc
                else:
                    raise DisplayBlockedError("unsupported legacy gap repair method")
                continue
            if not has_palette or encoded or step.get("method") != "four_equal_neighbors":
                raise DisplayBlockedError("unsupported gap repair; exact source method evidence required")
            passes = _positive_int(step.get("max_passes"), "gap max_passes")
            if passes > 8:
                raise DisplayBlockedError("gap repair iteration budget exceeded")
            for _ in range(passes):
                missing = rgba[:, :, 3] == 0
                repair = np.zeros_like(missing)
                interior = (missing[1:-1, 1:-1] & mapped[:-2, 1:-1] & mapped[2:, 1:-1] &
                            mapped[1:-1, :-2] & mapped[1:-1, 2:] &
                            (values[:-2, 1:-1] == values[2:, 1:-1]) &
                            (values[:-2, 1:-1] == values[1:-1, :-2]) &
                            (values[:-2, 1:-1] == values[1:-1, 2:]))
                repair[1:-1, 1:-1] = interior
                if not np.any(repair):
                    break
                neighbors = np.roll(values, 1, axis=0)
                values[repair] = neighbors[repair]
                rgba[repair, 3] = 255
                mapped[repair] = True
        elif op == "gray_encode":
            if not has_palette or encoded:
                raise DisplayBlockedError("gray encoding requires one provenance-bound palette")
            if legacy_codes is not None:
                if step.get("method") != "legacy_cookbook" or legacy_cookbook is None:
                    raise DisplayBlockedError("legacy palette requires explicit cookbook encoding")
                legacy_codes[~legacy_keep_mask] = 0
                if np.any(legacy_codes >= len(legacy_cookbook)):
                    raise DisplayBlockedError("inpainted legacy code exceeds the source cookbook")
                mapped_values = legacy_cookbook[legacy_codes]
                mapped_values[~legacy_keep_mask] = 0
                gray_values = np.multiply(mapped_values, np.float32(3.2))
                gray = np.clip(gray_values, 0, 224).astype(np.uint8)
                rgba[:, :, 3] = 255
            elif legacy_channel_method is not None:
                if step.get("method") != "legacy_tile_uint8_then_clip_224":
                    raise DisplayBlockedError("legacy tile channel decode requires its exact byte encoding")
                arithmetic_dtype = step.get("arithmetic_dtype")
                if arithmetic_dtype == "float32":
                    scaled = values.astype(np.float32) / 5 * 16
                elif arithmetic_dtype == "float64":
                    scaled = values.astype(np.float64) / 5 * 16
                else:
                    raise DisplayBlockedError("legacy tile gray encoding requires the old arithmetic dtype")
                # The pinned tile code cast to uint8 before clipping. Preserve
                # that historical wrap behavior for values above 255.
                gray = np.clip(scaled.astype(np.uint8), 0, 224).astype(np.uint8)
                rgba[:, :, 3] = 255
            else:
                gray = np.clip(np.nan_to_num(values, nan=0.0) * 16 / 5, 0, 224).astype(np.uint8)
            rgba[:, :, :3] = gray[:, :, None]
            encoded = True
        elif op == "legacy_luminance_alpha":
            if has_palette or encoded or legacy_codes is not None:
                raise DisplayBlockedError("legacy luminance conversion must be an explicit standalone encoding")
            rgb = rgba[:, :, :3].astype(np.float32)
            alpha = rgba[:, :, 3].astype(np.float32) / np.float32(255.0)
            luminance = (
                np.float32(0.299) * rgb[:, :, 0]
                + np.float32(0.587) * rgb[:, :, 1]
                + np.float32(0.114) * rgb[:, :, 2]
            )
            luminance *= alpha
            gray = np.clip(
                luminance / np.float32(255.0) * np.float32(224.0),
                np.float32(0.0), np.float32(224.0),
            ).astype(np.uint8)
            rgba[:, :, :3] = gray[:, :, None]
            rgba[:, :, 3] = 255
            encoded = True
        elif op == "resize":
            shape = step.get("shape")
            method = step.get("method")
            if (not isinstance(shape, (list, tuple)) or len(shape) != 2 or
                    method not in {"nearest", "pillow_bicubic"}):
                raise DisplayBlockedError("resize requires exact dimensions and supported source method")
            out_width = _positive_int(shape[0], "resize width")
            out_height = _positive_int(shape[1], "resize height")
            if out_width * out_height > pixel_limit or out_width * out_height * 32 > temp_limit:
                raise DisplayBlockedError("legacy display resize exceeds resource limit")
            size = (out_width, out_height)
            resample = Image.Resampling.NEAREST if method == "nearest" else Image.Resampling.BICUBIC
            rgba = np.asarray(Image.fromarray(rgba, "RGBA").resize(size, resample)).copy()
            if legacy_codes is None:
                values = np.asarray(Image.fromarray(values.astype(np.float32), "F").resize(size, resample)).copy()
                mask_resample = Image.Resampling.NEAREST if method == "nearest" else Image.Resampling.BICUBIC
                mapped = np.asarray(Image.fromarray(mapped.astype(np.uint8), "L").resize(size, mask_resample)).astype(bool)
        else:
            raise DisplayBlockedError(f"unsupported legacy display step: {op}")

    if not encoded:
        raise DisplayBlockedError("legacy display rule ended without explicit gray encoding")
    return rgba


__all__ = ["DisplayBlockedError", "render_legacy_display"]
