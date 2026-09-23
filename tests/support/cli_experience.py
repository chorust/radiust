"""Deterministic data for CLI behavior tests; never a source migration baseline."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
from io import BytesIO
from pathlib import Path
from typing import Any

from PIL import Image
from radiust.models import Artifact, FrameRef, SourceInfo
from radiust.raw import RawFrame

from .fixtures import load_fixture

FIXED_NOW = datetime(2026, 9, 22, tzinfo=timezone.utc)


def image_bytes(format_name: str = "PNG") -> bytes:
    image = Image.new("RGBA", (3, 2), (0, 0, 0, 0))
    image.putpixel((1, 1), (0, 0, 0, 255))
    output = BytesIO()
    image.save(output, format=format_name)
    return output.getvalue()


def animated_gif_bytes() -> bytes:
    """Synthetic first/second frames make GIF frame-selection assertions deterministic."""
    first = Image.new("RGBA", (3, 2), (1, 2, 3, 255))
    second = Image.new("RGBA", (3, 2), (4, 5, 6, 255))
    output = BytesIO()
    first.save(output, format="GIF", save_all=True, append_images=[second], duration=[100, 100], loop=0)
    return output.getvalue()


def frame(source: str = "th", product: str = "composite", station: str | None = "a", *, hour: int = 0) -> FrameRef:
    return FrameRef(source, product, FIXED_NOW.replace(hour=hour), station=station)


def multiple_frames() -> tuple[FrameRef, ...]:
    """Two products, two stations and two distinct times; no provider access."""
    return (frame(product="composite", station="a", hour=0),
            frame(product="composite", station="b", hour=1),
            frame(product="rain", station="a", hour=2),
            frame(product="rain", station="b", hour=3))


def fixed_clock() -> datetime:
    """Use as a clock injection in tests; never use a historical fixture as live latest."""
    return FIXED_NOW


def fixture_metadata(path: Path) -> dict[str, Any]:
    """Validate actual replay fixture metadata; synthetic images are NOT migration evidence."""
    return load_fixture(path)


def install_catalog(monkeypatch: Any, catalog: tuple[SourceInfo, ...]) -> None:
    """Inject a fixed listing at both existing CLI and discovery catalog boundaries."""
    import radiust.cli.main as cli_main
    import radiust.discovery as discovery

    monkeypatch.setattr(discovery, "sources", lambda: catalog)
    monkeypatch.setattr(cli_main, "sources", lambda: catalog)


def forbid_network(monkeypatch: Any, counts: CallCounts) -> None:
    """Count and fail any accidental socket connection in a synthetic CLI test."""
    import socket

    def denied(*_args: Any, **_kwargs: Any) -> None:
        counts.network += 1
        raise AssertionError("synthetic CLI test attempted a network connection")

    monkeypatch.setattr(socket.socket, "connect", denied)
    monkeypatch.setattr(socket.socket, "connect_ex", denied)


@dataclass
class CallCounts:
    discover: int = 0
    acquire: int = 0
    scientific: int = 0
    network: int = 0


def install_raw_source(monkeypatch, *, refs: tuple[FrameRef, ...] | None = None, raw: bytes | None = None, format_name: str = "PNG") -> CallCounts:
    """Mock only the Client boundary; any science or public network call fails."""
    from radiust.client import Client

    refs = refs if refs is not None else multiple_frames()
    payload = raw if raw is not None else (animated_gif_bytes() if format_name == "GIF" else image_bytes(format_name))
    counts = CallCounts()

    def discover(_client, _query):
        counts.discover += 1
        return list(refs)

    class Acquire:
        def __init__(self, selected: FrameRef):
            suffix = format_name.lower()
            self.raw = RawFrame(selected, (Artifact(f"frame.{suffix}", "data", f"image/{suffix}", payload),))

        def __enter__(self):
            return self.raw

        def __exit__(self, *_args):
            self.raw.close()

    def acquire(_client, selected):
        counts.acquire += 1
        return Acquire(selected)

    def forbid_science(*_args, **_kwargs):
        counts.scientific += 1
        raise AssertionError("raw preview must not invoke scientific decoding")

    monkeypatch.setattr(Client, "discover", discover)
    monkeypatch.setattr(Client, "acquire", acquire)
    monkeypatch.setattr(Client, "fetch", forbid_science)
    return counts
