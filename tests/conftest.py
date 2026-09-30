from __future__ import annotations

import os

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
