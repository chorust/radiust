from __future__ import annotations

import asyncio
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pytest
import xarray as xr
from radiust import Client
from radiust.config import load_config
from radiust.field import RadarField
from radiust.grids import GeographicGrid
from radiust.models import FrameRef
from radiust.storage.manifest import inspect_manifest
from radiust.storage.object import RustObjectBackend, parse_object_uri

from tests.support.providers import provider_matrix

_REMOTE_TARGETS = tuple(target for target in provider_matrix() if target.provider in {"s3", "oss"})
_REF = FrameRef(
    "provider-contract-test",
    "reflectivity",
    datetime(2026, 1, 1, tzinfo=timezone.utc),
    station="test-site",
    revision="provider-contract-v1",
)


def _field() -> RadarField:
    values = np.array([[10.0, 20.0], [30.0, 40.0]], dtype="float32")
    data = xr.DataArray(
        values,
        dims=("latitude", "longitude"),
        coords={"latitude": [20.0, 21.0], "longitude": [100.0, 101.0]},
        name="reflectivity",
        attrs={"units": "dBZ"},
    )
    return RadarField(data, GeographicGrid([100.0, 101.0], [20.0, 21.0]))


def test_provider_matrix_is_explicit_and_redacted() -> None:
    environment = {
        "RADIUST_PROVIDER_AWS_S3_URI": "s3://bucket/radiust-test/prefix",
        "AWS_ACCESS_KEY_ID": "access-value",
        "AWS_SECRET_ACCESS_KEY": "secret-value",
        "RADIUST_PROVIDER_S3_URI": "s3://compatible/radiust-test/prefix",
    }
    reports = {target.name: target.report(environment) for target in provider_matrix()}

    assert set(reports) == {"local", "aws-s3", "s3-compatible", "aliyun-oss"}
    assert reports["local"]["status"] == "configured_unverified"
    assert reports["aws-s3"]["status"] == "configured_unverified"
    assert reports["s3-compatible"]["credential_state"] == "anonymous"
    assert "access-value" not in str(reports)
    assert "secret-value" not in str(reports)


def test_provider_matrix_marks_partial_credentials_incomplete() -> None:
    target = next(target for target in provider_matrix() if target.name == "aliyun-oss")
    environment = {
        "RADIUST_PROVIDER_OSS_URI": "oss://bucket/radiust-test/prefix",
        "RADIUST_PROVIDER_OSS_ACCESS_KEY": "access-value",
    }

    assert target.configured(environment) is False
    assert target.credential_state(environment) == "incomplete"
    assert target.report(environment)["status"] == "unverified"


def test_provider_matrix_refuses_non_test_prefix_and_redacts_uri() -> None:
    target = next(target for target in provider_matrix() if target.name == "s3-compatible")
    environment = {"RADIUST_PROVIDER_S3_URI": "s3://compatible/production"}

    with pytest.raises(ValueError, match="dedicated test prefix"):
        target.isolated_uri(environment, run_id="safe-run")

    environment["RADIUST_PROVIDER_S3_URI"] = "s3://compatible/radiust-test/prefix"
    isolated = target.isolated_uri(environment, run_id="safe-run")
    assert isolated == "s3://compatible/radiust-test/prefix/radiust-validation-safe-run"
    assert "s3://compatible/" not in str(target.report(environment))


def test_local_provider_roundtrip_uses_the_same_manifest_contract(tmp_path: Path) -> None:
    output = tmp_path / "local-provider-test"
    config = load_config(
        {"cache": {"enabled": False}, "storage": {"output": str(output)}},
        environ={},
    )
    with Client(config=config) as client:
        first = client.write(_field(), output=output, ref=_REF)
        second = client.write(_field(), output=output, ref=_REF)

    assert first.counts["written"] == 1
    assert second.counts["skipped"] == 1
    manifest_path = Path(first.items[0].output_uri).with_name(Path(first.items[0].output_uri).name + ".manifest.json")
    assert inspect_manifest(manifest_path) == "complete"


@pytest.mark.provider
@pytest.mark.parametrize("target", _REMOTE_TARGETS, ids=lambda target: target.name)
def test_remote_provider_write_readback_and_idempotence(target, record_property) -> None:
    environment = os.environ
    if not target.configured(environment):
        pytest.skip(f"{target.name} status=unverified: configure an isolated URI and a complete credential pair")
    try:
        root_uri = target.isolated_uri(environment)
    except ValueError as exc:
        pytest.skip(str(exc))

    endpoint = target.endpoint(environment)
    region = target.region(environment)
    access_key = environment.get(target.access_key_env or "") or None
    secret_key = environment.get(target.secret_key_env or "") or None
    anonymous = target.name != "aws-s3" and target.credential_state(environment) == "anonymous"
    config = load_config(
        {
            "runtime": {"allow_network": False},
            "cache": {"enabled": False},
            "storage": {
                "endpoint": endpoint,
                "region": region,
                "access_key": access_key,
                "secret_key": secret_key,
                "anonymous": anonymous,
            },
        },
        environ={},
    )

    with Client(config=config) as client:
        first = client.write(_field(), output=root_uri, ref=_REF)
        second = client.write(_field(), output=root_uri, ref=_REF)

    assert first.counts["written"] == 1
    assert second.counts["skipped"] == 1
    location = parse_object_uri(first.items[0].output_uri, provider=target.provider, endpoint=endpoint, region=region)
    backend = RustObjectBackend(
        root_uri,
        endpoint=endpoint,
        region=region,
        access_key_id=access_key,
        secret_access_key=secret_key,
        anonymous=anonymous,
    )

    async def verify_objects() -> None:
        pointer_bytes = await backend.get(f"{location.key}.manifest.json")
        assert pointer_bytes is not None
        manifest = json.loads(pointer_bytes)
        assert manifest["output_id"]
        assert manifest["generation"]
        for artifact in manifest["artifacts"]:
            payload = await backend.get(artifact["relative_uri"])
            assert payload is not None
            assert len(payload) == artifact["size_bytes"]
            assert hashlib.sha256(payload).hexdigest() == artifact["sha256"]

    asyncio.run(verify_objects())
    record_property("provider", target.name)
    record_property("status", "passed")
    record_property("credential_state", target.credential_state(environment))
