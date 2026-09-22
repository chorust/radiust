from __future__ import annotations

from datetime import datetime, timezone

import pytest
from radiust import _core
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import DecodeError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.au import AuSource


@pytest.mark.asyncio
async def test_au_discovers_bom_filename_time_and_preserves_live_raw(
    tmp_path, fixture_root, monkeypatch
):
    raw_path = fixture_root / "au/raw/IDR021.T.202609180511.png"
    payload = raw_path.read_bytes()

    async def ftp_nlst(*args):
        assert args[0] == AuSource.FTP_ROOT
        return [
            "IDR021.T.202609180511.png",
            "IDR021.gif",
            "IDR024.T.202110092309.png.tmp",
        ]

    async def ftp_read(address, *args):
        assert address.endswith("IDR021.T.202609180511.png")
        return payload

    monkeypatch.setattr(_core, "ftp_nlst", ftp_nlst)
    monkeypatch.setattr(_core, "ftp_read", ftp_read)

    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "au",
        temp_root=tmp_path,
    )
    source = AuSource(registry.get_info("au"))
    refs = await source.discover(Query("au", latest=True), context)
    assert len(refs) == 1
    ref = refs[0]
    assert ref.station == "AU02"
    assert ref.valid_time == datetime(2026, 9, 18, 5, 11, tzinfo=timezone.utc)
    assert ref.uri.endswith("IDR021.T.202609180511.png")
    assert ref.metadata["ftp_mode"] == "passive_plain"

    raw = await source.download(ref, context)
    try:
        artifact = raw.artifacts[0]
        assert artifact.size_bytes == 1921
        assert artifact.sha256 == "4ab60f3c01df485d6d5bf24417733425a4900e160c43b4543b51bc124d7a0c8c"
        with pytest.raises(DecodeError, match="verified scientific decoder"):
            source.decode(raw, context)
    finally:
        raw.close()
        context.close()


def test_au_registry_uses_dedicated_adapter():
    assert isinstance(registry.get("au"), AuSource)
