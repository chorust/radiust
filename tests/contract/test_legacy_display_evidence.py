"""Data-only migration evidence audits; rendering behavior is covered by Rust."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[2]
DISPLAY_ROOT = ROOT / "python/radiust/resources/legacy_display"


def _json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def test_display_inventory_keeps_science_separate_and_missing_paths_visible():
    inventory = _json(ROOT / "migration/legacy-display-inventory.json")
    assert inventory["coverage_status"].startswith("partial:")
    assert "WU" not in inventory["source_aliases"]
    assert set(inventory["excluded_display_sources"]) == {"opensnow", "wunderground"}
    keys = [(row["source"], row["product"], row["path_id"]) for row in inventory["paths"]]
    assert len(keys) == len(set(keys))
    assert all(row["scientific_status_unchanged"] is True for row in inventory["paths"])
    assert sum(row["status"] == "passed" for row in inventory["paths"]) == 15
    assert sum(row["status"] == "difference_pending" for row in inventory["paths"]) == 0
    assert sum(row["status"] == "blocked" for row in inventory["paths"]) == 8
    assert all(
        row["blocked_reasons"] if row["status"] == "blocked" else not row["blocked_reasons"]
        for row in inventory["paths"]
    )
    assert inventory["unclosed_scope_evidence"]
    assert "remain blocked" in inventory["coverage_status"]


@pytest.mark.parametrize(
    ("source", "product"),
    [
        ("ph", "composite"),
        ("bmkg", "composite"),
        ("rainviewer", "composite"),
        ("windy", "reflectivity"),
    ],
)
def test_old_rules_without_lawful_samples_remain_blocked(source, product):
    rule = _json(DISPLAY_ROOT / f"{source}.json")
    assert rule["source"] == source
    assert rule["product"] == product
    assert rule["validation_status"] == "blocked"

    index = _json(DISPLAY_ROOT / "index.json")
    entry = next(row for row in index["paths"] if row["path_id"] == rule["path_id"])
    assert entry["status"] == "blocked"
    assert entry["rule_version"] == rule["rule_version"]
    assert entry["config_hash"] == rule["config_hash"]

    blocker = _json(ROOT / "tests/fixtures/legacy-display" / source / "blocked-evidence.json")
    assert blocker["status"] == "blocked"
    assert blocker["config_hash"] == rule["config_hash"]
    assert blocker["source_sample_retained"] is False
    assert blocker["scientific_status_unchanged"] is True


@pytest.mark.parametrize("source", ["opensnow", "wunderground"])
def test_open_snow_and_wu_have_no_legacy_display_rule(source):
    assert not (DISPLAY_ROOT / f"{source}.json").exists()
    index = _json(DISPLAY_ROOT / "index.json")
    inventory = _json(ROOT / "migration/legacy-display-inventory.json")
    manifest = _json(ROOT / "tests/fixtures/legacy-display/manifest.json")
    assert not any(row["source"] == source for row in index["paths"])
    assert not any(row["source"] == source for row in inventory["paths"])
    assert not any(row["source"] == source for row in manifest["entries"])
    assert "display_migration" not in _json(ROOT / f"migration/sources/{source}.json")


def test_twcomp3_archive_pairs_do_not_override_provider_station_identity():
    evidence = _json(ROOT / "tests/fixtures/legacy-display/tw/unmatched-archive-evidence.json")
    assert evidence["status"] == "blocked"
    assert evidence["station_identity"]["current_provider_station"] == "CV1_3600"
    assert evidence["station_identity"]["archive_station_label"] == "TWCOMP3"
    assert evidence["station_identity"]["alias_mapping"] == "not_assumed"
    assert evidence["pair_count"] == len(evidence["archive_pairs"]) == 6
    replay = evidence["transformation_replay"]
    assert replay["all_pairs_pixel_exact"] is True
    assert all(item["pixel_diff_count"] == 0 for item in replay["pairs"])
    assert any(item["path"] == "core/parser/__init__.py" for item in evidence["route_evidence"])
    assert "country-level rule evidence" in evidence["blocked_reasons"][0]
    inventory = _json(ROOT / "migration/legacy-display-inventory.json")
    assert not any(row["path_id"].endswith("/TWCOMP3") for row in inventory["paths"])


def test_my_east_archive_replay_and_rule_metadata_match_the_recorded_baseline():
    replay = _json(ROOT / "tests/fixtures/legacy-display/my/east/replay-comparison.json")
    history = _json(ROOT / "validation-results/legacy-display.json")
    assert replay["counts"] == {"total": 1, "passed": 1, "difference_pending": 0, "blocked": 0}
    replay_row = replay["paths"][0]
    assert replay_row["path_id"] == "my/composite/east"
    assert replay_row["pixel_diff_count"] == 0
    assert replay_row["shape"] == replay_row["baseline_shape"] == [650, 826]
    archived = next(row for row in history["paths"] if row["path_id"] == "my/composite/east")
    assert archived["status"] == "passed"
    assert archived["rule_version"] == "old-8d251601-my-east-base-v1"
    assert archived["pixel_diff_count"] == 0
    assert archived["shape"] == archived["baseline_shape"] == [640, 568]

    evidence = _json(ROOT / "tests/fixtures/legacy-display/my/east/archive-candidates.json")
    selection = evidence["current_station_selection"]
    rule = _json(DISPLAY_ROOT / "my-east.json")
    assert evidence["status"] == "passed"
    assert evidence["scientific_status_unchanged"] is True
    assert evidence["sample_use"]["redistribution_rights_inferred"] is False
    assert selection["config"] == "MY/base.yaml"
    assert selection["station"] == "east"
    assert selection["archive_candidates_pixel_exact"] == 3
    assert rule["rule_version"] == selection["rule_version"]
    assert rule["config_hash"] == selection["config_fingerprint"]
    assert rule["ordered_steps"][0]["bounds"] == [0, 0, 568, 640]
    assert len(evidence["archive_candidates"]) == 3
    for pair in evidence["archive_candidates"]:
        assert pair["pinned_replays"]["base.yaml"]["pixel_diff_count"] == 0


@pytest.mark.parametrize(
    ("source", "channel"),
    [("rainviewer", 0), ("windy", 1)],
)
def test_tile_channel_probes_remain_blocked_without_a_paired_historical_baseline(source, channel):
    evidence = _json(ROOT / f"tests/fixtures/legacy-display/{source}/blocked-evidence.json")
    probe = evidence["compatibility_probe"]
    assert evidence["status"] == "blocked"
    assert probe["old_rule"]["channel"] in {"red", "green"}
    assert probe["comparison"]["same_frame_old_gray_baseline_present"] is False
    assert probe["comparison"]["pixel_diff_comparison"].startswith("not possible")
    for tile in probe["tiles"]:
        payload = (ROOT / tile["path"]).read_bytes()
        assert hashlib.sha256(payload).hexdigest() == tile["sha256"]
        assert tile["declared_sha256_matches_fixture"] is True
    assert probe["output"]["dimensions"]


def test_ph_and_bmkg_blockers_record_missing_replay_evidence():
    ph = _json(ROOT / "tests/fixtures/legacy-display/ph/blocked-evidence.json")
    assert ph["archive_search"]["files_found"] == 0
    assert ph["archive_search"]["source_matched_pair_found"] is False
    bmkg = _json(ROOT / "tests/fixtures/legacy-display/bmkg/blocked-evidence.json")
    assert bmkg["probe_attempt"]["frame_count"] == 0
    assert bmkg["probe_attempt"]["old_transform_applied"] is False
    assert "403" in bmkg["probe_attempt"]["public_probe_result"]


def test_display_schema_matches_inventory_rule_and_evidence_records():
    schema = _json(ROOT / "migration/legacy-display.schema.json")
    inventory = _json(ROOT / "migration/legacy-display-inventory.json")
    assert schema["$schema"].endswith("2020-12/schema")
    assert set(schema["required"]) <= set(inventory)
    assert set(inventory) <= set(schema["properties"])
    assert {"inventoryPath", "rule", "evidence", "sourceDisplayMigration", "displayDisposition"} <= set(
        schema["$defs"]
    )
    rule_required = set(schema["$defs"]["rule"]["required"])
    assert {"ordered_steps", "input_constraints", "legacy_reference", "config_hash"} <= rule_required
    evidence_required = set(schema["$defs"]["evidence"]["required"])
    assert {
        "input_hashes",
        "output_hash",
        "baseline_hash",
        "pixel_diff_count",
        "scientific_status_unchanged",
        "blocked_reasons",
        "sample_provenance",
    } <= evidence_required
    for row in inventory["paths"]:
        assert set(schema["$defs"]["inventoryPath"]["required"]) <= set(row)
        assert set(row) <= set(schema["$defs"]["inventoryPath"]["properties"])
        assert row["path_id"].startswith(f'{row["source"]}/{row["product"]}')


def test_python_rendering_implementations_are_not_left_as_runtime_modules():
    assert not (ROOT / "python/radiust/display").exists()
    assert not (ROOT / "python/radiust/rendering").exists()
    assert not (ROOT / "python/radiust/terminal/api.py").exists()
    assert not (ROOT / "python/radiust/terminal/capabilities.py").exists()
    assert not (ROOT / "python/radiust/terminal/ansi.py").exists()
