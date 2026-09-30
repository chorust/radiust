"""The synthetic CLI support layer never claims to reproduce legacy display."""

from __future__ import annotations

from pathlib import Path

import pytest
from PIL import Image
from radiust.client import Client

from tests.support.cli_experience import (
    CallCounts,
    animated_gif_bytes,
    fixed_clock,
    fixture_metadata,
    forbid_network,
    image_bytes,
    install_raw_source,
    multiple_frames,
)


def test_fixed_clock_and_multisource_candidate_matrix():
    refs = multiple_frames()
    assert fixed_clock().tzinfo is not None
    assert len(refs) == 4
    assert {ref.product for ref in refs} == {"composite", "rain"}
    assert {ref.station for ref in refs} == {"a", "b"}
    assert len({ref.valid_time for ref in refs}) == 4


def test_replay_fixture_metadata_is_validated_not_assumed_licensed():
    info = fixture_metadata(Path("tests/fixtures/sources/my/fixture.json"))
    assert info["source"] == "my"
    assert info["frames"]
    assert "authorization to redistribute" in info["license_basis"]


@pytest.mark.parametrize("format_name", ("PNG", "GIF"))
def test_offline_png_gif_acquirer_and_no_science(monkeypatch, format_name):
    counts = install_raw_source(monkeypatch, format_name=format_name)
    with Client() as client:
        refs = client.discover(None)
        with client.acquire(refs[0]) as raw:
            with Image.open(__import__("io").BytesIO(raw.bytes())) as image:
                assert image.format == format_name and image.size == (3, 2)
            with pytest.raises(AssertionError, match="scientific"):
                client.fetch(refs[0])
    assert (counts.discover, counts.acquire, counts.scientific, counts.network) == (1, 1, 1, 0)


def test_synthetic_animated_gif_contains_distinct_frames():
    with Image.open(__import__("io").BytesIO(animated_gif_bytes())) as image:
        assert image.n_frames == 2
        image.seek(0)
        first = image.convert("RGBA").getpixel((0, 0))
        image.seek(1)
        assert image.convert("RGBA").getpixel((0, 0)) != first
    assert image_bytes().startswith(b"\x89PNG")


def test_native_cli_uses_rust_catalog_without_python_network_calls(monkeypatch):
    import builtins
    import json

    from click.testing import CliRunner
    from radiust.cli.main import main

    original_import = builtins.__import__

    def reject_legacy_source_runtime(name, *args, **kwargs):
        if name == "radiust.discovery" or name.startswith("radiust.sources"):
            raise AssertionError(f"native CLI imported legacy Python runtime module: {name}")
        return original_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", reject_legacy_source_runtime)
    counter = CallCounts()
    forbid_network(monkeypatch, counter)
    result = CliRunner().invoke(main, ["list", "sources", "--json"])
    assert result.exit_code == 0, result.output
    document = json.loads(result.output)
    ids = {item["id"] for item in document["items"]}
    assert len(ids) == 24
    assert "stub" not in ids
    assert counter.network == 0
