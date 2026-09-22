from __future__ import annotations

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.errors import UnsupportedQueryError
from radiust.models import Query
from radiust.registry import registry
from radiust.sources.uk import UkSource

from tests.support.http_replay import ReplayTransport


@pytest.mark.asyncio
async def test_uk_retired_datapoint_fails_before_using_external_key(tmp_path):
    context = SourceContext(
        load_config({"runtime": {"allow_network": True}}, environ={}),
        "uk",
        transport=ReplayTransport(),
        temp_root=tmp_path,
    )
    try:
        with pytest.raises(UnsupportedQueryError, match="retired.*2025-12-01"):
            await UkSource(registry.get_info("uk")).discover(Query("uk", latest=True), context)
        assert context.transport.get_calls == []
    finally:
        context.close()
