from __future__ import annotations

import asyncio
import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import unquote, urlsplit

import pytest
from radiust import Client, _bridge
from radiust.config import load_config
from radiust.models import FrameRef

from tests.support.providers import provider_matrix

core = pytest.importorskip("radiust._core")

_REMOTE_TARGETS = tuple(target for target in provider_matrix() if target.provider in {"s3", "oss"})
_REF = FrameRef(
    "provider-contract-test",
    "reflectivity",
    datetime(2026, 1, 1, tzinfo=timezone.utc),
    station="test-site",
    revision="provider-contract-v1",
)


def _field() -> core.RadarField:
    return core.RadarField(
        json.dumps(
            {
                "name": "reflectivity",
                "values": [10.0, 20.0, 30.0, 40.0],
                "shape": [2, 2],
                "quality": [0, 0, 0, 0],
                "units": "dBZ",
                "valid_time": "2026-01-01T00:00:00Z",
                "grid": {
                    "shape": [2, 2],
                    "crs": "EPSG:4326",
                    "x": [100.0, 101.0],
                    "y": [20.0, 21.0],
                    "affine": None,
                },
                "provenance": ["provider-contract-test"],
            }
        )
    )


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
    output_path = Path(first.items[0].output_uri)
    manifest_path = output_path.with_name(output_path.name + ".manifest.json")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    for artifact in manifest["artifacts"]:
        payload = (output / artifact["relative_uri"]).read_bytes()
        assert len(payload) == artifact["size_bytes"]
        assert hashlib.sha256(payload).hexdigest() == artifact["sha256"]


@pytest.mark.provider
@pytest.mark.parametrize("target", _REMOTE_TARGETS, ids=lambda target: target.name)
def test_remote_provider_manifest_commit_readback_and_idempotence(target, record_property) -> None:
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
    location = urlsplit(root_uri)
    bucket = location.netloc
    prefix = unquote(location.path).strip("/")
    config = load_config(
        {
            "runtime": {"allow_network": True},
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
    pointer_uri = first.items[0].output_uri
    assert pointer_uri is not None
    pointer = urlsplit(pointer_uri)
    assert pointer.scheme == target.provider and pointer.netloc == bucket
    pointer_key = unquote(pointer.path).lstrip("/")

    async def verify_objects() -> None:
        pointer_readback = await _bridge.object_read(
            target.provider,
            bucket,
            "",
            pointer_key,
            endpoint=endpoint,
            region=region,
            access_key_id=access_key,
            secret_access_key=secret_key,
            anonymous=anonymous,
        )
        assert pointer_readback is not None
        manifest_bytes, manifest_size, manifest_sha256 = pointer_readback
        assert manifest_size == len(manifest_bytes)
        assert manifest_sha256 == hashlib.sha256(manifest_bytes).hexdigest()
        manifest = json.loads(manifest_bytes)
        assert manifest["output_id"]
        assert manifest["generation"]
        assert manifest["artifacts"]
        for artifact in manifest["artifacts"]:
            readback = await _bridge.object_read(
                target.provider,
                bucket,
                prefix,
                artifact["relative_uri"],
                endpoint=endpoint,
                region=region,
                access_key_id=access_key,
                secret_access_key=secret_key,
                anonymous=anonymous,
            )
            assert readback is not None
            payload, size_bytes, sha256 = readback
            assert size_bytes == artifact["size_bytes"]
            assert size_bytes == len(payload)
            assert sha256 == artifact["sha256"] == hashlib.sha256(payload).hexdigest()

    asyncio.run(verify_objects())
    record_property("provider", target.name)
    record_property("status", "passed: Rust remote manifest commit, idempotence and artifact readback")
    record_property("credential_state", target.credential_state(environment))
