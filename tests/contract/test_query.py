from datetime import datetime, timezone

import pytest
from radiust import Query
from radiust.errors import UnsupportedQueryError


def test_query_requires_one_time_selector() -> None:
    with pytest.raises(UnsupportedQueryError):
        Query("my")
    with pytest.raises(UnsupportedQueryError):
        Query("my", latest=True, at=datetime.now(timezone.utc))


def test_query_range_is_half_open_and_utc() -> None:
    query = Query("my", start=datetime(2025, 1, 1, tzinfo=timezone.utc), end=datetime(2025, 1, 2, tzinfo=timezone.utc))
    assert query.start.tzinfo == timezone.utc
    with pytest.raises(UnsupportedQueryError):
        Query("my", start=datetime(2025, 1, 2, tzinfo=timezone.utc), end=datetime(2025, 1, 1, tzinfo=timezone.utc))


def test_query_normalizes_base_time() -> None:
    query = Query(
        "my",
        latest=True,
        base_time=datetime(2025, 1, 1, 8, tzinfo=timezone.utc),
    )
    assert query.base_time == datetime(2025, 1, 1, 8, tzinfo=timezone.utc)
