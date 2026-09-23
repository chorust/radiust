"""Optional CLI-only progress sink; never writes terminal controls to stdout."""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass
from typing import TextIO

from .safety import safe_text


@dataclass(frozen=True, slots=True)
class ProgressEvent:
    stage: str
    completed: int
    total: int | None = None


class Progress:
    def __init__(self, *, stream: TextIO | None = None, quiet: bool = False):
        self.stream = stream if stream is not None else sys.stderr
        self.active = bool(getattr(self.stream, "isatty", lambda: False)()) and not quiet
        self._dynamic = "NO_COLOR" not in os.environ and os.environ.get("TERM") != "dumb"
        self._used = False

    def __enter__(self) -> Progress:
        return self

    def update(self, event: ProgressEvent) -> None:
        if not self.active:
            return
        suffix = str(event.completed) if event.total is None else f"{event.completed}/{event.total}"
        value = f"{safe_text(event.stage)} {suffix}"
        self.stream.write(("\r\x1b[2K" if self._dynamic else "") + value + ("" if self._dynamic else "\n"))
        self.stream.flush()
        self._used = True

    def close(self) -> None:
        if self.active and self._used and self._dynamic:
            self.stream.write("\r\x1b[2K")
            self.stream.flush()
        self._used = False

    def __exit__(self, *_args: object) -> None:
        self.close()
