from __future__ import annotations

from pathlib import Path
from typing import Any, Protocol

from ..field import RadarDataset, RadarField


class Encoder(Protocol):
    format: str

    def write(self, value: RadarField | RadarDataset, path: Path, *, options: dict[str, Any] | None = None) -> list[Path]: ...
