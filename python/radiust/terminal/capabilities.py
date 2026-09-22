"""Conservative terminal capability detection with a bounded decision path."""

from __future__ import annotations

import os
import sys
from dataclasses import dataclass
from typing import TextIO


@dataclass(frozen=True, slots=True)
class TerminalCapabilities:
    is_tty: bool
    renderer: str
    color: bool
    kitty: bool
    iterm2: bool


def capabilities(stream: TextIO | None = None) -> TerminalCapabilities:
    stream = stream or sys.stdout
    is_tty = bool(getattr(stream, "isatty", lambda: False)())
    term = os.environ.get("TERM", "")
    no_color = "NO_COLOR" in os.environ or term == "dumb" or os.environ.get("COLORTERM") == "0"
    kitty = bool(os.environ.get("KITTY_WINDOW_ID")) and is_tty and not no_color
    iterm2 = os.environ.get("TERM_PROGRAM") == "iTerm.app" and is_tty and not no_color
    if not is_tty or no_color:
        renderer = "text"
    elif kitty:
        renderer = "kitty"
    elif iterm2:
        renderer = "iterm2"
    else:
        renderer = "ansi"
    return TerminalCapabilities(is_tty, renderer, not no_color, kitty, iterm2)
