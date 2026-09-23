"""CLI-only time defaulting. The public SDK Query remains explicitly selected."""

from __future__ import annotations

from datetime import datetime, timedelta

import click

from ..models import Query, utc_datetime


def parse_time(value: str | None) -> datetime | None:
    if value is None:
        return None
    try:
        return utc_datetime(datetime.fromisoformat(value.replace("Z", "+00:00")))
    except ValueError as exc:
        raise click.BadParameter("time must be ISO-8601 with a timezone") from exc


def build_query(
    source: str,
    *,
    product: str | None = None,
    stations: tuple[str, ...] = (),
    latest: bool = False,
    at: datetime | None = None,
    start: datetime | None = None,
    end: datetime | None = None,
    base_time: datetime | None = None,
    max_age: timedelta | None = None,
) -> Query:
    default_latest = not latest and at is None and start is None and end is None
    return Query(
        source,
        product=product,
        stations=stations,
        latest=latest or default_latest,
        at=at,
        start=start,
        end=end,
        base_time=base_time,
        max_age=max_age,
    )
