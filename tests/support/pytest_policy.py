from __future__ import annotations

from collections.abc import Iterable


def opt_in_skip_reasons(
    markers: Iterable[str],
    *,
    allow_live: bool,
    allow_provider: bool,
) -> list[str]:
    marker_names = set(markers)
    reasons = []
    if "live" in marker_names and not allow_live:
        reasons.append("set RADIUST_TEST_ALLOW_LIVE=1 to run live-source tests")
    if "provider" in marker_names and not allow_provider:
        reasons.append("set RADIUST_TEST_ALLOW_PROVIDER=1 to run credentialed provider tests")
    return reasons
