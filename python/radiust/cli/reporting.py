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
from .layout import render
from .safety import safe_text, safe_value

_RECOVERY_ADVICE = {
    "authentication": "Check the configured credentials for this source.",
    "missing_credentials": "Configure the credentials required by this source.",
    "network_restricted": "Network access is disabled; use an offline fixture or explicitly enable access.",
    "no_data": "Check the selected time, product and station for available frames.",
    "stale_frame": "Review --max-age or select an explicit observation time.",
    "ambiguous": "Select a single station, product or observation time.",
    "ambiguous_frame": "Select a single station, product or observation time.",
    "missing_dependency": "Install the optional dependency required by this source.",
    "resource_limit": "Review the configured resource limit and the input size.",
    "output_conflict": "Choose a different output path or explicitly enable overwrite.",
}


def _human_projection(value: Any) -> Any:
    """Add documented recovery hints to the display copy only."""
    if isinstance(value, list):
        return [_human_projection(item) for item in value]
    if not isinstance(value, Mapping):
        return value
    projected = {key: _human_projection(item) for key, item in value.items()}
    error = projected.get("error")
    code = error.get("code") if isinstance(error, Mapping) else None
    if isinstance(code, str) and code in _RECOVERY_ADVICE:
        projected["Suggestion"] = _RECOVERY_ADVICE[code]
    if "items" in projected and isinstance(projected["items"], list):
        projected["items"] = [_human_projection(item) for item in projected["items"]]
    return projected


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
        sys.stdout.write(json.dumps(safe_value(_json_safe(payload)), ensure_ascii=False, sort_keys=True) + "\n")
        return
    if quiet:
        return
    projection = value.as_dict() if hasattr(value, "as_dict") else value
    sys.stdout.write(render(_human_projection(safe_value(_json_safe(projection))), command=command) + "\n")


def emit_error(exc: Exception, *, as_json: bool = False) -> None:
    if isinstance(exc, RadiustError):
        error = exc.as_dict()
    else:
        # Unknown exception text can contain an entire provider response body,
        # including secrets without recognizable key names. Retain messages for
        # CLI validation errors only; never publish arbitrary runtime payloads.
        import click

        message = str(exc) if isinstance(exc, (click.ClickException, ValueError)) else "Unexpected operation failure"
        error = {"code": "error", "message": message, "stage": "validate", "retryable": False}
    if as_json:
        sys.stdout.write(json.dumps(safe_value({"schema_version": 1, "command": None, "run_id": None, "query": None, "counts": {}, "items": [], "error": error, "interrupted": False}), ensure_ascii=False, sort_keys=True) + "\n")
    else:
        sys.stderr.write(f"{safe_text(error['code'])}: {safe_text(error['message'])}\n")


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
