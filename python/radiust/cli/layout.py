"""Labeled CLI records with a compact table and narrow-terminal fallback."""

from __future__ import annotations

import os
import shutil
import textwrap
import unicodedata
from collections.abc import Mapping
from typing import Any

from .safety import safe_text


def cell_width(value: str) -> int:
    return sum(0 if unicodedata.combining(char) or unicodedata.category(char) == "Cf" else 2 if unicodedata.east_asian_width(char) in {"W", "F"} else 1 for char in value)


def _pad_cell(value: str, width: int) -> str:
    return value + " " * max(0, width - cell_width(value))


def _fit_display_width(line: str, columns: int, indent: str) -> list[str]:
    """Wrap text by visible terminal columns, including wide characters."""
    result: list[str] = []
    current = ""
    for char in line:
        if current and cell_width(current) + cell_width(char) > columns:
            result.append(current)
            current = indent
        current += char
    result.append(current)
    return result


def _value(value: object) -> str:
    if value is None:
        return "unknown"
    if isinstance(value, Mapping):
        return "; ".join(f"{safe_text(key)}: {_value(item)}" for key, item in value.items())
    if isinstance(value, (list, tuple)):
        return ", ".join(_value(item) for item in value)
    return safe_text(value)


def _record(item: Mapping[str, Any], columns: int) -> list[str]:
    result: list[str] = []
    for key, value in item.items():
        if value is None:
            continue
        heading = f"{safe_text(key)}: "
        indent = " " * min(len(heading), columns // 3)
        content = _value(value)
        for line in textwrap.wrap(heading + content, width=columns, subsequent_indent=indent, break_long_words=True, break_on_hyphens=False) or [heading + "unknown"]:
            result.extend(_fit_display_width(line, columns, indent))
    return result


def render(value: object, *, command: str | None = None, columns: int | None = None) -> str:
    columns = max(20, columns or int(os.environ.get("COLUMNS") or shutil.get_terminal_size((80, 24)).columns))
    header = safe_text(command or ("download" if hasattr(value, "counts") else "result")).upper()
    if hasattr(value, "as_dict"):
        value = value.as_dict()
    if isinstance(value, Mapping):
        if value.get("schema_version") == 1 and "items" in value:
            items = value.get("items") or []
            report = dict(value)
            report.pop("items", None)
            report.pop("schema_version", None)
            result = [header, *_record(report, columns)]
            result += [f"Items: {len(items)}"]
            if not items:
                result.append("No data found")
            for number, item in enumerate(items, 1):
                result += [f"#{number}", *_record(item if isinstance(item, Mapping) else {"value": item}, columns)]
            return "\n".join(result)
        return "\n".join([header, *_record(value, columns)])
    if not isinstance(value, list):
        return "\n".join([header, *_record({"result": value}, columns)])
    items = [item if isinstance(item, Mapping) else {"value": item} for item in value]
    if command == "list" and items and "availability" in items[0]:
        items = [{"source": item["id"], **{key: val for key, val in item.items() if key != "id"}} for item in items]
    lines = [header, f"Total: {len(items)}"]
    if not items:
        return "\n".join([*lines, "No data found"])
    if columns >= 80:
        keys = list(dict.fromkeys(key for item in items for key in item))
        # Show a compact table only when all columns fit without clipping.
        widths = {key: max(cell_width(key), *(cell_width(_value(item.get(key))) for item in items)) for key in keys}
        if keys and sum(widths.values()) + 3 * (len(keys) - 1) <= columns:
            def line(item: Mapping[str, Any]) -> str:
                return " | ".join(_pad_cell(_value(item.get(key)), widths[key]) for key in keys)
            lines.extend([" | ".join(_pad_cell(key, widths[key]) for key in keys), *(line(item) for item in items)])
            return "\n".join(lines)
    for number, item in enumerate(items, 1):
        lines.extend([f"#{number}", *_record(item, columns)])
    return "\n".join(lines)
