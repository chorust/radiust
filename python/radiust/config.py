"""SDK-compatible view of configuration resolved by the Rust core."""

from __future__ import annotations

import copy
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from . import _bridge


def _is_remote_output(value: object) -> bool:
    return isinstance(value, str) and value.lower().startswith(("s3://", "oss://"))


@dataclass(frozen=True, slots=True)
class EffectiveConfig:
    """The stable Python SDK view over Rust's resolved configuration."""

    values: dict[str, Any]
    origins: dict[str, str]
    _native_config: Any = field(default=None, repr=False, compare=False)
    _redacted_values: dict[str, Any] | None = field(default=None, repr=False, compare=False)
    _resolved_values_snapshot: dict[str, Any] | None = field(
        default=None, repr=False, compare=False
    )

    @property
    def output_root(self) -> Path | None:
        value = self.values["storage"]["output"]
        return None if _is_remote_output(value) else Path(value).expanduser()

    @property
    def cache_dir(self) -> Path:
        return Path(self.values["cache"]["dir"]).expanduser()

    def redacted(self) -> dict[str, Any]:
        resolved_unchanged = self._resolved_values_snapshot == self.values
        source = (
            self._redacted_values
            if resolved_unchanged and self._redacted_values is not None
            else _bridge.redact_config_values(self.values)
        )
        result = copy.deepcopy(source)
        result["_origins"] = dict(self.origins)
        return result


def load_config(
    overrides: Mapping[str, Any] | None = None,
    *,
    path: str | Path | None = None,
    environ: Mapping[str, str] | None = None,
) -> EffectiveConfig:
    """Merge user/project YAML, environment, and overrides using the Rust core.

    User YAML is ~/.config/radiust/config.yaml; project YAML is ./config.yaml.
    An explicit path replaces the project layer. Missing automatic files skip.
    """
    native, values, origins, redacted = _bridge.resolve_config(
        overrides,
        path=path,
        environ=environ,
    )
    return EffectiveConfig(
        values=values,
        origins=origins,
        _native_config=native,
        _redacted_values=redacted,
        _resolved_values_snapshot=copy.deepcopy(values),
    )
