from __future__ import annotations

import io

from radiust.terminal.capabilities import capabilities


class _TTY(io.StringIO):
    def isatty(self) -> bool:
        return True


def test_non_tty_and_dumb_terminal_degrade_to_text(monkeypatch):
    monkeypatch.delenv("KITTY_WINDOW_ID", raising=False)
    monkeypatch.delenv("TERM_PROGRAM", raising=False)
    assert capabilities(io.StringIO()).renderer == "text"

    monkeypatch.setenv("TERM", "dumb")
    assert capabilities(_TTY()).renderer == "text"


def test_capabilities_select_confirmed_protocols(monkeypatch):
    monkeypatch.delenv("NO_COLOR", raising=False)
    monkeypatch.delenv("COLORTERM", raising=False)
    monkeypatch.setenv("TERM", "xterm-256color")
    monkeypatch.setenv("KITTY_WINDOW_ID", "123")
    assert capabilities(_TTY()).renderer == "kitty"

    monkeypatch.delenv("KITTY_WINDOW_ID")
    monkeypatch.setenv("TERM_PROGRAM", "iTerm.app")
    assert capabilities(_TTY()).renderer == "iterm2"
