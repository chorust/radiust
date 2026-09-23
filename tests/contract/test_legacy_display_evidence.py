"""Synthetic rule/evidence fixtures exercise the contract, never source goldens."""

import hashlib
import json
import subprocess
import sys
from io import BytesIO
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest
from PIL import Image
from radiust.display.registry import LegacyDisplayRegistry
from radiust.display.rules import DisplayEvidence, LegacyDisplayRule, rule_fingerprint

ROOT = Path(__file__).parents[2]
HASH = hashlib.sha256(b"synthetic-only").hexdigest()


def rule_data(**changes):
    value = {
        "source": "example", "product": "composite", "path_id": "example/composite",
        "rule_version": "old-config-v1", "encoding_version": "旧项目-gray-dbz-v1",
        "legacy_reference": "test-only: old display configuration excerpt (no provider claim)",
        "ordered_steps": [{"op": "crop", "bounds": [0, 0, 2, 2], "basis": "synthetic fixture"}],
        "input_constraints": {"formats": ["PNG"], "max_pixels": 4},
        "validation_status": "blocked",
    }
    value.update(changes)
    value["config_hash"] = rule_fingerprint(value)
    return value


def evidence_data(**changes):
    value = {
        "path_id": "example/composite", "status": "blocked",
        "rule_version": None, "config_hash": None, "input_hashes": [],
        "output_hash": None, "baseline_identity": None, "baseline_hash": None,
        "crop": None, "shape": None, "pixel_diff_count": None,
        "alpha_comparison": None, "background_comparison": None,
        "missing_comparison": None, "intentional_differences": [],
        "review_conclusion": None, "sample_provenance": None,
        "blocked_reasons": ["missing legally sourced paired old output"],
        "scientific_status_unchanged": True,
    }
    value.update(changes)
    return value


def test_rule_requires_exact_identity_version_order_and_fingerprint():
    original = rule_data()
    rule = LegacyDisplayRule.from_mapping(original)
    assert rule.matches("example", "composite", "example/composite", "old-config-v1")
    assert not rule.matches("example", "rain", "example/composite", "old-config-v1")
    assert not rule.matches("example", "composite", "example/composite", "old-config-v2")
    assert [step["op"] for step in rule.ordered_steps] == ["crop"]
    for changed in (
        {**original, "source": "other"},
        {**original, "ordered_steps": [{"op": "resize", "shape": [2, 2], "basis": "test"}]},
        {**original, "input_constraints": {"formats": ["GIF"]}},
    ):
        with pytest.raises(ValueError):
            LegacyDisplayRule.from_mapping(changed)


@pytest.mark.parametrize("steps", [[], [{"op": "invented", "basis": "test"}],
                                    [{"op": "crop", "bounds": [0, 0, 2, 2]}]])
def test_rule_rejects_implicit_or_unproven_transformation(steps):
    with pytest.raises(ValueError):
        LegacyDisplayRule.from_mapping(rule_data(ordered_steps=steps))


def test_evidence_blocked_and_pending_cannot_be_declared_passed():
    assert DisplayEvidence.from_mapping(evidence_data()).status == "blocked"
    with pytest.raises(ValueError):
        DisplayEvidence.from_mapping(evidence_data(blocked_reasons=[]))
    with pytest.raises(ValueError):
        DisplayEvidence.from_mapping(evidence_data(status="passed", blocked_reasons=[]))
    with pytest.raises(ValueError):
        DisplayEvidence.from_mapping(evidence_data(status="passed", scientific_status_unchanged=False))
    assert DisplayEvidence.from_mapping(evidence_data(
        status="difference_pending", blocked_reasons=[], pixel_diff_count=1,
    )).status == "difference_pending"


def test_passed_evidence_is_bound_to_rule_and_lawful_baseline():
    rule = LegacyDisplayRule.from_mapping(rule_data())
    proof = evidence_data(
        status="passed", blocked_reasons=[], rule_version=rule.rule_version,
        config_hash=rule.config_hash, input_hashes=[HASH], output_hash=HASH,
        baseline_identity="test-only paired output", baseline_hash=HASH,
        crop=[0, 0, 2, 2], shape=[2, 2], pixel_diff_count=0,
        alpha_comparison="identical", background_comparison="identical",
        missing_comparison="identical", sample_provenance="synthetic-only; not real migration evidence",
        review_conclusion="exact pixel agreement on synthetic-only test fixture",
    )
    evidence = DisplayEvidence.from_mapping(proof)
    assert evidence.validates(rule)
    changed_rule = LegacyDisplayRule.from_mapping(rule_data(
        ordered_steps=[{"op": "crop", "bounds": [1, 0, 2, 2], "basis": "synthetic fixture"}]))
    assert not evidence.validates(changed_rule)
    for changed in (dict(proof, baseline_hash=None), dict(proof, sample_provenance=None),
                    dict(proof, pixel_diff_count=1), dict(proof, input_hashes=[])):
        with pytest.raises(ValueError):
            DisplayEvidence.from_mapping(changed)


def test_display_inventory_keeps_science_separate_and_all_missing_paths_visible():
    inventory = json.loads((ROOT / "migration/legacy-display-inventory.json").read_text())
    assert inventory["coverage_status"].startswith("partial:")
    assert "WU" not in inventory["source_aliases"]
    assert set(inventory["excluded_display_sources"]) == {"opensnow", "wunderground"}
    keys = [(row["source"], row["product"], row["path_id"]) for row in inventory["paths"]]
    assert len(keys) == len(set(keys))
    assert all(row["scientific_status_unchanged"] is True for row in inventory["paths"])
    assert sum(row["status"] == "passed" for row in inventory["paths"]) == 15
    assert sum(row["status"] == "difference_pending" for row in inventory["paths"]) == 0
    assert sum(row["status"] == "blocked" for row in inventory["paths"]) == 8
    my_east = next(row for row in inventory["paths"] if row["path_id"] == "my/composite/east")
    assert my_east["status"] == "passed"
    assert my_east["rule_version"] == "old-8d251601-my-east-base-v1"
    assert my_east["input_hashes"] and my_east["output_hash"]
    assert my_east["source_baseline_verified"]
    my_source = json.loads((ROOT / "migration/sources/my.json").read_text())
    my_east_evidence = next(row for row in my_source["display_migration"]["paths"]
                            if row["path_id"] == "my/composite/east")
    assert my_east_evidence["baseline_hash"]
    assert my_east_evidence["pixel_diff_count"] == 0
    assert my_east_evidence["shape"] == [640, 568]
    assert all(row["scientific_status_unchanged"] is True for row in inventory["paths"])
    assert all(row["blocked_reasons"] if row["status"] == "blocked" else not row["blocked_reasons"]
               for row in inventory["paths"])
    assert {"my/composite/east", "my/composite/peninsular", "th/composite/cmp1",
            "th/composite/kkn240Loop", "tw-http/observation", "th_royalrain/cappi",
            "id_sidarma/cmax"} <= {row["path_id"] for row in inventory["paths"]}
    named_regions = {"au", "ca", "es", "id", "kr", "my", "nz", "ph", "sg", "th",
                     "tw", "vn", "fr", "pt"}
    named_tiles = {"rainviewer", "windy", "bmkg"}
    assert named_regions | named_tiles <= {row["source"] for row in inventory["paths"]}
    assert inventory["unclosed_scope_evidence"] and "remain blocked" in inventory["coverage_status"]


@pytest.mark.parametrize("source,product", [
    ("ph", "composite"), ("bmkg", "composite"),
    ("rainviewer", "composite"), ("windy", "reflectivity"),
])
def test_known_old_rules_with_missing_samples_remain_blocked(source, product):
    display_root = ROOT / "python/radiust/resources/legacy_display"
    rule_path = display_root / f"{source}.json"
    rule_data_value = json.loads(rule_path.read_text())
    rule = LegacyDisplayRule.from_mapping(rule_data_value)
    assert rule.validation_status == "blocked"

    index = json.loads((display_root / "index.json").read_text())
    entry = next(row for row in index["paths"] if row["path_id"] == rule.path_id)
    assert entry["status"] == "blocked"
    assert entry["rule_file"] == rule_path.name
    assert entry["rule_version"] == rule.rule_version
    assert entry["config_hash"] == rule.config_hash

    blocker_path = ROOT / "tests/fixtures/legacy-display" / source / "blocked-evidence.json"
    blocker = json.loads(blocker_path.read_text())
    assert blocker["status"] == "blocked"
    assert blocker["config_hash"] == rule.config_hash
    assert blocker["source_sample_retained"] is False
    assert blocker["scientific_status_unchanged"] is True

    registry = LegacyDisplayRegistry(display_root)
    selected, evidence, reason = registry.select(SimpleNamespace(
        ref=SimpleNamespace(source=source, product=product, station=None),
    ))
    assert selected is None and evidence is None
    assert "blocked" in reason


@pytest.mark.parametrize("source", ["opensnow", "wunderground"])
def test_open_snow_and_wu_are_outside_legacy_display_scope(source):
    display_root = ROOT / "python/radiust/resources/legacy_display"
    assert not (display_root / f"{source}.json").exists()
    index = json.loads((display_root / "index.json").read_text())
    inventory = json.loads((ROOT / "migration/legacy-display-inventory.json").read_text())
    manifest = json.loads((ROOT / "tests/fixtures/legacy-display/manifest.json").read_text())
    assert not any(row["source"] == source for row in index["paths"])
    assert not any(row["source"] == source for row in inventory["paths"])
    assert not any(row["source"] == source for row in manifest["entries"])
    assert "display_migration" not in json.loads((ROOT / f"migration/sources/{source}.json").read_text())
    selected, evidence, reason = LegacyDisplayRegistry(display_root).select(SimpleNamespace(
        ref=SimpleNamespace(source=source, product="composite", station=None),
    ))
    assert selected is None and evidence is None and "no validated" in reason


def test_twcomp3_alias_pairs_confirm_base_rule_without_overriding_provider_station():
    evidence = json.loads((ROOT / "tests/fixtures/legacy-display/tw/unmatched-archive-evidence.json").read_text())
    assert evidence["status"] == "blocked"
    assert evidence["station_identity"] == {
        "current_provider_station": "CV1_3600",
        "archive_station_label": "TWCOMP3",
        "alias_mapping": "not_assumed",
        "policy": "Keep the current station provider-native; archive output/config labels are provenance only and do not determine its value.",
    }
    assert evidence["pair_count"] == 6
    assert len(evidence["archive_pairs"]) == 6
    assert {item.split("-")[1] for item in evidence["archive_pairs"]} == {"ICUSRC", "SRC"}
    replay = evidence["transformation_replay"]
    assert replay["all_pairs_pixel_exact"] is True
    assert len(replay["pairs"]) == 6
    assert all(item["shape"] == [3600, 3600] and item["pixel_diff_count"] == 0 for item in replay["pairs"])
    assert any(item["path"] == "core/parser/__init__.py" for item in evidence["route_evidence"])
    assert "country-level rule evidence" in evidence["blocked_reasons"][0]
    assert "collector path" in evidence["blocked_reasons"][0]
    inventory = json.loads((ROOT / "migration/legacy-display-inventory.json").read_text())
    assert any("six archived TWCOMP3 raw/map pairs" in item and "pixel-exact" in item for item in inventory["scope_evidence"])
    assert not any(row["path_id"].endswith("/TWCOMP3") for row in inventory["paths"])


def test_mycomp72_supplemental_replay_and_my_east_archive_rule_match_their_baselines():
    replay = json.loads((ROOT / "tests/fixtures/legacy-display/my/east/replay-comparison.json").read_text())
    historical = json.loads((ROOT / "validation-results/legacy-display.json").read_text())
    assert replay["counts"] == {"total": 1, "passed": 1, "difference_pending": 0, "blocked": 0}
    replay_row = replay["paths"][0]
    assert replay_row["path_id"] == "my/composite/east"
    assert replay_row["pixel_diff_count"] == 0
    assert replay_row["shape"] == replay_row["baseline_shape"] == [650, 826]
    archive_row = next(row for row in historical["paths"] if row["path_id"] == "my/composite/east")
    assert archive_row["status"] == "passed"
    assert archive_row["rule_version"] == "old-8d251601-my-east-base-v1"
    assert archive_row["pixel_diff_count"] == 0
    assert archive_row["shape"] == [640, 568]
    assert archive_row["baseline_shape"] == [640, 568]


def test_my_east_uses_the_config_that_exactly_replays_all_archived_candidates():
    evidence = json.loads((ROOT / "tests/fixtures/legacy-display/my/east/archive-candidates.json").read_text())
    assert evidence["status"] == "passed"
    assert evidence["scientific_status_unchanged"] is True
    assert evidence["sample_use"]["redistribution_rights_inferred"] is False
    selection = evidence["current_station_selection"]
    assert selection["config"] == "MY/base.yaml"
    assert selection["station"] == "east"
    assert selection["archive_candidates_pixel_exact"] == 3
    assert selection["historical_route_cause_investigated"] is False
    assert selection["pixel_diff_count"] == 0
    rule = json.loads((ROOT / "python/radiust/resources/legacy_display/my-east.json").read_text())
    assert rule["rule_version"] == selection["rule_version"]
    assert rule["config_hash"] == selection["config_fingerprint"]
    assert "conf/parse/MY/base.yaml" in rule["legacy_reference"]
    assert rule["ordered_steps"][0]["bounds"] == [0, 0, 568, 640]
    packaged_index = json.loads((ROOT / "python/radiust/resources/legacy_display/index.json").read_text())
    packaged_east = next(row for row in packaged_index["paths"] if row["path_id"] == "my/composite/east")
    assert packaged_east["status"] == "passed"
    assert len(evidence["archive_candidates"]) == 3
    for pair in evidence["archive_candidates"]:
        assert pair["source_dimensions"] == [1113, 650]
        assert pair["gray_dimensions"] == [568, 640]
        assert pair["pinned_replays"]["base.yaml"]["pixel_diff_count"] == 0
        assert pair["pinned_replays"]["MYCOMP72.yaml"]["dimensions"] == [826, 650]
        assert pair["pinned_replays"]["MYCOMP72.yaml"]["pixel_diff_count"] is None
    mislabeled = next(pair for pair in evidence["archive_candidates"] if pair["source_path"].endswith("_source.gif"))
    assert mislabeled["source_format_signature"] == "PNG"
    assert evidence["result"]["current_station_rule"] == "MY/base.yaml"
    assert "current station identity is not derived" in evidence["scope"]


@pytest.mark.parametrize("source,channel,transform", [
    ("rainviewer", 0, "rainviewer-red"),
    ("windy", 1, "windy-green"),
])
def test_old_tile_channel_probe_reproduces_current_fixture_output_but_stays_blocked(source, channel, transform):
    evidence = json.loads((ROOT / f"tests/fixtures/legacy-display/{source}/blocked-evidence.json").read_text())
    probe = evidence["compatibility_probe"]
    assert evidence["status"] == "blocked"
    assert probe["old_rule"]["channel"] in {"red", "green"}
    assert probe["comparison"]["same_frame_old_gray_baseline_present"] is False
    assert probe["comparison"]["pixel_diff_comparison"].startswith("not possible")

    rows = []
    for y in range(2):
        row = []
        for x in range(2):
            tile = next(item for item in probe["tiles"] if item["name"] == f"tile-z1-x{x}-y{y}.png")
            payload = (ROOT / tile["path"]).read_bytes()
            assert hashlib.sha256(payload).hexdigest() == tile["sha256"]
            assert tile["declared_sha256_matches_fixture"] is True
            with Image.open(BytesIO(payload)) as image:
                values = np.asarray(image)[:, :, channel].copy()
            if transform == "rainviewer-red":
                values = values.astype(np.float32)
                values[values >= 128] -= 128
                values[values <= 32] = 0
                values[values >= 32] -= 32
                gray = (values / 5 * 16).astype(np.uint8).clip(0, 224)
            else:
                values = values.astype(np.uint8)
                values = (values / 2).astype(np.uint8)
                gray = (values / 5 * 16).astype(np.uint8).clip(0, 224)
            row.append(gray)
        rows.append(np.concatenate(row, axis=1))
    output = np.concatenate(rows, axis=0)
    result = probe["output"]
    assert [output.shape[1], output.shape[0]] == result["dimensions"]
    assert int(np.unique(output).size) == result["unique_gray_values"]
    assert np.count_nonzero(output == 0) / output.size == pytest.approx(result["zero_fraction"])
    assert int(output[output > 0].min()) == result["min_nonzero"]
    assert int(output.max()) == result["max"]
    assert hashlib.sha256(output.tobytes()).hexdigest() == result["raster_bytes_sha256"]


def test_ph_and_bmkg_blockers_record_why_no_old_channel_replay_is_possible():
    ph = json.loads((ROOT / "tests/fixtures/legacy-display/ph/blocked-evidence.json").read_text())
    assert ph["archive_search"]["files_found"] == 0
    assert ph["archive_search"]["source_matched_pair_found"] is False
    assert ph["current_acquisition_status"].startswith("browser acquisition failed")
    bmkg = json.loads((ROOT / "tests/fixtures/legacy-display/bmkg/blocked-evidence.json").read_text())
    assert bmkg["probe_attempt"]["frame_count"] == 0
    assert bmkg["probe_attempt"]["old_transform_applied"] is False
    assert "403" in bmkg["probe_attempt"]["public_probe_result"]


def test_display_schema_declares_distinct_inventory_rule_and_evidence_contracts():
    schema = json.loads((ROOT / "migration/legacy-display.schema.json").read_text())
    inventory = json.loads((ROOT / "migration/legacy-display-inventory.json").read_text())
    assert schema["$schema"].endswith("2020-12/schema")
    assert set(schema["required"]) <= set(inventory)
    assert set(inventory) <= set(schema["properties"])
    assert set(schema["$defs"]) >= {"inventoryPath", "rule", "evidence", "sourceDisplayMigration", "displayDisposition"}
    rule_required = set(schema["$defs"]["rule"]["required"])
    assert {"ordered_steps", "input_constraints", "legacy_reference", "config_hash"} <= rule_required
    assert "legacy_channel_decode" in schema["$defs"]["rule"]["properties"]["ordered_steps"]["items"]["properties"]["op"]["enum"]
    evidence_required = set(schema["$defs"]["evidence"]["required"])
    assert {"input_hashes", "output_hash", "baseline_hash", "pixel_diff_count",
            "scientific_status_unchanged", "blocked_reasons", "sample_provenance"} <= evidence_required
    for row in inventory["paths"]:
        assert set(schema["$defs"]["inventoryPath"]["required"]) <= set(row)
        assert set(row) <= set(schema["$defs"]["inventoryPath"]["properties"])
        assert row["path_id"].startswith(f'{row["source"]}/{row["product"]}')


def _fixture_png(color):
    buffer = BytesIO()
    Image.new("RGBA", (2, 1), color).save(buffer, format="PNG")
    return buffer.getvalue()


def _synthetic_manifest(root: Path):
    raw = _fixture_png((1, 2, 3, 255))
    gray = _fixture_png((16, 16, 16, 255))
    (root / "input.png").write_bytes(raw)
    (root / "baseline.png").write_bytes(gray)
    rule = rule_data(
        ordered_steps=[
            {"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "synthetic-only"},
            {"op": "gray_encode", "basis": "synthetic-only"},
        ],
        input_constraints={"formats": ["PNG"], "max_pixels": 16},
    )
    (root / "rule.json").write_text(json.dumps(rule), encoding="utf-8")
    entry = {
        "source": "example", "product": "composite", "path_id": "example/composite",
        "status": "candidate", "rule_path": "rule.json", "rule_version": rule["rule_version"],
        "config_hash": rule["config_hash"], "license_basis": "synthetic-generated fixture; no provider claim",
        "sample_provenance": "synthetic unit test, not an upstream artifact",
        "crop": [0, 0, 2, 1],
        "inputs": [{"path": "input.png", "name": "input.png", "role": "data",
                    "media_type": "image/png", "format": "PNG", "sha256": hashlib.sha256(raw).hexdigest()}],
        "baseline_path": "baseline.png", "baseline_identity": "synthetic paired gray image",
        "baseline_hash": hashlib.sha256(gray).hexdigest(),
    }
    return {"schema_version": 1, "coverage_status": "synthetic-only", "entries": [entry]}


def _replay(root: Path, manifest: dict):
    (root / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
    report = root / "report.json"
    command = [sys.executable, str(ROOT / "scripts/validation/compare_legacy_display.py"),
               "--manifest", str(root / "manifest.json"), "--fixture-root", str(root),
               "--report", str(report)]
    result = subprocess.run(command, capture_output=True, text=True, check=False, timeout=15)
    return result, json.loads(report.read_text()) if report.exists() else None


def test_offline_display_replay_compares_real_bytes_without_claiming_provider_evidence(tmp_path):
    manifest = _synthetic_manifest(tmp_path)
    before = (tmp_path / "input.png").read_bytes()
    result, report = _replay(tmp_path, manifest)
    assert result.returncode == 0, result.stderr
    assert report["counts"] == {"total": 1, "passed": 1, "difference_pending": 0, "blocked": 0}
    assert report["paths"][0]["pixel_diff_count"] == 0
    assert report["paths"][0]["alpha_comparison"] == "identical"
    assert report["paths"][0]["baseline_hash"] == manifest["entries"][0]["baseline_hash"]
    assert report["scientific_status_unchanged"] is True
    assert (tmp_path / "input.png").read_bytes() == before


def test_offline_display_difference_requires_review_and_returns_nonzero(tmp_path):
    manifest = _synthetic_manifest(tmp_path)
    different = _fixture_png((32, 32, 32, 255))
    (tmp_path / "baseline.png").write_bytes(different)
    manifest["entries"][0]["baseline_hash"] = hashlib.sha256(different).hexdigest()
    result, report = _replay(tmp_path, manifest)
    assert result.returncode == 1, result.stderr
    assert report["counts"] == {"total": 1, "passed": 0, "difference_pending": 1, "blocked": 0}
    assert report["paths"][0]["pixel_diff_count"] == 2
    assert report["paths"][0]["review_conclusion"].startswith("differences require review")
    manifest["entries"][0]["intentional_differences"] = [{"reason": "test", "impact": "test"}]
    result, report = _replay(tmp_path, manifest)
    assert result.returncode == 1
    assert report["paths"][0]["status"] == "difference_pending"


def test_offline_display_records_explicitly_reviewed_synthetic_difference(tmp_path):
    manifest = _synthetic_manifest(tmp_path)
    different = _fixture_png((32, 32, 32, 255))
    (tmp_path / "baseline.png").write_bytes(different)
    entry = manifest["entries"][0]
    entry["baseline_hash"] = hashlib.sha256(different).hexdigest()
    entry["intentional_differences"] = [{
        "reason": "synthetic test deliberately uses a changed reference",
        "impact": "two gray pixels differ; no provider claim",
        "reviewer": "synthetic-contract-test", "accepted": True,
    }]
    entry["review_conclusion"] = "accepted: synthetic test-only difference"
    result, report = _replay(tmp_path, manifest)
    assert result.returncode == 0
    assert report["paths"][0]["pixel_diff_count"] == 2
    assert report["paths"][0]["status"] == "passed"
    assert report["paths"][0]["intentional_differences"] == entry["intentional_differences"]
    assert "synthetic" in report["paths"][0]["review_conclusion"]


@pytest.mark.parametrize("change", ["raw_digest", "baseline_digest", "rule_hash", "missing_license",
                                      "url_path", "parent_path", "absolute_path", "missing_rule"])
def test_offline_display_replay_fails_closed_on_missing_or_untrusted_material(tmp_path, change):
    manifest = _synthetic_manifest(tmp_path)
    entry = manifest["entries"][0]
    if change == "raw_digest":
        entry["inputs"][0]["sha256"] = HASH
    elif change == "baseline_digest":
        entry["baseline_hash"] = HASH
    elif change == "rule_hash":
        entry["config_hash"] = HASH
    elif change == "missing_license":
        entry.pop("license_basis")
    elif change == "url_path":
        entry["inputs"][0]["path"] = "https://example.com/fixture.png"
    elif change == "parent_path":
        entry["inputs"][0]["path"] = "../other-source/input.png"
    elif change == "absolute_path":
        entry["inputs"][0]["path"] = str(tmp_path / "input.png")
    else:
        entry["rule_path"] = "missing.json"
    result, report = _replay(tmp_path, manifest)
    assert result.returncode != 0
    assert report["counts"] == {"total": 1, "passed": 0, "difference_pending": 0, "blocked": 1}
    assert report["paths"][0]["blocked_reasons"]


def test_offline_display_blocked_paths_remain_counted_and_duplicates_are_rejected(tmp_path):
    manifest = _synthetic_manifest(tmp_path)
    manifest["entries"].append({"source": "second", "product": "rain", "path_id": "second/rain",
                                "status": "blocked", "blocked_reasons": ["missing lawful old baseline"]})
    result, report = _replay(tmp_path, manifest)
    assert result.returncode == 1
    assert report["counts"] == {"total": 2, "passed": 1, "blocked": 1, "difference_pending": 0}
    assert report["paths"][1]["blocked_reasons"] == ["missing lawful old baseline"]
    manifest["entries"].append(manifest["entries"][1])
    result, report = _replay(tmp_path, manifest)
    assert result.returncode == 2
    assert report is not None  # prior result was not overwritten with a misleading duplicate report


def test_offline_blocked_rule_keeps_its_fingerprint_without_claiming_a_baseline(tmp_path):
    value = rule_data(source="example", product="composite", path_id="example/composite")
    (tmp_path / "rule.json").write_text(json.dumps(value), encoding="utf-8")
    manifest = {
        "schema_version": 1,
        "coverage_status": "blocked rule only; no source sample or golden",
        "entries": [{
            "source": "example", "product": "composite", "path_id": "example/composite",
            "status": "blocked", "rule_path": "rule.json",
            "rule_version": value["rule_version"], "config_hash": value["config_hash"],
            "blocked_reasons": ["missing lawful source raw and paired old gray output"],
        }],
    }
    result, report = _replay(tmp_path, manifest)
    assert result.returncode == 1
    row = report["paths"][0]
    assert row["status"] == "blocked"
    assert row["rule_version"] == value["rule_version"]
    assert row["config_hash"] == value["config_hash"]
    assert row["input_hashes"] == []
    assert row["baseline_hash"] is None


def test_offline_no_gray_disposition_is_reported_without_enabling_a_rule(tmp_path):
    reason = "pinned method passes through the source image; no old gray output is evidenced"
    disposition = {
        "schema_version": 1, "kind": "no_legacy_gray_transform",
        "source": "example", "product": "composite", "path_id": "example/composite",
        "status": "blocked", "old_commit": "8d251601ca551fbd5c05451f1fb337fc4b75362c",
        "legacy_reference": "8d251601:core/tiles/radar.py#git-blob=fixture",
        "verified_code_fact": "_parse_value returns merged_pic unchanged",
        "blocked_reasons": [reason], "source_sample_retained": False,
        "scientific_status_unchanged": True,
    }
    (tmp_path / "disposition.json").write_text(json.dumps(disposition), encoding="utf-8")
    manifest = {
        "schema_version": 1, "coverage_status": "no gray path; no source sample",
        "entries": [{
            "source": "example", "product": "composite", "path_id": "example/composite",
            "status": "blocked", "display_disposition_path": "disposition.json",
            "blocked_reasons": [reason],
        }],
    }
    result, report = _replay(tmp_path, manifest)
    assert result.returncode == 1
    row = report["paths"][0]
    assert row["status"] == "blocked"
    assert row["display_disposition"] == "no_legacy_gray_transform"
    assert row["rule_version"] is None and row["config_hash"] is None
    assert row["baseline_hash"] is None and row["input_hashes"] == []
