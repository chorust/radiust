from __future__ import annotations

from dataclasses import dataclass

import numpy as np


@dataclass(frozen=True, slots=True)
class RawPreview:
    rgba: np.ndarray
    file_identity: str
    format: str
    sha256: str
    source: str | None = None
    product: str | None = None
    station: str | None = None
    valid_time: str | None = None
    frame_index: int = 0
    display_mode: str = "original"
    rule_version: str | None = None
    reason: str | None = "original source pixels preserved"

    @property
    def width(self) -> int:
        return int(self.rgba.shape[1])

    @property
    def height(self) -> int:
        return int(self.rgba.shape[0])
