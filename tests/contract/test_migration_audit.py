from __future__ import annotations

from copy import deepcopy
from pathlib import Path

from scripts.validation import audit_migration
from scripts.validation.audit_migration import build_report

ROOT = Path(__file__).parents[2]


def test_migration_audit_covers_every_head_adapter_and_reports_external_gates():
    report = build_report(ROOT)

    assert report["source_count"] == 24
    assert report["missing"] == []
    assert report["task_counts"] == {"total": 154, "checked": 148, "open": 6}
    assert report["open_tasks"] == ["T011", "T038", "T053", "T065", "T084", "T085"]
    assert report["ready_for_v1"] is False
    assert report["display_migration"]["total_paths"] == 23
    assert report["display_migration"]["source_count"] == 20
    assert report["display_migration"]["counts"] == {
        "passed": 15, "difference_pending": 0, "blocked": 8,
    }
    assert report["display_migration"]["structural_errors"] == []
    assert report["display_migration"]["coverage_closed"] is False
    assert report["display_migration"]["ready"] is False


def test_display_audit_rejects_missing_packaged_path_without_changing_source_evidence(monkeypatch):
    read = audit_migration._read_json

    def missing_packaged(path):
        value = deepcopy(read(path))
        if path.name == "index.json" and "legacy_display" in path.parts:
            value["paths"].pop()
        return value

    monkeypatch.setattr(audit_migration, "_read_json", missing_packaged)
    result = audit_migration._audit_display(ROOT)
    assert result["ready"] is False
    assert "packaged index path coverage differs from inventory" in result["structural_errors"]


def test_display_audit_rejects_missing_source_evidence_or_science_state_mutation(monkeypatch):
    read = audit_migration._read_json

    def invalid_source(path):
        value = deepcopy(read(path))
        if path.parts[-3:] == ("migration", "sources", "au.json"):
            value["display_migration"]["paths"][0]["scientific_status_unchanged"] = False
            value["display_migration"]["paths"][0]["blocked_reasons"] = []
        return value

    monkeypatch.setattr(audit_migration, "_read_json", invalid_source)
    result = audit_migration._audit_display(ROOT)
    assert result["ready"] is False
    assert any("source au has invalid display evidence" in error for error in result["structural_errors"])
    assert any("source au display status differs" in error for error in result["structural_errors"])
