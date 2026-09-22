from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from typing import Any

from ..config import load_config
from ..registry import get_source


def run_doctor(*, source: str | None = None, network: bool = False, config_path: str | Path | None = None) -> dict[str, Any]:
    config = load_config(path=config_path)
    root = config.output_root
    cache = config.cache_dir
    if root is not None:
        root.parent.mkdir(parents=True, exist_ok=True)
    cache.parent.mkdir(parents=True, exist_ok=True)
    output_writable: bool | str = "remote_unverified" if root is None else os.access(root.parent, os.W_OK)
    checks: dict[str, Any] = {
        "python": True,
        "dependencies": {name: importlib.util.find_spec(name) is not None for name in ("numpy", "xarray", "yaml", "PIL", "h5netcdf", "h5py", "click")},
        "output_writable": output_writable,
        "cache_writable": os.access(cache.parent, os.W_OK),
        "network_probe": "not_requested",
        "source": source,
    }
    if network:
        checks["network_probe"] = "requested_but_source_probe_is_adapter_specific"
        if source:
            checks["source_availability"] = get_source(source).info.availability
    checks["ok"] = all(checks["dependencies"].values()) and output_writable is True and checks["cache_writable"]
    return {"schema_version": 1, "command": "doctor", "checks": checks}
