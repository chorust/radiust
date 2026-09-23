from datetime import datetime, timedelta, timezone

import pytest
from radiust.cli.query import build_query
from radiust.errors import UnsupportedQueryError
from radiust.models import Query


def test_default_and_explicit_time_do_not_change_sdk_contract():
    assert build_query("th").latest
    assert build_query("th", latest=True).latest
    at = datetime(2025, 1, 1, tzinfo=timezone.utc)
    assert build_query("th", at=at).at == at
    assert not build_query("th", at=at).latest
    assert not build_query("th", start=at, end=at.replace(hour=1)).latest
    with pytest.raises(UnsupportedQueryError):
        Query("th")


@pytest.mark.parametrize("options", [
    {"latest": True, "at": datetime(2025, 1, 1, tzinfo=timezone.utc)},
    {"start": datetime(2025, 1, 1, tzinfo=timezone.utc)},
    {"end": datetime(2025, 1, 1, tzinfo=timezone.utc)},
])
def test_incomplete_and_conflicting_explicit_selectors_remain_errors(options):
    with pytest.raises(UnsupportedQueryError):
        build_query("th", **options)


def test_base_time_and_max_age_keep_existing_selector_rules():
    base_time = datetime(2025, 1, 1, tzinfo=timezone.utc)
    age = timedelta(hours=1)
    query = build_query("th", base_time=base_time, max_age=age)
    assert query.latest and query.base_time == base_time and query.max_age == age
    with pytest.raises(UnsupportedQueryError):
        build_query("th", at=base_time, max_age=age)
    with pytest.raises(UnsupportedQueryError):
        build_query("th", start=base_time, end=base_time.replace(hour=1), max_age=age)
    with pytest.raises(UnsupportedQueryError):
        build_query("th", latest=True, at=base_time, base_time=base_time)


def test_cli_rejects_invalid_time_without_falling_back_to_latest():
    from click.testing import CliRunner
    from radiust.cli.main import main

    result = CliRunner().invoke(main, ["discover", "th", "--at", "2025-01-01"])
    assert result.exit_code == 2
    assert "timezone" in result.output
