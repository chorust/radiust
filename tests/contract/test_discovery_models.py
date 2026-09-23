"""Aggregate discovery has a separate report model and preserves v1 JSON."""

from __future__ import annotations

import pytest
from radiust.models import DiscoveryItem, DiscoveryReport, DiscoveryTarget


def test_typed_report_has_complete_zero_counts_and_null_first_sort():
    report = DiscoveryReport((
        DiscoveryItem(DiscoveryTarget("a", "product", "s"), "no_data", error={"code": "no_data", "message": "none", "stage": "discover", "retryable": False}),
        DiscoveryItem(DiscoveryTarget("a", None, None), "network_restricted", error={"code": "network_restricted", "message": "offline", "stage": "discover", "retryable": False}),
        DiscoveryItem(DiscoveryTarget("b", "product", None), "success", valid_time="2026-09-22T00:00:00Z",
                      frame={"source": "b", "product": "product", "station": None, "valid_time": "2026-09-22T00:00:00Z", "base_time": None,
                             "uri": "https://user:password@host.invalid/file?token=private", "metadata": {"token": "private"}},
                      capabilities={"scientific_decode": False, "reason": "decoder unavailable"}),
    ))
    payload = report.as_dict()
    assert payload["schema_version"] == 1
    assert payload["counts"]["total"] == len(payload["items"]) == 3
    assert sum(n for k, n in payload["counts"].items() if k != "total") == 3
    assert payload["counts"]["timeout"] == payload["counts"]["cancelled"] == 0
    assert [(i["source"], i["product"]) for i in payload["items"]] == [("a", None), ("a", "product"), ("b", "product")]
    assert payload["items"][-1]["capabilities"]["scientific_decode"] is False
    assert "password" not in repr(payload) and "private" not in repr(payload)


def test_typed_report_rejects_duplicate_target_and_invalid_status():
    target = DiscoveryTarget("a", "p", "s")
    one = DiscoveryItem(target, "not_started", error={"code": "not_started", "message": "queued", "stage": "discover", "retryable": False})
    with pytest.raises(ValueError, match="duplicate"):
        DiscoveryReport((one, one))
    with pytest.raises(ValueError, match="status"):
        DiscoveryItem(target, "written")
    with pytest.raises(ValueError, match="frame"):
        DiscoveryItem(target, "success", valid_time="2026-09-22T00:00:00Z")


def test_typed_report_rejects_success_mismatched_frame_identity():
    with pytest.raises(ValueError, match="identity"):
        DiscoveryItem(DiscoveryTarget("a", "p", None), "success", valid_time="2026-09-22T00:00:00Z",
                      frame={"source": "other", "product": "p", "station": None, "valid_time": "2026-09-22T00:00:00Z", "base_time": None})
