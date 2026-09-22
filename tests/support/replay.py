"""Local fixture replay transport; it never registers as a product source."""

from __future__ import annotations

from pathlib import Path

from .fixtures import load_fixture


class FixtureReplay:
    def __init__(self, fixture: str | Path):
        self.fixture_path = Path(fixture)
        self.data = load_fixture(self.fixture_path)

    def artifact(self, relative: str) -> bytes:
        path = self.fixture_path.parent / relative
        return path.read_bytes()
