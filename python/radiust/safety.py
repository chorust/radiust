"""Thin Python facade for Rust secret and terminal-control sanitization."""

from __future__ import annotations

import json
from typing import Any

from . import _bridge


def _native() -> Any:
    if _bridge._core is None:
        raise ImportError("safe output requires the compiled radiust Rust extension")
    return _bridge._core


def safe_text(value: object) -> str:
    return _native().safety_text(str(value))


def safe_value(value: object) -> object:
    payload = json.dumps(
        _bridge._json_safe(value), ensure_ascii=False, allow_nan=False, separators=(",", ":")
    )
    return json.loads(_native().safety_value_json(payload))
