"""Opt-in structured logging with a deliberately small safe field set."""

from __future__ import annotations

import logging
import re
from typing import Any

SAFE_FIELDS = {"run_id", "frame_id", "stage", "source", "duration", "cache_hit", "bytes", "retries", "status"}
SECRET_PATTERN = re.compile(
    r"(?i)(authorization|cookie|x-api-key|api[_-]?key|access[-_]?key|secret|token|signature|sig)=([^&\s]+)"
)


def redact(value: str) -> str:
    return SECRET_PATTERN.sub(lambda match: f"{match.group(1)}=<redacted>", value)


def safe_event(**fields: Any) -> dict[str, Any]:
    result = {key: fields[key] for key in SAFE_FIELDS if key in fields}
    return {key: redact(str(value)) if isinstance(value, str) else value for key, value in result.items()}


def get_logger(name: str = "radiust") -> logging.Logger:
    return logging.getLogger(name)
