from __future__ import annotations

import json
import sys
from collections.abc import Mapping, Sequence
from dataclasses import fields, is_dataclass
from datetime import date, datetime, timedelta
from pathlib import Path
from typing import Any

from ..errors import RadiustError
from ..models import DownloadReport


def _json_safe(value: Any) -> Any:
    if isinstance(value, datetime):
        return value.isoformat().replace("+00:00", "Z")
    if isinstance(value, (date, timedelta, Path)):
        return str(value)
    if is_dataclass(value):
        return {item.name: _json_safe(getattr(value, item.name)) for item in fields(value)}
    if isinstance(value, Mapping):
        return {str(key): _json_safe(item) for key, item in value.items()}
    if isinstance(value, Sequence) and not isinstance(value, (str, bytes, bytearray)):
        return [_json_safe(item) for item in value]
    return value


def emit(value: Any, *, as_json: bool = False, quiet: bool = False, command: str | None = None) -> None:
    if as_json:
        if hasattr(value, "as_dict"):
            payload = value.as_dict()
        elif isinstance(value, dict) and value.get("schema_version") == 1:
            payload = value
        else:
            items = value if isinstance(value, list) else []
            payload = {
                "schema_version": 1,
                "command": command,
                "run_id": None,
                "query": None,
                "counts": {"items": len(items)} if isinstance(value, list) else {},
                "items": items,
                "result": value if not isinstance(value, list) else None,
                "error": None,
                "interrupted": False,
            }
        sys.stdout.write(json.dumps(_json_safe(payload), ensure_ascii=False, sort_keys=True) + "\n")
        return
    if quiet:
        return
    if isinstance(value, DownloadReport):
        counts = value.counts
        sys.stdout.write("download " + " ".join(f"{key}={counts[key]}" for key in counts if counts[key]) + "\n")
    elif isinstance(value, list):
        for item in value:
            sys.stdout.write(str(item) + "\n")
    else:
        sys.stdout.write(str(value) + "\n")


def emit_error(exc: Exception, *, as_json: bool = False) -> None:
    error = exc.as_dict() if isinstance(exc, RadiustError) else {"code": "error", "message": str(exc), "stage": "validate", "retryable": False}
    if as_json:
        sys.stdout.write(json.dumps({"schema_version": 1, "command": None, "run_id": None, "query": None, "counts": {}, "items": [], "error": error, "interrupted": False}, ensure_ascii=False, sort_keys=True) + "\n")
    else:
        sys.stderr.write(f"{error['code']}: {error['message']}\n")


def exit_code(report: DownloadReport) -> int:
    counts = report.counts
    if report.interrupted:
        return 130
    if counts["failed"] and (counts["written"] or counts["skipped"]):
        return 4
    if counts["failed"] and all(
        item.error and item.error.get("code") in {"no_data", "stale_frame"}
        for item in report.items
        if item.status == "failed"
    ):
        return 3
    if counts["failed"]:
        return 5
    if counts["planned"] == len(report.items):
        return 0
    if not report.items:
        return 3
    return 0
