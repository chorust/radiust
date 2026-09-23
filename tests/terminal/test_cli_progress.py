from io import StringIO

from radiust.cli.progress import Progress, ProgressEvent


class FakeTTY(StringIO):
    def isatty(self) -> bool:
        return True


def test_progress_tty_restores_line(monkeypatch):
    monkeypatch.delenv("NO_COLOR", raising=False)
    monkeypatch.setenv("TERM", "xterm-256color")
    stream = FakeTTY()
    with Progress(stream=stream) as progress:
        progress.update(ProgressEvent("discover", 2, 5))
    assert "discover" in stream.getvalue() and "2/5" in stream.getvalue()
    assert stream.getvalue().endswith("\r\x1b[2K")


def test_progress_has_no_non_tty_or_no_color_control_sequences(monkeypatch):
    stream = StringIO()
    with Progress(stream=stream) as progress:
        progress.update(ProgressEvent("download", 1, None))
    assert stream.getvalue() == ""
    monkeypatch.setenv("NO_COLOR", "1")
    tty = FakeTTY()
    with Progress(stream=tty) as progress:
        progress.update(ProgressEvent("download", 1, None))
    assert "\x1b" not in tty.getvalue()
    assert "%" not in tty.getvalue()
