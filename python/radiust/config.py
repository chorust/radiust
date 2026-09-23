"""Safe, explainable configuration loading."""

from __future__ import annotations

import copy
import json
import math
import os
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml

from .errors import ConfigError

DEFAULTS: dict[str, Any] = {
    "runtime": {
        "frame_concurrency": 2,
        "request_concurrency": 16,
        "host_concurrency": 4,
        "decode_workers": 2,
        "request_timeout": 30.0,
        "frame_deadline": 300.0,
        "discovery_deadline": 300.0,
        "max_artifact_bytes": 512 * 1024 * 1024,
        "max_frame_bytes": 2 * 1024 * 1024 * 1024,
        "max_pixels": 100_000_000,
        "max_temp_bytes": 10 * 1024 * 1024 * 1024,
        "allow_network": False,
        "temp_root": None,
    },
    "cache": {
        "dir": str(Path.home() / ".cache" / "radiust"),
        "max_bytes": 20_000_000_000,
        "max_age_days": 30,
        "gc_interval_hours": 24,
        "enabled": True,
    },
    "storage": {"output": "./data", "endpoint": None, "region": None, "access_key": None, "secret_key": None, "anonymous": False},
    "sources": {},
    "output": {"format": "netcdf", "grid": "native"},
}
KNOWN_SECTIONS = set(DEFAULTS)
SECTION_FIELDS = {
    "runtime": {
        "frame_concurrency", "request_concurrency", "host_concurrency", "decode_workers",
        "request_timeout", "frame_deadline", "discovery_deadline", "max_artifact_bytes", "max_frame_bytes",
        "max_pixels", "max_temp_bytes", "allow_network",
        "temp_root",
    },
    "cache": {"dir", "max_bytes", "max_age_days", "gc_interval_hours", "enabled"},
    "storage": {"output", "endpoint", "region", "access_key", "secret_key", "anonymous"},
    "output": {"format", "grid", "resampling"},
}


def _is_remote_output(value: object) -> bool:
    return isinstance(value, str) and value.lower().startswith(("s3://", "oss://"))


class UniqueKeyLoader(yaml.SafeLoader):
    pass


def _construct_mapping(loader: UniqueKeyLoader, node: yaml.MappingNode, deep: bool = False) -> dict[str, Any]:
    mapping: dict[str, Any] = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in mapping:
            raise ConfigError(f"duplicate configuration key: {key}")
        mapping[key] = loader.construct_object(value_node, deep=deep)
    return mapping


UniqueKeyLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _construct_mapping)


def _merge(base: dict[str, Any], override: Mapping[str, Any]) -> dict[str, Any]:
    result = copy.deepcopy(base)
    for key, value in override.items():
        if key not in KNOWN_SECTIONS and not str(key).startswith("RADIUST_TEST_"):
            raise ConfigError(f"unknown configuration section: {key}")
        if key in result and isinstance(result[key], dict) and isinstance(value, Mapping):
            result[key] = _merge_section(result[key], value, section=str(key))
        else:
            result[key] = copy.deepcopy(value)
    return result


def _merge_section(base: dict[str, Any], override: Mapping[str, Any], *, section: str | None = None) -> dict[str, Any]:
    result = copy.deepcopy(base)
    for key, value in override.items():
        if section in SECTION_FIELDS and str(key) not in SECTION_FIELDS[section]:
            raise ConfigError(f"unknown configuration field: {section}.{key}")
        if key in result and isinstance(result[key], dict) and isinstance(value, Mapping):
            result[key] = _merge_section(result[key], value, section=str(key))
        else:
            result[key] = copy.deepcopy(value)
    return result


def _parse_env(environ: Mapping[str, str]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, raw in environ.items():
        if not key.startswith("RADIUST_") or key.startswith("RADIUST_TEST_"):
            continue
        path = key.removeprefix("RADIUST_").lower().split("__")
        if len(path) == 3 and path[0] == "sources":
            section = result.setdefault("sources", {})
            source = section.setdefault(path[1], {})
            value: Any = raw
            try:
                value = json.loads(raw)
            except json.JSONDecodeError:
                if raw.lower() in {"true", "false"}:
                    value = raw.lower() == "true"
            source[path[2]] = value
            continue
        if len(path) != 2 or path[0] not in KNOWN_SECTIONS:
            raise ConfigError(f"unknown environment configuration key: {key}")
        value: Any = raw
        try:
            value = json.loads(raw)
        except json.JSONDecodeError:
            if raw.lower() in {"true", "false"}:
                value = raw.lower() == "true"
        result.setdefault(path[0], {})[path[1]] = value
    return result


def _validate_credentials(config: Mapping[str, Any]) -> None:
    storage = config.get("storage", {})
    if isinstance(storage, Mapping):
        has_access = bool(storage.get("access_key"))
        has_secret = bool(storage.get("secret_key"))
        if has_access != has_secret:
            raise ConfigError("storage must provide access_key and secret_key together")
    for name, values in config.get("sources", {}).items():
        if not isinstance(values, Mapping):
            continue
        has_access = bool(values.get("access_key"))
        has_secret = bool(values.get("secret_key"))
        if has_access != has_secret:
            raise ConfigError(f"source {name} must provide access_key and secret_key together")


def _validate_values(config: Mapping[str, Any]) -> None:
    for section in KNOWN_SECTIONS:
        if not isinstance(config.get(section), Mapping):
            raise ConfigError(f"configuration section {section} must be a mapping")
    runtime = config.get("runtime", {})
    positive_ints = ("frame_concurrency", "request_concurrency", "host_concurrency", "decode_workers", "max_artifact_bytes", "max_frame_bytes", "max_pixels", "max_temp_bytes")
    for name in positive_ints:
        try:
            value = int(runtime[name])
        except (KeyError, TypeError, ValueError) as exc:
            raise ConfigError(f"runtime.{name} must be a positive integer") from exc
        if value < 1:
            raise ConfigError(f"runtime.{name} must be a positive integer")
    for name in ("request_timeout", "frame_deadline", "discovery_deadline"):
        try:
            value = float(runtime[name])
        except (KeyError, TypeError, ValueError) as exc:
            raise ConfigError(f"runtime.{name} must be positive") from exc
        if not math.isfinite(value) or value <= 0:
            raise ConfigError(f"runtime.{name} must be positive")
    if runtime.get("temp_root") is not None and not isinstance(runtime.get("temp_root"), (str, Path)):
        raise ConfigError("runtime.temp_root must be a path or null")
    cache = config.get("cache", {})
    try:
        if int(cache["max_bytes"]) < 0 or int(cache["max_age_days"]) <= 0 or int(cache["gc_interval_hours"]) <= 0:
            raise ValueError
    except (KeyError, TypeError, ValueError) as exc:
        raise ConfigError("cache limits must be non-negative and durations must be positive") from exc
    if config.get("output", {}).get("format", "netcdf") not in {"netcdf", "geotiff", "png", "zarr"}:
        raise ConfigError("output.format is unsupported")
    if config.get("output", {}).get("grid", "native") not in {"native", "geographic"}:
        raise ConfigError("output.grid is unsupported")


def _origin_paths(value: Mapping[str, Any], origin: str, prefix: str = "") -> dict[str, str]:
    result: dict[str, str] = {}
    for key, child in value.items():
        path = f"{prefix}.{key}" if prefix else str(key)
        result[path] = origin
        if isinstance(child, Mapping):
            result.update(_origin_paths(child, origin, path))
    return result


@dataclass(frozen=True, slots=True)
class EffectiveConfig:
    values: dict[str, Any]
    origins: dict[str, str]

    @property
    def output_root(self) -> Path | None:
        value = self.values["storage"]["output"]
        return None if _is_remote_output(value) else Path(value).expanduser()

    @property
    def cache_dir(self) -> Path:
        return Path(self.values["cache"]["dir"]).expanduser()

    def redacted(self) -> dict[str, Any]:
        result = copy.deepcopy(self.values)
        for key in ("access_key", "secret_key"):
            if result.get("storage", {}).get(key):
                result["storage"][key] = "<configured>"
        for section in ("sources",):
            for source in result.get(section, {}).values():
                if isinstance(source, dict):
                    for key in tuple(source):
                        if any(secret in key.lower() for secret in ("key", "secret", "token", "cookie", "password")):
                            source[key] = "<configured>"
        result["_origins"] = dict(self.origins)
        return result


def load_config(
    overrides: Mapping[str, Any] | None = None,
    *,
    path: str | Path | None = None,
    environ: Mapping[str, str] | None = None,
) -> EffectiveConfig:
    values = copy.deepcopy(DEFAULTS)
    origins = {section: "default" for section in DEFAULTS}
    if path is not None:
        config_path = Path(path).expanduser()
        try:
            parsed = yaml.load(config_path.read_text(encoding="utf-8"), Loader=UniqueKeyLoader) or {}
        except FileNotFoundError as exc:
            raise ConfigError(f"configuration file does not exist: {config_path}") from exc
        except yaml.YAMLError as exc:
            raise ConfigError(f"invalid YAML configuration: {exc}") from exc
        if not isinstance(parsed, Mapping):
            raise ConfigError("configuration root must be a mapping")
        values = _merge(values, parsed)
        origins.update(_origin_paths(parsed, "file"))
    env_values = _parse_env(environ if environ is not None else os.environ)
    values = _merge(values, env_values)
    origins.update(_origin_paths(env_values, "environment"))
    if overrides:
        values = _merge(values, overrides)
        origins.update(_origin_paths(overrides, "explicit"))
    _validate_credentials(values)
    _validate_values(values)
    output_value = values["storage"]["output"]
    output_root = None if _is_remote_output(output_value) else Path(output_value).expanduser().resolve()
    cache_dir = Path(values["cache"]["dir"]).expanduser().resolve()
    if output_root is not None and (output_root == cache_dir or output_root in cache_dir.parents or cache_dir in output_root.parents):
        raise ConfigError("cache directory and output root cannot overlap")
    values["storage"]["output"] = output_value if output_root is None else str(output_root)
    values["cache"]["dir"] = str(cache_dir)
    return EffectiveConfig(values, origins)
