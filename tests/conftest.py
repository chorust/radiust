from __future__ import annotations

import os
from pathlib import Path

import pytest

from tests.support.pytest_policy import opt_in_skip_reasons


def pytest_collection_modifyitems(config, items):
    """Keep live and credentialed provider traffic behind explicit opt-ins."""

    allow_live = os.environ.get("RADIUST_TEST_ALLOW_LIVE") == "1"
    allow_provider = os.environ.get("RADIUST_TEST_ALLOW_PROVIDER") == "1"
    for item in items:
        markers = {mark.name for mark in item.iter_markers()}
        for reason in opt_in_skip_reasons(markers, allow_live=allow_live, allow_provider=allow_provider):
            item.add_marker(pytest.mark.skip(reason=reason))


@pytest.fixture(autouse=True)
def inject_my_fixture_for_offline_pipeline_tests(monkeypatch, request):
    """Keep deterministic MY fixtures in offline SDK/CLI tests only.

    Source-adapter and live tests talk to MySource directly or opt into the
    provider. Production registry lookups never substitute checked-in test
    frames for provider data.
    """

    if (
        request.node.get_closest_marker("live")
        or request.node.get_closest_marker("provider")
        or request.node.get_closest_marker("source_adapter")
    ):
        return

    from radiust.registry import registry
    from radiust.sources.base import FixtureSource

    original_get = registry.get
    fixture_path = Path(__file__).parent / "fixtures" / "sources" / "my" / "fixture.json"
    fixture_source = FixtureSource(registry.get_info("my"), fixture_path)
    monkeypatch.setattr(registry, "get", lambda source_id: fixture_source if source_id == "my" else original_get(source_id))
