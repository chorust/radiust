"""Packaged legacy display rule selection with independent evidence gating.

The built-in index lists no passed source rule until a lawful matched old
baseline and explicit review have been packaged. An arbitrary image path or
the scientific palette registry never implies a legacy display rule.
"""

from __future__ import annotations

import json
from functools import lru_cache
from importlib import resources
from pathlib import Path
from typing import Any

from ..raw import RawFrame
from .engine import DisplayBlockedError
from .rules import DisplayEvidence, LegacyDisplayRule


def _read_json(root: Path, value: object) -> dict[str, Any]:
    if not isinstance(value, str) or not value or ":" in value or "\\" in value:
        raise DisplayBlockedError("matched legacy display resource path is unsafe")
    relative = Path(value)
    if relative.is_absolute() or ".." in relative.parts or relative.suffix != ".json":
        raise DisplayBlockedError("matched legacy display resource path is unsafe")
    full = (root / relative).resolve()
    if not full.is_relative_to(root) or not full.is_file() or full.stat().st_size > 4 * 1024 * 1024:
        raise DisplayBlockedError("matched legacy display resource is absent or exceeds its limit")
    try:
        data = json.loads(full.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise DisplayBlockedError("matched legacy display resource is not valid JSON") from exc
    if not isinstance(data, dict):
        raise DisplayBlockedError("matched legacy display resource is not an object")
    return data


class LegacyDisplayRegistry:
    """Index source/product/subpath rules without creating provider fixtures."""

    def __init__(self, root: Path, *, index: str = "index.json") -> None:
        self.root = root.resolve()
        data = _read_json(self.root, index)
        if data.get("schema_version") != 1 or not isinstance(data.get("paths"), list):
            raise DisplayBlockedError("legacy display index has an unsupported schema")
        entries: dict[str, dict[str, Any]] = {}
        for item in data["paths"]:
            if not isinstance(item, dict):
                raise DisplayBlockedError("legacy display index has a malformed path")
            source, product, path_id = (item.get(key) for key in ("source", "product", "path_id"))
            if (not isinstance(source, str) or not isinstance(product, str) or not isinstance(path_id, str)
                    or not (path_id == f"{source}/{product}" or path_id.startswith(f"{source}/{product}/"))):
                raise DisplayBlockedError("legacy display index has a mismatched path identity")
            if path_id in entries:
                raise DisplayBlockedError("legacy display index contains a duplicate path")
            if item.get("status") not in {"passed", "difference_pending", "blocked"}:
                raise DisplayBlockedError("legacy display index has an unknown status")
            entries[path_id] = item
        self._entries = entries

    def select(self, raw: RawFrame) -> tuple[LegacyDisplayRule | None, DisplayEvidence | None, str]:
        """Return a verified rule or a display-only fallback reason.

        A *matched* passed rule with missing or invalid evidence is an error,
        never a reason to silently fall back and label the image as migrated.
        """
        ref = raw.ref
        product_path = f"{ref.source}/{ref.product}"
        candidates = ([f"{product_path}/{ref.station}"] if ref.station else []) + [product_path]
        match = next((self._entries[path] for path in candidates if path in self._entries), None)
        if match is None:
            return None, None, "no validated legacy display rule for this source and product"
        if match["status"] != "passed":
            return None, None, "legacy display rule blocked: no source-matched reviewed baseline"
        try:
            rule = LegacyDisplayRule.from_mapping(_read_json(self.root, match.get("rule_file")))
            evidence = DisplayEvidence.from_mapping(_read_json(self.root, match.get("evidence_file")))
            if (not rule.matches(ref.source, ref.product, match["path_id"], match.get("rule_version"))
                    or rule.config_hash != match.get("config_hash") or rule.validation_status != "passed"
                    or not evidence.validates(rule)):
                raise DisplayBlockedError("matched legacy display evidence is stale or refers to another source")
            station = rule.input_constraints.get("station")
            if station is not None and station != ref.station:
                raise DisplayBlockedError("matched legacy display rule station differs from selected frame")
            return rule, evidence, ""
        except (OSError, KeyError, TypeError, ValueError) as exc:
            raise DisplayBlockedError("matched legacy display rule failed evidence or identity validation") from exc


@lru_cache(maxsize=1)
def default_registry() -> LegacyDisplayRegistry:
    root = Path(resources.files("radiust.resources").joinpath("legacy_display"))
    return LegacyDisplayRegistry(root)


__all__ = ["LegacyDisplayRegistry", "default_registry"]
