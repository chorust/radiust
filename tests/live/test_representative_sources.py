from __future__ import annotations

import os

import numpy as np
import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.models import ProductInfo, Query, SourceInfo
from radiust.registry import registry
from radiust.sources.br_sipam import BrSipamSource
from radiust.transport import HTTPTransport

PUBLIC_CASES = (
    ("cam", ()),
    ("ca", ("CASFT",)),
    ("au", ("AU02",)),
    ("rainviewer", ()),
    ("fr", ()),
    ("kr", ()),
    ("tw-http", ("CV1_3600",)),
    ("es", ("ESCOMP",)),
    ("pt", ("PTST2",)),
    ("sg", ("SGCOMP",)),
    ("th", ("cmp1",)),
    ("th", ("kkn240Loop",)),
    ("th_royalrain", ("takhli",)),
    ("nz", ()),
    ("vn", ()),
    ("windy", ()),
)

BR_SIPAM_INFO = SourceInfo(
    id="br_sipam",
    description="Brazil SIPAM radar historical adapter",
    adapter_version="1",
    products=(ProductInfo("dbz", ("reflectivity",), {"reflectivity": "dBZ"}, default=True),),
)


def _context(source_id: str, *, source_config: dict[str, object] | None = None) -> SourceContext:
    overrides: dict[str, object] = {
        "runtime": {
            "allow_network": True,
            "request_timeout": 20.0,
            "frame_deadline": 90.0,
            "max_artifact_bytes": 64 * 1024 * 1024,
            "max_frame_bytes": 128 * 1024 * 1024,
        }
    }
    if source_config:
        overrides["sources"] = {source_id: source_config}
    return SourceContext(
        load_config(overrides, environ={}),
        source_id,
        transport=HTTPTransport(allow_network=True, max_bytes=64 * 1024 * 1024, timeout=20.0, max_attempts=2),
    )


@pytest.mark.live
@pytest.mark.asyncio
@pytest.mark.parametrize(("source_id", "stations"), PUBLIC_CASES)
async def test_public_latest_raw_smoke(source_id: str, stations: tuple[str, ...]):
    source = registry.get(source_id)
    context = _context(source_id)
    try:
        refs = await source.discover(Query(source_id, latest=True, stations=stations), context)
        assert refs, f"{source_id} returned no latest frame"
        raw = await source.download(refs[-1], context)
        try:
            assert raw.artifacts
            assert all(artifact.sha256 and artifact.size_bytes for artifact in raw.artifacts)
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.live
@pytest.mark.provider
@pytest.mark.asyncio
async def test_sidarma_latest_raw_provider_smoke():
    api_key = os.environ.get("RADIUST_TEST_ID_SIDARMA_API_KEY")
    if not api_key:
        pytest.skip("RADIUST_TEST_ID_SIDARMA_API_KEY is not configured")
    source = registry.get("id_sidarma")
    context = _context("id_sidarma", source_config={"api_key": api_key, "radar_ids": ["CGK"]})
    try:
        refs = await source.discover(Query("id_sidarma", latest=True, stations=("CGK",)), context)
        assert refs
        raw = await source.download(refs[-1], context)
        try:
            assert raw.artifacts[0].sha256
            assert raw.artifacts[0].size_bytes
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.live
@pytest.mark.provider
@pytest.mark.asyncio
async def test_pagasa_latest_raw_provider_smoke():
    """Exercise the real HTTP/browser acquisition only with an authorized token."""
    timeline_token = os.environ.get("RADIUST_TEST_PH_TIMELINE_TOKEN")
    if not timeline_token:
        pytest.skip("RADIUST_TEST_PH_TIMELINE_TOKEN is not configured")
    source = registry.get("ph")
    context = _context("ph", source_config={"timeline_token": timeline_token})
    try:
        refs = await source.discover(Query("ph", latest=True), context)
        assert len(refs) == 1
        raw = await source.download(refs[0], context)
        try:
            assert raw.artifacts and all(artifact.sha256 and artifact.size_bytes for artifact in raw.artifacts)
            assert raw.ref.valid_time == refs[0].valid_time
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.live
@pytest.mark.provider
@pytest.mark.asyncio
async def test_wunderground_latest_raw_provider_smoke():
    api_key = os.environ.get("RADIUST_TEST_WUNDERGROUND_API_KEY")
    if not api_key:
        pytest.skip("RADIUST_TEST_WUNDERGROUND_API_KEY is not configured")
    source = registry.get("wunderground")
    context = _context("wunderground", source_config={"api_key": api_key})
    try:
        refs = await source.discover(Query("wunderground", latest=True), context)
        assert len(refs) == 1
        raw = await source.download(refs[0], context)
        try:
            assert len(raw.artifacts) == 4
            assert all(artifact.sha256 and artifact.size_bytes for artifact in raw.artifacts)
            assert api_key not in repr(refs[0])
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.live
@pytest.mark.asyncio
async def test_tw_native_numeric_grid_latest_scientific_smoke():
    """Opt-in check of the authoritative numeric field, distinct from the CWA PNG."""
    source = registry.get("tw")
    context = _context("tw")
    try:
        refs = await source.discover(Query("tw", product="grid", latest=True), context)
        assert len(refs) == 1
        raw = await source.download(refs[0], context)
        try:
            assert raw.artifacts[0].sha256 and raw.artifacts[0].size_bytes
            field = source.decode(raw, context)
            assert field.grid.crs == "EPSG:3821"
            assert field.grid.shape == (881, 921)
            assert field.grid.extent == (115.0, 18.0, 126.5, 29.0)
            assert field.data.attrs["units"] == "dBZ"
            assert np.array_equal(np.isnan(field.data.values), field.quality.values != 0)
            assert set(np.unique(field.quality.values)) <= {0, 1, 2}
        finally:
            raw.close()
    finally:
        context.close()


@pytest.mark.live
@pytest.mark.asyncio
async def test_br_sipam_historical_latest_raw_smoke():
    """Exercise the historical-only SIPAM adapter directly without adding it to the public catalog."""
    source = BrSipamSource(BR_SIPAM_INFO)
    context = _context("br_sipam")
    try:
        refs = await source.discover(Query("br_sipam", latest=True, stations=("BRBE",)), context)
        assert refs
        raw = await source.download(refs[-1], context)
        try:
            assert raw.ref.station == "BRBE"
            assert raw.ref.valid_time.tzinfo is not None
            assert raw.artifacts[0].media_type == "image/png"
            assert raw.artifacts[0].sha256 and raw.artifacts[0].size_bytes
        finally:
            raw.close()
    finally:
        context.close()
