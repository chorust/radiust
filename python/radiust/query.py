"""Query normalization and deterministic frame selection helpers."""

from __future__ import annotations

from collections.abc import Iterable, Mapping
from datetime import datetime, timedelta
from typing import Any

from .errors import AmbiguousFrameError, NoDataError, UnsupportedQueryError
from .models import UTC, FrameRef, Query, utc_datetime


def _time(value: datetime | str | None) -> datetime | None:
    if value is None or isinstance(value, datetime):
        return utc_datetime(value) if value is not None else None
    if isinstance(value, str):
        try:
            return utc_datetime(datetime.fromisoformat(value.replace("Z", "+00:00")))
        except ValueError as exc:
            raise UnsupportedQueryError("time must be ISO-8601 with a timezone") from exc
    raise UnsupportedQueryError("time must be a datetime or ISO-8601 string")


def normalize_query(value: Query | Mapping[str, Any]) -> Query:
    """Build a validated Query from the public object or a CLI/config mapping."""
    if isinstance(value, Query):
        return value
    if not isinstance(value, Mapping) or "source" not in value:
        raise UnsupportedQueryError("query must contain a source")
    max_age = value.get("max_age")
    if isinstance(max_age, (int, float)):
        max_age = timedelta(seconds=float(max_age))
    if max_age is not None and not isinstance(max_age, timedelta):
        raise UnsupportedQueryError("max_age must be a timedelta or seconds")
    stations = value.get("stations", value.get("station", ()))
    if isinstance(stations, str):
        stations = (stations,)
    return Query(
        source=str(value["source"]),
        product=value.get("product"),
        stations=tuple(stations or ()),
        latest=bool(value.get("latest", False)),
        at=_time(value.get("at")),
        start=_time(value.get("start")),
        end=_time(value.get("end")),
        base_time=_time(value.get("base_time")),
        max_age=max_age,
    )


def sort_frames(refs: Iterable[FrameRef]) -> tuple[FrameRef, ...]:
    """Return stable UTC ordering for discovered frames."""
    return tuple(
        sorted(
            refs,
            key=lambda ref: (
                ref.valid_time,
                ref.source,
                ref.product,
                ref.station or "",
                ref.base_time or datetime.min.replace(tzinfo=UTC),
                ref.logical_id,
            ),
        )
    )


def select_one(refs: Iterable[FrameRef], *, context: str = "query") -> FrameRef:
    ordered = sort_frames(refs)
    if not ordered:
        raise NoDataError(f"{context} returned no frames")
    if len(ordered) > 1:
        raise AmbiguousFrameError(f"{context} returned {len(ordered)} frames; select a single time or station")
    return ordered[0]
