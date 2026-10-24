import json

import pytest
from radiust._bridge import native_discover

CREDENTIAL_FIELDS = {
    "id": "token",
    "id_sidarma": "api_key",
    "tw": "api_key",
    "wunderground": "api_key",
}


@pytest.mark.asyncio
async def test_native_discovery_fails_closed_without_required_credentials():
    report = await native_discover(
        {"runtime": {"allow_network": False}}, {"source": "all"}
    )
    statuses = {}
    for item in report["items"]:
        statuses.setdefault(item["target"]["source"], set()).add(item["status"])

    for source_id in CREDENTIAL_FIELDS:
        assert "missing_credentials" in statuses[source_id]
    assert statuses["ph"] == {"network_restricted"}
    assert statuses["uk"] == {"retired"}


@pytest.mark.asyncio
async def test_native_reports_never_echo_configured_source_credentials():
    sources = {
        source_id: {field: f"offline-test-{source_id}-secret"}
        for source_id, field in CREDENTIAL_FIELDS.items()
    }
    report = await native_discover(
        {"runtime": {"allow_network": False}, "sources": sources},
        {"source": "all"},
    )
    serialized = json.dumps(report)

    assert report["counts"]["missing_credentials"] == 0
    for item in report["items"]:
        if item["target"]["source"] in CREDENTIAL_FIELDS:
            assert item["status"] == "network_restricted"
    for source_id in CREDENTIAL_FIELDS:
        assert f"offline-test-{source_id}-secret" not in serialized
