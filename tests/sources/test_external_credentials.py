from __future__ import annotations

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import ConfigError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.id import IdSource
from radiust.sources.id_sidarma import IdSidarmaSource
from radiust.sources.ph import PhSource
from radiust.sources.wunderground import WundergroundSource

from tests.support.http_replay import ReplayTransport


def test_external_provider_environment_names_are_nested_and_redacted():
    config = load_config(
        environ={
            "RADIUST_SOURCES__ID_SIDARMA__API_KEY": "fixture-sidarma-key",
            "RADIUST_SOURCES__PH__TIMELINE_TOKEN": "fixture-pagasa-token",
            "RADIUST_SOURCES__WUNDERGROUND__API_KEY": "fixture-wu-key",
        }
    )

    assert config.values["sources"]["id_sidarma"]["api_key"] == "fixture-sidarma-key"
    assert config.values["sources"]["ph"]["timeline_token"] == "fixture-pagasa-token"
    assert config.values["sources"]["wunderground"]["api_key"] == "fixture-wu-key"
    redacted = config.redacted()["sources"]
    assert redacted["id_sidarma"]["api_key"] == "<configured>"
    assert redacted["ph"]["timeline_token"] == "<configured>"
    assert redacted["wunderground"]["api_key"] == "<configured>"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("source_id", "factory", "message"),
    [
        ("ph", PhSource, "sources.ph.timeline_token"),
        ("id", IdSource, "sources.id.token"),
        ("id_sidarma", IdSidarmaSource, "sources.id_sidarma.api_key"),
        ("wunderground", WundergroundSource, "sources.wunderground.api_key"),
    ],
)
async def test_external_credential_sources_fail_closed_without_legacy_secrets(
    source_id, factory, message, tmp_path
):
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        source_id,
        transport=ReplayTransport(),
        temp_root=tmp_path,
    )
    source = factory(registry.get_info(source_id))
    try:
        with pytest.raises(ConfigError, match=message.replace(".", r"\.")):
            await source.discover(Query(source_id, latest=True), context)
        assert context.transport.get_calls == []
        assert context.transport.post_calls == []
    finally:
        context.close()
