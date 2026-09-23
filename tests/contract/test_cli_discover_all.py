"""Aggregate discovery keeps a target for every catalog product/station."""

import json

import pytest
from click.testing import CliRunner
from radiust.cli.main import main
from radiust.config import load_config
from radiust.discovery import (
    STATUSES,
    discover_all,
    discovery_exit_code,
    discovery_report,
    error_item,
    expand_targets,
)
from radiust.models import DiscoveryTarget, ProductInfo, SourceInfo, StationInfo


def test_catalog_expansion_respects_station_products_and_no_station():
    info = SourceInfo(
        "fake", "Offline", "1",
        (ProductInfo("a", variables=("v",)), ProductInfo("b", variables=("v",))),
        (StationInfo("s1", "S1", 0, 0, product_ids=("a",)), StationInfo("s2", "S2", 1, 1, product_ids=("b",))),
    )
    targets = expand_targets((info,))
    assert {(t.source, t.product, t.station) for t in targets} == {("fake", "a", "s1"), ("fake", "b", "s2")}


def test_report_counts_statuses_and_safe_order():
    report = discovery_report([
        {"source": "b", "product": None, "station": None, "status": "network_restricted", "valid_time": None, "frame": None, "capabilities": None, "error": {"code": "network_restricted", "message": "disabled", "stage": "discover", "retryable": False}},
        {"source": "a", "product": "p", "station": "s", "status": "success", "valid_time": "2026-09-22T00:00:00Z", "frame": {"source": "a", "product": "p", "station": "s", "valid_time": "2026-09-22T00:00:00Z", "base_time": None}, "capabilities": None, "error": None},
    ])
    assert report["counts"]["total"] == 2
    assert sum(n for k, n in report["counts"].items() if k != "total") == 2
    assert [i["source"] for i in report["items"]] == ["a", "b"]


def test_cli_all_rejects_disallowed_query_options_before_work():
    result = CliRunner().invoke(main, ["discover", "all", "--product", "composite", "--json"])
    assert result.exit_code == 2
    assert "single" in result.output.lower() or "source" in result.output.lower()


def test_cli_all_default_produces_single_report():
    result = CliRunner().invoke(main, ["discover", "all", "--json"])
    assert result.exit_code in {0, 3, 4, 5}, result.output
    report = json.loads(result.output)
    assert report["schema_version"] == 1
    assert report["query"]["source"] == "all"
    assert report["counts"]["total"] == len(report["items"])


def test_discovery_report_rejects_unrecognized_status_and_duplicate_target():
    one = {"source": "a", "product": "p", "station": None, "status": "not_started", "error": {"code": "not_started", "message": "queued", "stage": "discover", "retryable": False}}
    with pytest.raises(ValueError, match="status"):
        discovery_report([{**one, "status": "written"}])
    with pytest.raises(ValueError, match="duplicate"):
        discovery_report([one, one])


def test_every_status_is_counted_and_non_scientific_success_stays_success():
    items = [error_item(DiscoveryTarget(f"source{i}", "rain", None), status, status)
             for i, status in enumerate(STATUSES) if status != "success"]
    items.append({
        "source": "source_success", "product": "rain", "station": None, "status": "success",
        "valid_time": "2026-09-22T00:00:00Z", "error": None,
        "frame": {"source": "source_success", "product": "rain", "station": None,
                  "valid_time": "2026-09-22T00:00:00Z", "base_time": None},
        "capabilities": {"scientific_decode": False, "reason": "not validated"},
    })
    report = discovery_report(items)
    assert report["counts"]["total"] == len(STATUSES)
    assert all(report["counts"][status] == 1 for status in STATUSES)
    assert discovery_exit_code(report) == 4
    success = next(item for item in report["items"] if item["status"] == "success")
    assert success["capabilities"]["scientific_decode"] is False


@pytest.mark.parametrize(("statuses", "interrupted", "expected"), [
    ([], False, 3), (["success"], False, 0), (["success", "no_data"], False, 4),
    (["no_data", "stale"], False, 3), (["upstream_failed"], False, 5),
    (["success"], True, 130),
])
def test_aggregate_exit_code_contract(statuses, interrupted, expected):
    items = []
    for i, status in enumerate(statuses):
        target = DiscoveryTarget(f"source{i}", "rain", None)
        if status != "success":
            items.append(error_item(target, status, status))
        else:
            stamp = "2026-09-22T00:00:00Z"
            items.append({"source": target.source, "product": "rain", "station": None,
                          "status": "success", "valid_time": stamp, "error": None,
                          "frame": {"source": target.source, "product": "rain", "station": None,
                                    "valid_time": stamp, "base_time": None}, "capabilities": None})
    assert discovery_exit_code(discovery_report(items, interrupted=interrupted)) == expected


def test_catalog_with_no_products_retains_source_placeholder_offline():
    info = SourceInfo("empty", "Empty catalog source", "1", ())
    targets = expand_targets((info,))
    assert targets == [DiscoveryTarget("empty", None, None)]
    report = discover_all(load_config(environ={}), catalog=(info,))
    assert report["items"][0]["product"] is None
    assert report["items"][0]["station"] is None
    assert report["counts"]["total"] == 1


@pytest.mark.parametrize("selector", [
    ("--at", "2026-09-22T00:00:00Z"), ("--start", "2026-09-22T00:00:00Z"),
    ("--end", "2026-09-23T00:00:00Z"), ("--base-time", "2026-09-22T00:00:00Z"),
    ("--product", "rain"), ("--station", "s"),
])
def test_all_rejects_selectors_before_loading_catalog(monkeypatch, selector):
    import radiust.discovery as discovery

    calls = []
    monkeypatch.setattr(discovery, "sources", lambda: calls.append("catalog") or ())
    response = CliRunner().invoke(main, ["discover", "all", *selector, "--json"])
    assert response.exit_code == 2
    assert calls == []


def test_all_max_age_is_preserved_in_single_json_envelope():
    response = CliRunner().invoke(main, ["discover", "all", "--max-age", "600", "--json"])
    assert response.exit_code in {0, 3, 4, 5}
    report = json.loads(response.output)
    assert report["query"]["max_age"] == 600.0
    assert report["query"]["latest"] is True


def test_catalog_expansion_includes_valid_no_station_product_and_one_target_per_relation():
    info = SourceInfo(
        "weather", "offline", "1",
        (ProductInfo("radar", variables=("v",)), ProductInfo("satellite", variables=("v",))),
        (StationInfo("a", "A", 0, 0, product_ids=("radar",)),
         StationInfo("b", "B", 1, 1, product_ids=("radar",))),
    )
    targets = expand_targets((info, info))
    assert targets == [DiscoveryTarget("weather", "radar", "a"),
                       DiscoveryTarget("weather", "radar", "b"),
                       DiscoveryTarget("weather", "satellite", None)]


def test_discover_all_records_actual_target_statuses_and_keeps_capabilities_separate(monkeypatch):
    import radiust.discovery as discovery

    info = SourceInfo("local", "offline", "1", (
        ProductInfo("a", variables=("v",)), ProductInfo("b", variables=("v",)),
    ), (StationInfo("s", "S", 0, 0),))
    observed = []

    def run(_ctx, target, _config, max_age, _deadline, _worker, _limiter):
        observed.append((target.product, target.station, max_age))
        if target.product == "a":
            stamp = "2026-09-22T00:00:00Z"
            return {"source": target.source, "product": target.product,
                    "station": target.station, "status": "success", "valid_time": stamp,
                    "frame": {"source": target.source, "product": target.product,
                              "station": target.station, "valid_time": stamp},
                    "capabilities": {"scientific_decode": False, "reason": "not verified"},
                    "error": None}
        return error_item(target, "stale", "only an expired frame is available")

    monkeypatch.setattr(discovery, "_run_isolated_target", run)
    config = load_config({"runtime": {"allow_network": True}}, environ={})
    report = discover_all(config, catalog=(info,), max_age=600)
    assert observed == [("a", "s", 600), ("b", "s", 600)]
    assert report["counts"]["total"] == 2
    assert report["counts"]["success"] == report["counts"]["stale"] == 1
    assert all(report["counts"][status] == 0 for status in STATUSES if status not in {"success", "stale"})
    assert discovery_exit_code(report) == 4
    assert report["items"][0]["capabilities"]["scientific_decode"] is False
    assert report["items"][1]["error"]["code"] == "stale"


def test_discovery_report_null_first_sort_and_safe_error_projection():
    target = DiscoveryTarget("source", None, None)
    message = "bad\x1b]0;title\x07 token=unpublished"
    report = discovery_report([
        error_item(DiscoveryTarget("source", "b", "z"), "upstream_failed", message),
        error_item(DiscoveryTarget("source", "a", None), "no_data", "empty"),
        error_item(target, "network_restricted", "offline"),
    ])
    assert [(item["product"], item["station"]) for item in report["items"]] == [
        (None, None), ("a", None), ("b", "z"),
    ]
    assert "unpublished" not in json.dumps(report)
    assert "\x1b" not in json.dumps(report)
    assert report["counts"]["total"] == 3


def test_all_rejects_invalid_max_age_before_catalog_expansion(monkeypatch):
    import radiust.discovery as discovery

    called = []
    monkeypatch.setattr(discovery, "sources", lambda: called.append(True) or ())
    for age in ("0", "-1"):
        result = CliRunner().invoke(main, ["discover", "all", "--max-age", age, "--json"])
        assert result.exit_code == 2
    assert called == []
