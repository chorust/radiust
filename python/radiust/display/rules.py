"""Evidence-bound legacy *display* rules; no scientific decoding or auto-enable.

These objects describe a recorded old display procedure and its comparison
evidence.  A status supplied by a rule file is not by itself verification:
the display registry must also check a separately reproduced passed evidence.
"""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

ENCODING_VERSION = "旧项目-gray-dbz-v1"
STATUSES = frozenset({"blocked", "difference_pending", "passed"})
STEP_OPERATIONS = frozenset({
    "color_preprocess", "palette", "zero_palette", "threshold", "background_mask",
    "range_mask", "disk_mask", "crop", "legacy_crop", "legacy_palette", "legacy_luminance_alpha",
    "gap_repair", "legacy_channel_decode", "resize", "gray_encode",
})
_SHA256 = re.compile(r"[a-f0-9]{64}\Z")
_RULE_FINGERPRINT_FIELDS = (
    "source", "product", "path_id", "rule_version", "encoding_version",
    "legacy_reference", "ordered_steps", "input_constraints",
)


def _nonempty(value: object, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{name} must be a non-empty string")
    return value


def _hash(value: object, name: str) -> str:
    if not isinstance(value, str) or _SHA256.fullmatch(value) is None:
        raise ValueError(f"{name} must be a lowercase SHA-256 hex digest")
    return value


def _optional_hash(value: object, name: str) -> str | None:
    return None if value is None else _hash(value, name)


def rule_fingerprint(value: Mapping[str, Any]) -> str:
    """Fingerprint the exact algorithm, source identity and input constraints.

    Verification status and evidence are intentionally excluded: accepting a
    previous sample must never change the implementation fingerprint itself.
    """
    try:
        canonical = {name: value[name] for name in _RULE_FINGERPRINT_FIELDS}
        encoded = json.dumps(canonical, ensure_ascii=False, sort_keys=True,
                             separators=(",", ":"), allow_nan=False).encode("utf-8")
    except (KeyError, TypeError, ValueError) as exc:
        raise ValueError("legacy display rule has incomplete or non-JSON configuration") from exc
    return hashlib.sha256(encoded).hexdigest()


def merge_ordered_steps(
    base_steps: list[Mapping[str, Any]],
    product_overrides: list[Mapping[str, Any]],
) -> list[dict[str, Any]]:
    """Apply evidenced product overrides at explicit positions in the old order.

    Base and product steps have stable `step_id` values. A product must name
    each replaced/removed step or the insertion anchor; a missing id or an
    implicit reorder is a configuration error, never a reason to drop a step.
    The returned list is an independent copy suitable for rule fingerprinting.
    """
    if not isinstance(base_steps, list) or not base_steps:
        raise ValueError("legacy display base steps must be a nonempty array")
    if not isinstance(product_overrides, list):
        raise ValueError("legacy display product overrides must be an explicit array")
    steps: list[dict[str, Any]] = []
    ids: set[str] = set()
    for step in base_steps:
        if not isinstance(step, Mapping):
            raise ValueError("legacy display base step must be an object")
        step_id = _nonempty(step.get("step_id"), "base step_id")
        if step_id in ids or step.get("op") not in STEP_OPERATIONS:
            raise ValueError("legacy display base steps must have unique supported step identities")
        _nonempty(step.get("basis"), "base step basis")
        ids.add(step_id)
        steps.append(dict(step))
    touched: set[str] = set()
    for override in product_overrides:
        if not isinstance(override, Mapping):
            raise ValueError("legacy display override must be an object")
        action = override.get("action")
        anchor = _nonempty(override.get("target"), "product override target")
        basis = _nonempty(override.get("basis"), "product override basis")
        if anchor not in ids or anchor in touched:
            raise ValueError("product override refers to absent or already modified step")
        touched.add(anchor)
        position = next(i for i, step in enumerate(steps) if step["step_id"] == anchor)
        if action == "remove":
            if "step" in override:
                raise ValueError("remove override cannot contain a replacement step")
            steps.pop(position)
            ids.remove(anchor)
            continue
        if action not in {"replace", "insert_before", "insert_after"}:
            raise ValueError("legacy display product override action is unsupported")
        replacement = override.get("step")
        if not isinstance(replacement, Mapping) or replacement.get("op") not in STEP_OPERATIONS:
            raise ValueError("legacy display product override requires a supported step")
        new_id = _nonempty(replacement.get("step_id"), "product step_id")
        _nonempty(replacement.get("basis"), "product step basis")
        if new_id in ids and (action != "replace" or new_id != anchor):
            raise ValueError("product override cannot shadow another display step")
        new_step = dict(replacement)
        # Preserve the provenance of a product's choice to replace or insert.
        new_step["override_basis"] = basis
        if action == "replace":
            steps[position] = new_step
            ids.remove(anchor)
        else:
            steps.insert(position + int(action == "insert_after"), new_step)
        ids.add(new_id)
    if not steps:
        raise ValueError("product overrides removed all legacy display steps")
    return steps


@dataclass(frozen=True, slots=True)
class LegacyDisplayRule:
    source: str
    product: str
    path_id: str
    rule_version: str
    encoding_version: str
    legacy_reference: str
    config_hash: str
    ordered_steps: tuple[Mapping[str, Any], ...]
    input_constraints: Mapping[str, Any]
    validation_status: str

    @classmethod
    def from_mapping(cls, value: Mapping[str, Any]) -> LegacyDisplayRule:
        source = _nonempty(value.get("source"), "source")
        product = _nonempty(value.get("product"), "product")
        path_id = _nonempty(value.get("path_id"), "path_id")
        if path_id != f"{source}/{product}" and not path_id.startswith(f"{source}/{product}/"):
            raise ValueError("path_id must belong to the declared source and product")
        version = _nonempty(value.get("rule_version"), "rule_version")
        encoding = value.get("encoding_version")
        if encoding != ENCODING_VERSION:
            raise ValueError("unknown legacy display encoding version")
        reference = _nonempty(value.get("legacy_reference"), "legacy_reference")
        steps = value.get("ordered_steps")
        if not isinstance(steps, (list, tuple)) or not steps:
            raise ValueError("ordered_steps must explicitly document the old operation sequence")
        verified_steps: list[dict[str, Any]] = []
        for step in steps:
            if (not isinstance(step, Mapping) or not isinstance(step.get("op"), str)
                    or step.get("op") not in STEP_OPERATIONS):
                raise ValueError("legacy display step is unsupported or unidentified")
            _nonempty(step.get("basis"), "ordered_steps[].basis")
            verified_steps.append(dict(step))
        constraints = value.get("input_constraints")
        if not isinstance(constraints, Mapping) or not constraints:
            raise ValueError("input_constraints must explicitly identify admissible input")
        status = value.get("validation_status")
        if not isinstance(status, str) or status not in STATUSES:
            raise ValueError("unknown display validation status")
        fingerprint = _hash(value.get("config_hash"), "config_hash")
        if rule_fingerprint(value) != fingerprint:
            raise ValueError("display configuration fingerprint mismatch")
        return cls(source, product, path_id, version, encoding, reference, fingerprint,
                   tuple(verified_steps), dict(constraints), status)

    def matches(self, source: str, product: str, path_id: str, version: str) -> bool:
        return (source, product, path_id, version) == (
            self.source, self.product, self.path_id, self.rule_version)

    def fingerprint_matches(self) -> bool:
        """Check mutable nested settings again at the point of use."""
        try:
            return self.config_hash == rule_fingerprint({
                name: getattr(self, name) for name in _RULE_FINGERPRINT_FIELDS
            })
        except (TypeError, ValueError, AttributeError):
            return False


@dataclass(frozen=True, slots=True)
class DisplayEvidence:
    path_id: str
    status: str
    rule_version: str | None
    config_hash: str | None
    input_hashes: tuple[str, ...]
    output_hash: str | None
    baseline_identity: str | None
    baseline_hash: str | None
    crop: tuple[int, int, int, int] | None
    shape: tuple[int, int] | None
    pixel_diff_count: int | None
    alpha_comparison: str | None
    background_comparison: str | None
    missing_comparison: str | None
    intentional_differences: tuple[str | Mapping[str, Any], ...]
    review_conclusion: str | None
    blocked_reasons: tuple[str, ...]
    sample_provenance: str | None
    scientific_status_unchanged: bool

    @classmethod
    def from_mapping(cls, value: Mapping[str, Any]) -> DisplayEvidence:
        path_id = _nonempty(value.get("path_id"), "path_id")
        status = value.get("status")
        if not isinstance(status, str) or status not in STATUSES:
            raise ValueError("unknown display evidence status")
        if value.get("scientific_status_unchanged") is not True:
            raise ValueError("display evidence cannot change scientific status")
        hashes = value.get("input_hashes", [])
        if not isinstance(hashes, (tuple, list)):
            raise ValueError("input_hashes must be an array")
        fingerprints = tuple(_hash(item, "input_hashes[]") for item in hashes)
        reasons = value.get("blocked_reasons", [])
        differences = value.get("intentional_differences", [])
        if not isinstance(reasons, (list, tuple)) or not isinstance(differences, (list, tuple)):
            raise ValueError("blocked_reasons and intentional_differences must be arrays")
        checked_reasons = tuple(_nonempty(item, "blocked_reasons[]") for item in reasons)
        checked_differences: tuple[str | Mapping[str, Any], ...] = tuple(
            _nonempty(item, "intentional_differences[]") if isinstance(item, str) else
            dict(item) if isinstance(item, Mapping) else
            _nonempty(item, "intentional_differences[]")
            for item in differences
        )
        crop = value.get("crop")
        if crop is not None and (not isinstance(crop, (tuple, list)) or len(crop) != 4 or
                                 any(type(item) is not int or item < 0 for item in crop)):
            raise ValueError("crop must be four nonnegative integer coordinates")
        shape = value.get("shape")
        if shape is not None and (not isinstance(shape, (tuple, list)) or len(shape) != 2 or
                                  any(type(item) is not int or item <= 0 for item in shape)):
            raise ValueError("shape must be two positive integer dimensions")
        diff_count = value.get("pixel_diff_count")
        if diff_count is not None and (type(diff_count) is not int or diff_count < 0):
            raise ValueError("pixel_diff_count must be nonnegative")
        rule_version = value.get("rule_version")
        baseline_identity = value.get("baseline_identity")
        provenance = value.get("sample_provenance")
        conclusion = value.get("review_conclusion")
        alpha = value.get("alpha_comparison")
        background = value.get("background_comparison")
        missing = value.get("missing_comparison")
        if status == "blocked" and not checked_reasons:
            raise ValueError("blocked display paths must enumerate missing evidence")
        if status == "passed":
            if checked_reasons:
                raise ValueError("passed display path still contains blockers")
            for name, item in (("rule_version", rule_version), ("baseline_identity", baseline_identity),
                               ("sample_provenance", provenance), ("review_conclusion", conclusion),
                               ("alpha_comparison", alpha), ("background_comparison", background),
                               ("missing_comparison", missing)):
                _nonempty(item, name)
            if not fingerprints or crop is None or shape is None or diff_count is None:
                raise ValueError("passed display evidence must include input, crop, dimensions and pixel comparison")
            if (diff_count or any(item != "identical" for item in (alpha, background, missing))) and (
                not checked_differences or not str(conclusion).startswith("accepted:") or
                any(not isinstance(item, Mapping) or item.get("accepted") is not True or
                    any(not isinstance(item.get(name), str) or not item[name].strip()
                        for name in ("reason", "impact", "reviewer")) for item in checked_differences)
            ):
                raise ValueError("display differences need explicit recorded review acceptance")
        elif status == "difference_pending" and not (diff_count or checked_differences):
            raise ValueError("difference_pending must identify a measured or described difference")
        return cls(
            path_id, status, rule_version, _optional_hash(value.get("config_hash"), "config_hash"),
            fingerprints, _optional_hash(value.get("output_hash"), "output_hash"),
            baseline_identity, _optional_hash(value.get("baseline_hash"), "baseline_hash"),
            tuple(crop) if crop is not None else None, tuple(shape) if shape is not None else None,
            diff_count, alpha, background, missing, checked_differences, conclusion,
            checked_reasons, provenance, True,
        )._validated()

    def _validated(self) -> DisplayEvidence:
        if self.status == "passed" and (not self.config_hash or not self.output_hash or not self.baseline_hash):
            raise ValueError("passed display evidence requires rule, output and baseline SHA-256")
        return self

    def validates(self, rule: LegacyDisplayRule) -> bool:
        return (self.status == "passed" and self.path_id == rule.path_id and
                self.rule_version == rule.rule_version and self.config_hash == rule.config_hash and
                self.scientific_status_unchanged and rule.fingerprint_matches())


__all__ = ["DisplayEvidence", "ENCODING_VERSION", "LegacyDisplayRule", "merge_ordered_steps", "rule_fingerprint"]
