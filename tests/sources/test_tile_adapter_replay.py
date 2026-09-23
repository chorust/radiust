from __future__ import annotations

import hashlib
import io
import json
from dataclasses import replace
from datetime import datetime, timezone
from urllib.parse import parse_qs, urlparse

import numpy as np
import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.display.engine import DisplayBlockedError
from radiust.display.rules import DisplayEvidence, LegacyDisplayRule, rule_fingerprint
from radiust.display.tiles import compose_verified_tiles
from radiust.errors import DecodeError, IntegrityError, UnsupportedQueryError
from radiust.identity import artifact_bytes, safe_ref
from radiust.models import Artifact, FrameRef, Query
from radiust.raw import RawFrame
from radiust.registry import registry
from radiust.sources.bmkg import BmkgSource
from radiust.sources.opensnow import OpenSnowSource
from radiust.sources.windy import WindySource
from radiust.sources.wunderground import WundergroundSource


class _TileReplayTransport:
    def __init__(self, payloads: tuple[bytes, ...]) -> None:
        self.payloads = payloads
        self.calls: list[str] = []

    async def get(self, url: str, *, headers=None) -> bytes:
        self.calls.append(url)
        return self.payloads[len(self.calls) - 1]


def _png(color: tuple[int, int, int, int], *, size: int = 256) -> bytes:
    buffer = io.BytesIO()
    Image.new("RGBA", (size, size), color).save(buffer, format="PNG")
    return buffer.getvalue()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("source_id", "source_type", "api_key"),
    [
        ("opensnow", OpenSnowSource, None),
        ("bmkg", BmkgSource, None),
        ("wunderground", WundergroundSource, "fixture-only-key"),
    ],
)
async def test_legacy_tile_adapters_replay_all_raw_tiles_without_claiming_science(
    tmp_path, monkeypatch, source_id, source_type, api_key
):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(source_type, "frame_time", lambda _self: valid_time)
    payloads = tuple(_png((index * 40, 30, 90, 255)) for index in range(4))
    transport = _TileReplayTransport(payloads)
    values = {"runtime": {"allow_network": True}}
    if api_key is not None:
        values["sources"] = {"wunderground": {"api_key": api_key}}
    context = SourceContext(
        load_config(values, environ={}),
        source_id,
        transport=transport,
        temp_root=tmp_path,
    )
    source = source_type(registry.get_info(source_id))
    try:
        ref = (await source.discover(Query(source_id, latest=True), context))[0]
        specs = ({"url": ref.uri, "name": ref.locator["name"]}, *ref.locator["artifacts"])
        assert len(specs) == 4
        assert ref.valid_time == valid_time
        assert ref.station == "global"
        assert "fixture-only-key" not in repr(ref)
        assert "fixture-only-key" not in json.dumps(safe_ref(ref))

        raw = await source.download(ref, context)
        try:
            assert len(transport.calls) == 4
            assert [artifact.name for artifact in raw.artifacts] == [spec["name"] for spec in specs]
            assert [artifact.role for artifact in raw.artifacts] == ["data", "tile", "tile", "tile"]
            for artifact, payload, request_url in zip(raw.artifacts, payloads, transport.calls, strict=True):
                assert artifact_bytes(artifact) == payload
                assert artifact.sha256 == hashlib.sha256(payload).hexdigest()
                query = parse_qs(urlparse(request_url).query)
                if api_key is None:
                    assert "apiKey" not in query
                else:
                    assert query["apiKey"] == [api_key]
                    assert request_url.count(api_key) == 1

            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("source_id", "source_type", "config"),
    [
        ("windy", WindySource, {}),
        ("opensnow", OpenSnowSource, {}),
        ("bmkg", BmkgSource, {}),
        ("wunderground", WundergroundSource, {"sources": {"wunderground": {"api_key": "fixture-only-key"}}}),
    ],
)
async def test_legacy_tile_adapters_reject_historical_queries_before_discovery(
    tmp_path, source_id, source_type, config
):
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}, **config}, environ={}),
        source_id,
        transport=_TileReplayTransport(()),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(UnsupportedQueryError, match="only supports latest frames"):
            await source_type(registry.get_info(source_id)).discover(
                Query(source_id, at=datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)),
                context,
            )
        assert context.transport.calls == []
    finally:
        context.close()


@pytest.mark.asyncio
async def test_legacy_tile_adapter_rejects_non_256_square_tile(tmp_path, monkeypatch):
    valid_time = datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc)
    monkeypatch.setattr(OpenSnowSource, "frame_time", lambda _self: valid_time)
    transport = _TileReplayTransport(tuple(_png((index * 40, 30, 90, 255), size=2) for index in range(4)))
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "opensnow",
        transport=transport,
        temp_root=tmp_path,
    )
    source = OpenSnowSource(registry.get_info("opensnow"))
    try:
        ref = (await source.discover(Query("opensnow", latest=True), context))[0]
        with pytest.raises(IntegrityError, match="256x256"):
            await source.download(ref, context)
    finally:
        context.close()


def _synthetic_display_tiles(*, source="synthetic", station="global", layout="synthetic-2x2"):
    """Strict synthetic tile-composition fixture; no provider gray baseline."""
    revision = "synthetic-frame-20260918T0410"
    tiles = []
    artifacts = []
    for y in range(2):
        for x in range(2):
            name = f"tile-z1-x{x}-y{y}.png"
            role = "data" if x == 0 and y == 0 else "tile"
            tiles.append({"x": x, "y": y, "name": name, "role": role,
                          "media_type": "image/png", "format": "PNG"})
            rgba = (20 + 30 * y + x, 0, 0, 0 if x == 0 and y == 1 else 255)
            artifacts.append(Artifact(name, role, "image/png", _png(rgba, size=2), source_revision=revision))
    ref = FrameRef(source, "composite", datetime(2026, 9, 18, 4, 10, tzinfo=timezone.utc),
                   station=station, revision=revision)
    raw = RawFrame(ref, tuple(artifacts), metadata={"tile_count": 4, "tile_size": 2, "tile_layout": layout})
    data = {
        "source": "synthetic", "product": "composite", "path_id": "synthetic/composite",
        "rule_version": "synthetic-v1", "encoding_version": "旧项目-gray-dbz-v1",
        "legacy_reference": "synthetic contract only, not evidence of any provider",
        "ordered_steps": [
            {"op": "palette", "basis": "synthetic fixture",
             "entries": [{"rgb": [20 + 30 * y + x, 0, 0], "dbz": 5 * (2 * y + x)}
                         for y in range(2) for x in range(2)]},
            {"op": "gray_encode", "basis": "synthetic fixture"},
        ],
        "input_constraints": {
            "formats": ["PNG"], "max_pixels": 16,
            "tile_plan": {"station": "global", "layout": "synthetic-2x2", "tile_width": 2,
                          "tile_height": 2, "columns": 2, "rows": 2, "tiles": tiles},
        },
        "validation_status": "passed",
    }
    data["config_hash"] = rule_fingerprint(data)
    rule = LegacyDisplayRule.from_mapping(data)
    fake_digest = hashlib.sha256(b"synthetic-only-not-a-provider-baseline").hexdigest()
    evidence = DisplayEvidence.from_mapping({
        "path_id": rule.path_id, "status": "passed", "rule_version": rule.rule_version,
        "config_hash": rule.config_hash, "input_hashes": [fake_digest],
        "output_hash": fake_digest, "baseline_identity": "synthetic only",
        "baseline_hash": fake_digest, "crop": [0, 0, 4, 4], "shape": [4, 4],
        "pixel_diff_count": 0, "alpha_comparison": "identical",
        "background_comparison": "identical", "missing_comparison": "identical",
        "intentional_differences": [], "review_conclusion": "synthetic only",
        "blocked_reasons": [], "sample_provenance": "generated synthetic tile fixture",
        "scientific_status_unchanged": True,
    })
    return raw, rule, evidence


def test_synthetic_verified_tile_plan_preserves_frame_alpha_and_order():
    raw, rule, evidence = _synthetic_display_tiles()
    before = [raw.bytes(artifact.name) for artifact in raw.artifacts]
    with raw:
        rgba = compose_verified_tiles(raw, rule, evidence, limits={"max_pixels": 16})
        assert rgba.shape == (4, 4, 4)
        assert [int(rgba[y, x, 0]) for y, x in ((0, 0), (0, 2), (2, 0), (2, 2))] == [20, 21, 50, 51]
        assert np.all(rgba[2:4, 0:2, 3] == 0)
        assert np.all(rgba[0:2, :, 3] == 255)
        rgba[0, 0, 0] = 99
        assert [raw.bytes(artifact.name) for artifact in raw.artifacts] == before


@pytest.mark.parametrize(("mutation", "message"), [
    ("missing", "incomplete"), ("duplicate_position", "duplicate"),
    ("order", "order"), ("wrong_size", "dimensions"),
    ("other_revision", "revision"), ("wrong_source", "source/product"),
    ("wrong_station", "station"), ("wrong_layout", "layout"),
    ("unknown_combination", "combination"), ("exceeded_pixel_limit", "limit"),
])
def test_synthetic_tile_plan_rejects_missing_misbound_or_unverified_tiles(mutation, message):
    raw, rule, evidence = _synthetic_display_tiles()
    if mutation == "missing":
        raw.artifacts = raw.artifacts[:-1]
    elif mutation == "duplicate_position":
        rule.input_constraints["tile_plan"]["tiles"][1]["x"] = 0
    elif mutation == "order":
        raw.artifacts = (raw.artifacts[1], raw.artifacts[0], *raw.artifacts[2:])
    elif mutation == "wrong_size":
        raw.artifacts = (*raw.artifacts[:-1], Artifact(raw.artifacts[-1].name, "tile", "image/png",
                                                       _png((0, 0, 0, 255), size=3), source_revision=raw.ref.revision))
        raw.receipts = (*raw.receipts[:-1], RawFrame(raw.ref, (raw.artifacts[-1],)).receipts[0])
    elif mutation == "other_revision":
        artifact = raw.artifacts[0]
        raw.artifacts = (Artifact(artifact.name, artifact.role, artifact.media_type,
                                  artifact.payload, source_revision="other-time"), *raw.artifacts[1:])
    elif mutation in {"wrong_source", "wrong_station"}:
        raw, _, _ = _synthetic_display_tiles(source="different" if mutation == "wrong_source" else "synthetic",
                                             station="other" if mutation == "wrong_station" else "global")
    elif mutation == "wrong_layout":
        raw.metadata["tile_layout"] = "unverified-layout"
    elif mutation == "unknown_combination":
        rule.input_constraints.pop("tile_plan")
    if mutation in {"duplicate_position", "unknown_combination"}:
        new_hash = rule_fingerprint({
            key: getattr(rule, key) for key in (
                "source", "product", "path_id", "rule_version", "encoding_version",
                "legacy_reference", "ordered_steps", "input_constraints",
            )
        })
        rule = replace(rule, config_hash=new_hash)
        evidence = replace(evidence, config_hash=new_hash)
    with raw, pytest.raises(DisplayBlockedError, match=message):
        compose_verified_tiles(raw, rule, evidence,
                               limits={"max_pixels": 15 if mutation == "exceeded_pixel_limit" else 16})


def test_synthetic_tile_plan_rejects_tampered_receipt_or_unverified_rule():
    raw, rule, evidence = _synthetic_display_tiles()
    with raw:
        assert compose_verified_tiles(raw, rule, evidence).shape == (4, 4, 4)
        with pytest.raises(DisplayBlockedError, match="evidence"):
            compose_verified_tiles(raw, rule, None)
        raw.receipts = raw.receipts[1:]
        with pytest.raises(DisplayBlockedError, match="receipt"):
            compose_verified_tiles(raw, rule, evidence)


def test_mutating_validated_tile_plan_invalidates_rule_fingerprint():
    raw, rule, evidence = _synthetic_display_tiles()
    with raw:
        assert compose_verified_tiles(raw, rule, evidence).shape == (4, 4, 4)
        rule.input_constraints["tile_plan"]["tiles"][0]["x"] = 1
        with pytest.raises(DisplayBlockedError, match="evidence"):
            compose_verified_tiles(raw, rule, evidence)
