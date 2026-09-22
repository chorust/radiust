"""Per-operation context shared by source adapters."""

from __future__ import annotations

import asyncio
import tempfile
import threading
from collections.abc import Mapping
from contextlib import suppress
from dataclasses import dataclass
from dataclasses import field as dc_field
from pathlib import Path
from typing import Any

from .config import EffectiveConfig
from .errors import ResourceLimitError


@dataclass
class Cancellation:
    event: threading.Event = dc_field(default_factory=threading.Event)

    def cancel(self) -> None:
        self.event.set()

    def check(self) -> None:
        if self.event.is_set():
            raise asyncio.CancelledError


@dataclass
class SourceContext:
    config: EffectiveConfig
    source_id: str
    cancellation: Cancellation = dc_field(default_factory=Cancellation)
    transport: Any = None
    cache: Any = None
    temp_root: Path | None = None
    metadata: dict[str, Any] = dc_field(default_factory=dict)

    def __post_init__(self) -> None:
        if self.temp_root is None:
            self.temp_root = Path(tempfile.mkdtemp(prefix="radiust-"))
        self.temp_root.mkdir(parents=True, exist_ok=True)

    @property
    def limits(self) -> Mapping[str, Any]:
        return self.config.values["runtime"]

    def check_bytes(self, size: int) -> None:
        limit = int(self.limits["max_artifact_bytes"])
        if size > limit:
            raise ResourceLimitError(f"artifact is {size} bytes, limit is {limit}")

    def check_frame_bytes(self, size: int) -> None:
        limit = int(self.limits["max_frame_bytes"])
        if size > limit:
            raise ResourceLimitError(f"frame is {size} bytes, limit is {limit}")

    def check_artifacts(self, artifacts: Any) -> None:
        total = 0
        for artifact in artifacts:
            payload = artifact.payload
            size = len(payload) if isinstance(payload, bytes) else Path(payload).stat().st_size
            self.check_bytes(size)
            total += size
        self.check_frame_bytes(total)

    def check_pixels(self, pixels: int) -> None:
        limit = int(self.limits["max_pixels"])
        if pixels > limit:
            raise ResourceLimitError(f"decoded field has {pixels} pixels, limit is {limit}")

    def check_temp_bytes(self, size: int) -> None:
        limit = int(self.limits["max_temp_bytes"])
        if size > limit:
            raise ResourceLimitError(f"temporary data is {size} bytes, limit is {limit}")

    def check_value(self, value: Any) -> None:
        data = getattr(value, "data", None)
        if data is None:
            return
        if hasattr(data, "data_vars"):
            arrays = tuple(data.data_vars.values())
            pixels = sum(int(array.size) for array in arrays)
            byte_size = sum(int(getattr(array.values, "nbytes", 0)) for array in arrays)
        else:
            pixels = int(data.size)
            byte_size = int(getattr(data.values, "nbytes", 0))
        quality = getattr(value, "quality", None)
        if quality is not None:
            pixels += int(quality.size)
            byte_size += int(getattr(quality.values, "nbytes", 0))
        self.check_pixels(pixels)
        self.check_temp_bytes(byte_size)

    def detach_temp_root(self) -> Path | None:
        root = self.temp_root
        self.temp_root = None
        return root

    def close(self) -> None:
        if self.temp_root is None:
            return
        for path in sorted(self.temp_root.rglob("*"), reverse=True):
            if path.is_file() or path.is_symlink():
                path.unlink(missing_ok=True)
            elif path.is_dir():
                with suppress(OSError):
                    path.rmdir()
        with suppress(OSError):
            self.temp_root.rmdir()
