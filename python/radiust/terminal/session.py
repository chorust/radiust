from __future__ import annotations

import sys
import termios
from contextlib import AbstractContextManager, suppress
from typing import TextIO


class TerminalSession(AbstractContextManager["TerminalSession"]):
    """Restore termios state if a renderer is interrupted."""

    def __init__(self, stream: TextIO | None = None) -> None:
        self.stream = stream or sys.stdout
        self._fd: int | None = None
        self._state: list[int] | None = None

    def __enter__(self) -> TerminalSession:
        try:
            if self.stream.isatty():
                self._fd = self.stream.fileno()
                self._state = termios.tcgetattr(self._fd)
        except (AttributeError, OSError, termios.error):
            self._fd, self._state = None, None
        return self

    def __exit__(self, exc_type: object, exc: object, tb: object) -> None:
        if self._fd is not None and self._state is not None:
            with suppress(OSError, termios.error):
                termios.tcsetattr(self._fd, termios.TCSADRAIN, self._state)
