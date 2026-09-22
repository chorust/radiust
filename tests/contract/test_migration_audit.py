from __future__ import annotations

from pathlib import Path

from scripts.validation.audit_migration import build_report

ROOT = Path(__file__).parents[2]


def test_migration_audit_covers_every_head_adapter_and_reports_external_gates():
    report = build_report(ROOT)

    assert report["source_count"] == 24
    assert report["missing"] == []
    assert report["task_counts"] == {"total": 154, "checked": 148, "open": 6}
    assert report["open_tasks"] == ["T011", "T038", "T053", "T065", "T084", "T085"]
    assert report["ready_for_v1"] is False
