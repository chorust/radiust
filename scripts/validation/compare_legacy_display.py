"""Offline, evidence-gated source display comparison; never contacts providers.

Manifest paths are relative to the manifest's fixture root (or --fixture-root).
The script never treats a synthetic mechanism fixture as a provider golden.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from dataclasses import replace
from datetime import datetime
from pathlib import Path
from typing import Any

import numpy as np
from radiust.display.engine import render_legacy_display
from radiust.display.raw import preview_bytes
from radiust.display.rules import DisplayEvidence, LegacyDisplayRule
from radiust.display.tiles import compose_verified_tiles
from radiust.models import Artifact, FrameRef
from radiust.raw import RawFrame

MAX_JSON = 4 * 1024 * 1024
MAX_INPUT_BYTES = 512 * 1024 * 1024
MAX_PIXELS = 100_000_000
SHA256 = re.compile(r"[a-f0-9]{64}\Z")


class InvalidFixture(ValueError):
    """The offline fixture cannot establish a source-matched comparison."""


def _text(value: object, name: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise InvalidFixture(f"missing or invalid {name}")
    return value


def _hash(value: object, name: str) -> str:
    if not isinstance(value, str) or SHA256.fullmatch(value) is None:
        raise InvalidFixture(f"missing or invalid {name} SHA-256")
    return value


def _path(root: Path, relative: object) -> Path:
    """Refuse absolute/URL/traversal paths even when normalized within root."""
    path = Path(_text(relative, "fixture path"))
    if path.is_absolute() or ".." in path.parts or ":" in str(path) or "\\" in str(path):
        raise InvalidFixture("fixture path is not a confined relative file path")
    resolved = (root / path).resolve()
    if not resolved.is_relative_to(root) or not resolved.is_file():
        raise InvalidFixture("fixture path escapes its root or required file is absent")
    return resolved


def _read(root: Path, path: object, *, max_bytes: int) -> bytes:
    file = _path(root, path)
    if file.stat().st_size > max_bytes:
        raise InvalidFixture("fixture file exceeds byte limit")
    with file.open("rb") as stream:
        content = stream.read(max_bytes + 1)
    if len(content) > max_bytes:
        raise InvalidFixture("fixture file exceeds byte limit")
    return content


def _json(root: Path, path: object) -> dict[str, Any]:
    try:
        content = json.loads(_read(root, path, max_bytes=MAX_JSON))
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise InvalidFixture("fixture JSON is invalid") from exc
    if not isinstance(content, dict):
        raise InvalidFixture("fixture JSON must be an object")
    return content


def _preview(data: bytes, name: str, *, media_format: str) -> np.ndarray:
    if media_format not in {"PNG", "GIF"}:
        raise InvalidFixture("single raw preview requires PNG or GIF")
    preview = preview_bytes(data, name=name,
                            limits={"max_artifact_bytes": MAX_INPUT_BYTES, "max_pixels": MAX_PIXELS})
    if preview.format != media_format:
        raise InvalidFixture("fixture declared format differs from image bytes")
    return preview.rgba


def _composed(entry: dict[str, Any], inputs: list[tuple[dict[str, Any], bytes]], rule: LegacyDisplayRule,
              provisional: DisplayEvidence) -> tuple[np.ndarray, str]:
    if len(inputs) == 1:
        item, payload = inputs[0]
        return _preview(payload, _text(item.get("name"), "input name"),
                        media_format=_text(item.get("format"), "input format")), item["format"]
    plan = rule.input_constraints.get("tile_plan")
    if not isinstance(plan, dict):
        raise InvalidFixture("multi-artifact input lacks a verified tile plan")
    frame = entry.get("frame")
    if not isinstance(frame, dict):
        raise InvalidFixture("multi-artifact fixture has no bound frame identity")
    if frame.get("source") != rule.source or frame.get("product") != rule.product:
        raise InvalidFixture("frame identity differs from display rule")
    revision = _text(frame.get("revision"), "frame revision")
    timestamp = datetime.fromisoformat(_text(frame.get("valid_time"), "frame UTC time").replace("Z", "+00:00"))
    if timestamp.tzinfo is None or timestamp.utcoffset() is None:
        raise InvalidFixture("frame time requires explicit timezone")
    artifacts = tuple(Artifact(
        _text(item.get("name"), "tile name"), _text(item.get("role"), "tile role"),
        _text(item.get("media_type"), "tile media type"), payload, source_revision=revision,
    ) for item, payload in inputs)
    ref = FrameRef(rule.source, rule.product, timestamp, station=frame.get("station"), revision=revision)
    with RawFrame(ref, artifacts, metadata={
        "tile_layout": plan.get("layout"), "tile_count": len(artifacts),
        "tile_size": plan.get("tile_width"),
    }) as raw:
        rgba = compose_verified_tiles(raw, rule, provisional, limits={"max_pixels": MAX_PIXELS})
    input_format = inputs[0][0].get("format")
    if any(item.get("format") != input_format for item, _ in inputs):
        raise InvalidFixture("mixed tile formats require a separately validated source rule")
    return rgba, input_format


def _provisional_evidence(entry: dict[str, Any], rule: LegacyDisplayRule,
                          input_hashes: list[str], baseline_hash: str) -> DisplayEvidence:
    """Internal execution gate only; `passed` is decided by real pixel comparison."""
    return DisplayEvidence.from_mapping({
        "path_id": rule.path_id, "status": "passed", "rule_version": rule.rule_version,
        "config_hash": rule.config_hash, "input_hashes": input_hashes,
        "output_hash": baseline_hash, "baseline_identity": entry["baseline_identity"],
        "baseline_hash": baseline_hash, "crop": [0, 0, 1, 1], "shape": [1, 1],
        "pixel_diff_count": 0, "alpha_comparison": "identical",
        "background_comparison": "identical", "missing_comparison": "identical",
        "intentional_differences": [], "review_conclusion": "provisional: pending offline comparison",
        "blocked_reasons": [], "sample_provenance": entry["sample_provenance"],
        "scientific_status_unchanged": True,
    })


def _compare(actual: np.ndarray, baseline: np.ndarray) -> dict[str, Any]:
    result: dict[str, Any] = {"shape": list(actual.shape[:2]), "baseline_shape": list(baseline.shape[:2]),
                              "output_hash": hashlib.sha256(actual.tobytes()).hexdigest()}
    if actual.shape != baseline.shape:
        return {**result, "pixel_diff_count": None, "alpha_comparison": "shape differs",
                "background_comparison": "shape differs", "missing_comparison": "shape differs",
                "alpha_diff_count": None, "background_diff_count": None, "missing_diff_count": None}
    result["pixel_diff_count"] = int(np.count_nonzero(np.any(actual != baseline, axis=2)))
    result["alpha_diff_count"] = int(np.count_nonzero(actual[:, :, 3] != baseline[:, :, 3]))
    result["missing_diff_count"] = int(np.count_nonzero((actual[:, :, 3] == 0) != (baseline[:, :, 3] == 0)))
    zero_actual = (actual[:, :, 3] != 0) & np.all(actual[:, :, :3] == 0, axis=2)
    zero_baseline = (baseline[:, :, 3] != 0) & np.all(baseline[:, :, :3] == 0, axis=2)
    result["background_diff_count"] = int(np.count_nonzero(zero_actual != zero_baseline))
    for label in ("alpha", "missing", "background"):
        result[f"{label}_comparison"] = "identical" if result[f"{label}_diff_count"] == 0 else "different"
    return result


def _entry_result(entry: dict[str, Any], root: Path) -> dict[str, Any]:
    source = _text(entry.get("source"), "source")
    product = _text(entry.get("product"), "product")
    path_id = _text(entry.get("path_id"), "path_id")
    if path_id != f"{source}/{product}" and not path_id.startswith(f"{source}/{product}/"):
        raise InvalidFixture("path_id does not belong to source/product")
    result: dict[str, Any] = {
        "source": source, "product": product, "path_id": path_id,
        "status": "blocked", "blocked_reasons": [], "scientific_status_unchanged": True,
        "rule_version": None, "config_hash": None, "input_hashes": [], "output_hash": None,
        "baseline_identity": None, "baseline_hash": None,
    }
    if entry.get("status") == "blocked":
        reasons = entry.get("blocked_reasons")
        if not isinstance(reasons, list) or not reasons:
            raise InvalidFixture("blocked entry must enumerate missing materials")
        result["blocked_reasons"] = [_text(reason, "blocked reason") for reason in reasons]
        if entry.get("rule_path") is not None or entry.get("display_disposition_path") is not None:
            try:
                if entry.get("rule_path") is not None:
                    rule = LegacyDisplayRule.from_mapping(_json(root, entry.get("rule_path")))
                    version = _text(entry.get("rule_version"), "blocked rule version")
                    config_hash = _hash(entry.get("config_hash"), "blocked rule configuration")
                    if (not rule.matches(source, product, path_id, version) or
                            rule.config_hash != config_hash or rule.validation_status != "blocked"):
                        raise InvalidFixture("blocked rule identity, fingerprint or status differs from manifest")
                    result["rule_version"], result["config_hash"] = rule.rule_version, rule.config_hash
                if entry.get("display_disposition_path") is not None:
                    disposition = _json(root, entry.get("display_disposition_path"))
                    expected = (source, product, path_id)
                    actual = tuple(disposition.get(key) for key in ("source", "product", "path_id"))
                    disposition_reasons = disposition.get("blocked_reasons")
                    if (disposition.get("kind") != "no_legacy_gray_transform" or
                            disposition.get("status") != "blocked" or actual != expected or
                            disposition.get("schema_version") != 1 or
                            disposition.get("source_sample_retained") is not False or
                            disposition.get("scientific_status_unchanged") is not True or
                            disposition_reasons != result["blocked_reasons"]):
                        raise InvalidFixture("blocked no-transform disposition differs from manifest identity or evidence")
                    _text(disposition.get("old_commit"), "disposition old commit")
                    _text(disposition.get("legacy_reference"), "disposition legacy reference")
                    _text(disposition.get("verified_code_fact"), "disposition code fact")
                    result["display_disposition"] = disposition["kind"]
            except (InvalidFixture, ValueError, OSError, KeyError, TypeError) as exc:
                result["blocked_reasons"] = ["blocked rule metadata could not be verified"]
                result["validation_issue"] = type(exc).__name__
        return result
    if entry.get("status") not in {"candidate", "difference_pending", "passed"}:
        raise InvalidFixture("manifest entry status is unrecognized")
    try:
        result["baseline_identity"] = _text(entry.get("baseline_identity"), "old baseline identity")
        result["baseline_hash"] = _hash(entry.get("baseline_hash"), "old baseline")
        _text(entry.get("license_basis"), "lawful sample basis")
        result["license_basis"] = _text(entry.get("license_basis"), "lawful sample basis")
        result["sample_provenance"] = _text(entry.get("sample_provenance"), "canonical raw provenance")
        rule = LegacyDisplayRule.from_mapping(_json(root, entry.get("rule_path")))
        if not rule.matches(source, product, path_id, _text(entry.get("rule_version"), "rule version")):
            raise InvalidFixture("display rule identity or version differs from manifest")
        if rule.config_hash != _hash(entry.get("config_hash"), "rule configuration"):
            raise InvalidFixture("display rule hash differs from manifest")
        result["rule_version"], result["config_hash"] = rule.rule_version, rule.config_hash
        baseline_bytes = _read(root, entry.get("baseline_path"), max_bytes=MAX_INPUT_BYTES)
        if hashlib.sha256(baseline_bytes).hexdigest() != result["baseline_hash"]:
            raise InvalidFixture("old baseline SHA-256 does not match")
        baseline = _preview(baseline_bytes, str(entry["baseline_path"]), media_format="PNG")
        declared_inputs = entry.get("inputs")
        if not isinstance(declared_inputs, list) or not declared_inputs:
            raise InvalidFixture("manifest requires at least one source-matched input")
        inputs: list[tuple[dict[str, Any], bytes]] = []
        for item in declared_inputs:
            if not isinstance(item, dict):
                raise InvalidFixture("raw artifact entry is invalid")
            data = _read(root, item.get("path"), max_bytes=MAX_INPUT_BYTES)
            if hashlib.sha256(data).hexdigest() != _hash(item.get("sha256"), "raw input"):
                raise InvalidFixture("raw input SHA-256 does not match")
            inputs.append((item, data))
        result["input_hashes"] = [item["sha256"] for item, _ in inputs]
        rule_for_test = replace(rule, validation_status="passed")
        provisional = _provisional_evidence(entry, rule_for_test, result["input_hashes"], result["baseline_hash"])
        rgba, media_format = _composed(entry, inputs, rule_for_test, provisional)
        crop = entry.get("crop")
        if (not isinstance(crop, list) or len(crop) != 4 or
                any(type(value) is not int or value < 0 for value in crop)):
            raise InvalidFixture("old display crop or explicit full-image extent is absent")
        result["crop"] = crop
        actual = render_legacy_display(rgba, rule_for_test, provisional,
                                       input_format=media_format, limits={"max_pixels": MAX_PIXELS})
        comparison = _compare(actual, baseline)
        result.update(comparison)
        differences = entry.get("intentional_differences", [])
        result["intentional_differences"] = differences
        exact = comparison["pixel_diff_count"] == 0 and comparison["shape"] == comparison["baseline_shape"]
        accepted = (
            isinstance(differences, list) and bool(differences) and
            all(isinstance(d, dict) and all(isinstance(d.get(k), str) and d[k].strip()
                for k in ("reason", "impact", "reviewer"))
                and d.get("accepted") is True for d in differences)
            and isinstance(entry.get("review_conclusion"), str)
            and entry["review_conclusion"].startswith("accepted:")
        ) if not exact else False
        if exact or accepted:
            result["status"] = "passed"
            result["review_conclusion"] = "exact pixel agreement" if exact else entry["review_conclusion"]
        else:
            result["status"] = "difference_pending"
            result["review_conclusion"] = "differences require review and explicit acceptance"
    except (InvalidFixture, ValueError, OSError, KeyError, TypeError) as exc:
        # A path with absent or invalid evidence stays visible. A provider
        # exception or arbitrary raw response is never copied into the report.
        result["status"] = "blocked"
        result["blocked_reasons"] = ["offline rule, source input, or old baseline could not be verified"]
        result["validation_issue"] = type(exc).__name__
    return result


def compare_manifest(manifest: dict[str, Any], root: Path) -> dict[str, Any]:
    if manifest.get("schema_version") != 1 or not isinstance(manifest.get("entries"), list):
        raise InvalidFixture("manifest must have schema_version=1 and an entries array")
    if not manifest["entries"]:
        raise InvalidFixture("empty manifest cannot establish source coverage")
    records: list[dict[str, Any]] = []
    seen: set[str] = set()
    for entry in manifest["entries"]:
        if not isinstance(entry, dict):
            raise InvalidFixture("manifest entry must be an object")
        result = _entry_result(entry, root)
        if result["path_id"] in seen:
            raise InvalidFixture("duplicate display path_id in manifest")
        seen.add(result["path_id"])
        records.append(result)
    counts = {status: sum(item["status"] == status for item in records)
              for status in ("passed", "difference_pending", "blocked")}
    return {"schema_version": 1, "coverage_status": manifest.get("coverage_status", "unverified"),
            "counts": {"total": len(records), **counts}, "paths": records,
            "scientific_status_unchanged": True}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path("tests/fixtures/legacy-display/manifest.json"))
    parser.add_argument("--fixture-root", type=Path, default=None)
    parser.add_argument("--report", type=Path, default=Path("validation-results/legacy-display.json"))
    args = parser.parse_args(argv)
    root = (args.fixture_root or args.manifest.parent).resolve()
    try:
        manifest_file = args.manifest.resolve()
        if not manifest_file.is_relative_to(root):
            raise InvalidFixture("manifest is outside the specified fixture root")
        manifest = json.loads(_read(root, manifest_file.relative_to(root).as_posix(), max_bytes=MAX_JSON))
        if not isinstance(manifest, dict):
            raise InvalidFixture("manifest must be a JSON object")
        report = compare_manifest(manifest, root)
    except (InvalidFixture, json.JSONDecodeError, OSError) as exc:
        print(f"invalid offline display manifest: {type(exc).__name__}", file=sys.stderr)
        return 2
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"legacy display: {report['counts']}; report={args.report}")
    return 0 if report["counts"]["passed"] == report["counts"]["total"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
