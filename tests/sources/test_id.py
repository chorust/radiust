from __future__ import annotations

import hashlib
import importlib
import json
from datetime import datetime, timezone
from io import BytesIO
from pathlib import Path
from urllib.parse import parse_qs, urlencode, urlparse

import pytest
from PIL import Image
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError, TransportError
from radiust.identity import artifact_bytes, safe_ref
from radiust.logging import redact
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.id import IdSource

from tests.support.http_replay import ReplayTransport


def test_id_legacy_raw_is_replayable_without_promoting_unverified_provenance():
    fixture_root = Path(__file__).parents[1] / "fixtures/sources/id"
    fixture = json.loads((fixture_root / "fixture.json").read_text(encoding="utf-8"))

    assert fixture["status"] == "blocked"
    assert fixture["frames"] == []
    evidence = fixture["legacy_raw_evidence"]
    assert evidence["station"] == "MAJ"
    assert evidence["legacy_filename_time"] == "2025-12-30T07:59:00Z"
    assert evidence["source_archive_path"] == (
        "output/id/ID-ICUSRC-MAJ_20251230075900_merca_r_source.png"
    )
    assert evidence["payload_url_retained"] is False
    assert evidence["discovery_response_retained"] is False
    assert evidence["tls_verification"] == "disabled_by_legacy_client"
    assert evidence["time_binding"] == "not_independently_verified"

    artifact = evidence["artifact"]
    assert artifact["path"] == "raw/ID-ICUSRC-MAJ_20251230075900_merca_r_source.png"
    assert artifact["sha256"] == (
        "2c82b22aee6fb8891169cbc32f4d821b27c9dbc311fecfe71a5446ff0f558df3"
    )
    assert artifact["size_bytes"] == 84756
    payload = (fixture_root / artifact["path"]).read_bytes()
    assert hashlib.sha256(payload).hexdigest() == (
        "2c82b22aee6fb8891169cbc32f4d821b27c9dbc311fecfe71a5446ff0f558df3"
    )
    assert len(payload) == 84756
    with Image.open(BytesIO(payload)) as image:
        image.load()
        assert image.format == "PNG"
        assert image.size == (4084, 4084)
        assert image.mode == "RGBA"

    migration = json.loads(
        (Path(__file__).parents[2] / "migration/sources/id.json").read_text(
            encoding="utf-8"
        )
    )
    assert migration["status"] == "credential-blocked"
    assert migration["legacy_raw_recovery"]["artifact_sha256"] == artifact["sha256"]
    assert "not independently verified" in migration["legacy_raw_recovery"]["time_binding"]


@pytest.mark.asyncio
async def test_id_uses_external_token_and_parses_last_hour(tmp_path):
    source = IdSource(registry.get_info("id"))
    radar_id = source._radars[0]
    url = f"{source.API_URL}?{urlencode({'token': 'test-token', 'radar': radar_id})}"
    image_url = "https://example.test/radar.png"
    image_buffer = BytesIO()
    Image.new("RGBA", (2, 2), (30, 80, 120, 255)).save(image_buffer, format="PNG")
    image_payload = image_buffer.getvalue()
    transport = ReplayTransport(
        {
            url: json.dumps(
                {"LastOneHour": {"file": [image_url], "timeUTC": ["2026-09-18 04:00:00 UTC"]}}
            ).encode(),
            image_url: image_payload,
        }
    )
    context = SourceContext(
        load_config(
            {"runtime": {"allow_network": True}, "sources": {"id": {"token": "test-token", "radar_ids": [radar_id]}}},
            environ={},
        ),
        "id",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        ref = (await source.discover(Query("id", latest=True), context))[0]
        assert ref.station == radar_id
        assert ref.valid_time == datetime(2026, 9, 18, 4, 0, tzinfo=timezone.utc)
        assert ref.uri == "https://example.test/radar.png"
        assert "test-token" not in ref.logical_id
        assert "test-token" not in repr(ref)
        assert "test-token" not in json.dumps(safe_ref(ref))
        assert "test-token" not in redact(transport.get_calls[0][0])

        raw = await source.download(ref, context)
        try:
            assert raw.ref == ref
            assert raw.artifacts[0].sha256 == hashlib.sha256(image_payload).hexdigest()
            assert artifact_bytes(raw.artifacts[0]) == image_payload
            with pytest.raises(DecodeError, match="verified scientific decoder"):
                source.decode(raw, context)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.asyncio
async def test_id_transport_error_does_not_echo_query_token(tmp_path):
    source = IdSource(registry.get_info("id"))
    token = "fixture-id-discovery-secret"

    class EchoErrorTransport:
        async def get(self, url, *, headers=None):
            raise RuntimeError(f"request failed for {url}")

    context = SourceContext(
        load_config(
            {
                "runtime": {"allow_network": True},
                "sources": {"id": {"token": token, "radar_ids": [source._radars[0]]}},
            },
            environ={},
        ),
        "id",
        transport=EchoErrorTransport(),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(Exception) as raised:
            await source.discover(Query("id", latest=True), context)
        assert token not in str(raised.value)
        assert isinstance(raised.value, TransportError)
    finally:
        context.close()


@pytest.mark.asyncio
@pytest.mark.parametrize("payload", [b"\xff", b"{"], ids=["non-utf8", "invalid-json"])
async def test_id_rejects_invalid_discovery_json(tmp_path, payload):
    source = IdSource(registry.get_info("id"))
    radar_id = source._radars[0]
    url = f"{source.API_URL}?{urlencode({'token': 'test-token', 'radar': radar_id})}"
    context = SourceContext(
        load_config(
            {"runtime": {"allow_network": True}, "sources": {"id": {"token": "test-token", "radar_ids": [radar_id]}}},
            environ={},
        ),
        "id",
        transport=ReplayTransport({url: payload}),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(DecodeError, match=f"BMKG returned invalid discovery JSON for {radar_id}"):
            await source.discover(Query("id", latest=True), context)
    finally:
        context.close()


@pytest.mark.asyncio
async def test_id_uses_its_own_radar_resource_instead_of_sidarma_resource(tmp_path, monkeypatch):
    resource_root = tmp_path / "radiust.resources"
    source_resources = resource_root / "sources"
    source_resources.mkdir(parents=True)
    (source_resources / "id.json").write_text(
        json.dumps({"radar_ids": ["ID_ONLY"]}), encoding="utf-8"
    )
    (source_resources / "id_sidarma.json").write_text(
        json.dumps({"radars": [{"id": "SIDARMA_ONLY"}]}), encoding="utf-8"
    )
    id_module = importlib.import_module("radiust.sources.id")
    monkeypatch.setattr(id_module.resources, "files", lambda _package: resource_root)

    class RadarListTransport:
        def __init__(self):
            self.radar_ids = []

        async def get(self, url, *, headers=None):
            radar_id = parse_qs(urlparse(url).query)["radar"][0]
            self.radar_ids.append(radar_id)
            return json.dumps(
                {
                    "LastOneHour": {
                        "file": [f"https://example.test/{radar_id}.png"],
                        "timeUTC": ["2026-09-18 04:00:00 UTC"],
                    }
                }
            ).encode()

    transport = RadarListTransport()
    context = SourceContext(
        load_config(
            {"runtime": {"allow_network": True}, "sources": {"id": {"token": "fixture-token"}}},
            environ={},
        ),
        "id",
        transport=transport,
        temp_root=tmp_path,
    )
    try:
        refs = await IdSource(registry.get_info("id")).discover(Query("id", latest=True), context)

        assert transport.radar_ids == ["ID_ONLY"]
        assert [ref.station for ref in refs] == ["ID_ONLY"]
    finally:
        context.close()
