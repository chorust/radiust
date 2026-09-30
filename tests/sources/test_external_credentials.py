import json

import pytest
from radiust._bridge import native_discover

CREDENTIAL_FIELDS = {
    "id": "token",
    "id_sidarma": "api_key",
    "wunderground": "api_key",
}


@pytest.mark.asyncio
async def test_native_discovery_fails_closed_without_required_credentials():
    report = await native_discover(
        {"runtime": {"allow_network": False}}, {"source": "all"}
    )
    statuses = {
        item["target"]["source"]: item["status"] for item in report["items"]
    }

    assert report["counts"]["total"] == 26
    for source_id in CREDENTIAL_FIELDS:
        assert statuses[source_id] == "missing_credentials"
    assert statuses["ph"] == "network_restricted"
    assert statuses["uk"] == "retired"


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
    assert report["counts"]["retired"] == 1
    assert report["counts"]["network_restricted"] == 25
    for source_id in CREDENTIAL_FIELDS:
        assert f"offline-test-{source_id}-secret" not in serialized
