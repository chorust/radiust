"""Safe serialization of untrusted provider metadata for terminal and JSON output."""

from __future__ import annotations

import re
from collections.abc import Mapping
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

_SENSITIVE = re.compile(
    r"(?i)(?:token|secret|password|passwd|authorization|api[_-]?key|access[_-]?key|credential|cookie|bearer|signature|(?<![a-z0-9])sig(?![a-z0-9]))"
)
_URL = re.compile(r"https?://[^\s<>\"']+")
_BEARER = re.compile(r"(?i)\b(Bearer\s+)[^\s\x00-\x1f]+")
_AUTH = re.compile(r"(?i)\b(authorization\s*[:=]\s*)(?:(?:Bearer|Basic|Token)\s+)?[^\s\x00-\x1f]+")
_SECRET_NAME = r"(?:token|secret|password|passwd|authorization|(?:x[_-])?api[_-]?key|access[_-]?(?:key|token)|secret[_-]?key|credential|cookie|bearer|refresh[_-]?token|client[_-]?secret|(?:[a-z0-9]+[_-])*(?:signature|sig))"
_ASSIGNMENT = re.compile(r"(?i)\b((?:" + _SECRET_NAME + r")\s*[:=]\s*)[^\s,;&]+")
_SENSITIVE_KEY = re.compile(r"(?i)^" + _SECRET_NAME + r"$")


def _redact_url(match: re.Match[str]) -> str:
    value = match.group(0)
    try:
        parts = urlsplit(value)
        host = parts.hostname or ""
        if ":" in host and not host.startswith("["):
            host = f"[{host}]"
        if parts.port:
            host = f"{host}:{parts.port}"
        query = urlencode([(key, "[REDACTED]" if _SENSITIVE.search(key) else val) for key, val in parse_qsl(parts.query, keep_blank_values=True)])
        return urlunsplit((parts.scheme, host, parts.path, query, ""))
    except (ValueError, UnicodeError):
        return "[REDACTED URL]"


def safe_text(value: object) -> str:
    result = str(value)
    result = _URL.sub(_redact_url, result)
    result = _BEARER.sub(r"\1[REDACTED]", result)
    result = _AUTH.sub(r"\1[REDACTED]", result)
    result = _ASSIGNMENT.sub(r"\1[REDACTED]", result)
    # Preserve the information that a control byte was received without emitting it.
    return "".join(f"\\x{ord(char):02x}" if ord(char) < 32 or ord(char) == 127 or 0x80 <= ord(char) <= 0x9f else char for char in result)


def safe_value(value: object) -> object:
    if isinstance(value, Mapping):
        return {safe_text(key): "[REDACTED]" if _SENSITIVE_KEY.fullmatch(str(key)) else safe_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [safe_value(item) for item in value]
    if isinstance(value, str):
        return safe_text(value)
    return value
