from __future__ import annotations

import asyncio
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import pytest
from radiust import (
    AsyncClient,
    Client,
    DecodeError,
    FrameRef,
    IntegrityError,
    OperationCancelled,
    to_xarray,
)
from radiust.identity import cache_key, safe_ref

from tests.support.native_raw_cache import seed_native_cache_entry, seed_native_raw_cache
from tests.support.native_science_fixture import seed_tw_grid_cache, tw_grid_ref

REPO_ROOT = Path(__file__).parents[2]
SOURCE_FIXTURE = REPO_ROOT / "tests/fixtures/sources/fr/fixture.json"


def _cached_fr_frame(cache_root: Path) -> tuple[FrameRef, dict[str, str]]:
    fixture = json.loads(SOURCE_FIXTURE.read_text(encoding="utf-8"))
    metadata = fixture["frames"][0]
    artifact = metadata["artifacts"][0]
    valid_time = datetime.fromisoformat(metadata["valid_time"].replace("Z", "+00:00"))
    ref = FrameRef(
        source=fixture["source"],
        product=metadata["product"],
        valid_time=valid_time,
        station=metadata["station"],
        uri=metadata["uri"],
        locator={
            **metadata["locator"],
            "url": metadata["uri"],
            "station": metadata["station"],
            "name": artifact["name"],
            "media_type": artifact["media_type"],
            "artifacts": [],
        },
        locator_version=metadata["locator_version"],
        revision=metadata["revision"],
    )
    raw_path = SOURCE_FIXTURE.parent / artifact["path"]
    seed_native_raw_cache(
        cache_root,
        ref,
        [(artifact["name"], artifact["media_type"], raw_path.read_bytes())],
    )
    return ref, artifact


def _offline_config(cache_root: Path, tmp_path: Path) -> dict[str, object]:
    return {
        "runtime": {"allow_network": False, "temp_root": str(tmp_path / "temp")},
        "cache": {"enabled": True, "dir": str(cache_root)},
    }


def _seed_fixture_frame(cache_root: Path, source: str, product: str) -> FrameRef:
    fixture_path = REPO_ROOT / f"tests/fixtures/sources/{source}/fixture.json"
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    metadata = next(frame for frame in fixture["frames"] if frame["product"] == product)
    artifacts = metadata["artifacts"]
    station = metadata.get("station")
    locator = dict(metadata.get("locator") or {})
    locator.update({"url": metadata["uri"], "station": station, "artifacts": []})
    if artifacts:
        locator.update({"name": artifacts[0]["name"], "media_type": artifacts[0]["media_type"]})
    ref = FrameRef(
        source=source,
        product=product,
        valid_time=datetime.fromisoformat(metadata["valid_time"].replace("Z", "+00:00")),
        station=station,
        uri=metadata["uri"],
        locator=locator,
        locator_version=metadata["locator_version"],
        revision=metadata["revision"],
    )
    payloads = [
        (
            artifact["name"],
            artifact["media_type"],
            (fixture_path.parent / artifact["path"]).read_bytes(),
        )
        for artifact in artifacts
    ]
    seed_native_raw_cache(cache_root, ref, payloads)
    return ref


def _write_fixture_raw_manifest(root: Path, ref: FrameRef, source: str, product: str) -> Path:
    fixture_path = REPO_ROOT / f"tests/fixtures/sources/{source}/fixture.json"
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    metadata = next(frame for frame in fixture["frames"] if frame["product"] == product)
    raw_root = root / "raw"
    raw_root.mkdir(parents=True)
    descriptors = []
    for artifact in metadata["artifacts"]:
        payload = (fixture_path.parent / artifact["path"]).read_bytes()
        (raw_root / artifact["name"]).write_bytes(payload)
        descriptors.append(
            {
                "name": artifact["name"],
                "media_type": artifact["media_type"],
                "size_bytes": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        )
    manifest_path = root / "raw-manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "raw_complete": True,
                "ref": safe_ref(ref),
                "artifacts": descriptors,
            },
            sort_keys=True,
        ),
        encoding="utf-8",
    )
    return manifest_path


def _write_rdcap_raw_manifest(root: Path, ref: FrameRef) -> Path:
    response = (
        REPO_ROOT
        / "tests/fixtures/sources/rdcap/TWN/RCHL/file-response.reconstructed.json"
    ).read_bytes()
    safe = safe_ref(ref)
    binding = {
        "schema_version": 1,
        "source": "rdcap",
        "product": "reflectivity",
        "station": "TWRCHL",
        "country": "TWN",
        "station_code": "RCHL",
        "key": str(ref.locator["key"]),
        "valid_time": safe["valid_time"],
        "logical_id": safe["logical_id"],
        "content_sha256": hashlib.sha256(response).hexdigest(),
        "content_size_bytes": len(response),
    }
    binding_bytes = json.dumps(binding, ensure_ascii=False, indent=2).encode("utf-8")
    raw_root = root / "raw"
    raw_root.mkdir(parents=True)
    (raw_root / "file-response.json").write_bytes(response)
    (raw_root / "binding.json").write_bytes(binding_bytes)
    artifacts = []
    for name, payload in (
        ("file-response.json", response),
        ("binding.json", binding_bytes),
    ):
        artifacts.append(
            {
                "name": name,
                "media_type": "application/json",
                "size_bytes": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        )
    manifest_path = root / "raw-manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "raw_complete": True,
                "ref": safe,
                "artifacts": artifacts,
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    return manifest_path


def _seed_rdcap_frame(cache_root: Path) -> FrameRef:
    fixture_root = REPO_ROOT / "tests/fixtures/sources/rdcap"
    fixture = json.loads((fixture_root / "manifest.json").read_text(encoding="utf-8"))
    station = next(item for item in fixture["stations"] if item["station_id"] == "TWRCHL")
    key = str(station["selected_key_epoch_ms"])
    response = (
        fixture_root / "TWN/RCHL/file-response.reconstructed.json"
    ).read_bytes()
    content_digest = hashlib.sha256(response).hexdigest()
    valid_time = datetime.fromtimestamp(int(key) / 1000, tz=timezone.utc)
    ref = FrameRef(
        "rdcap",
        "reflectivity",
        valid_time,
        station="TWRCHL",
        locator={"country": "TWN", "station_code": "RCHL", "key": key},
        locator_version="rdcap-csr-v1",
        revision=content_digest,
    )
    normalized_time = valid_time.isoformat(timespec="microseconds").replace("+00:00", "Z")
    binding = {
        "schema_version": 1,
        "source": "rdcap",
        "product": "reflectivity",
        "station": "TWRCHL",
        "country": "TWN",
        "station_code": "RCHL",
        "key": key,
        "valid_time": normalized_time,
        "logical_id": ref.logical_id,
        "content_sha256": content_digest,
        "content_size_bytes": len(response),
    }
    binding_bytes = json.dumps(binding, sort_keys=True, indent=2).encode("utf-8")
    artifacts = [
        {
            "name": name,
            "media_type": "application/json",
            "size_bytes": len(payload),
            "sha256": hashlib.sha256(payload).hexdigest(),
        }
        for name, payload in [
            ("file-response.json", response),
            ("binding.json", binding_bytes),
        ]
    ]
    frame_key = cache_key(ref, "rdcap-raw-v1")
    for name, payload in [
        ("file-response.json", response),
        ("binding.json", binding_bytes),
    ]:
        seed_native_cache_entry(cache_root, f"raw:{frame_key}:artifact:{name}", payload)
    manifest = {
        "schema_version": 1,
        "logical_id": ref.logical_id,
        "revision": content_digest,
        "ref": {},
        "artifacts": artifacts,
    }
    seed_native_cache_entry(
        cache_root,
        f"raw:{frame_key}:manifest",
        json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode("utf-8"),
    )
    return ref


def _assert_dbz_raster(result: object, expected_input_kind: str = "source") -> None:
    info = json.loads(result.mode_info_json)
    source_input = json.loads(result.input_json)
    assert info["actual"] == "dbz"
    assert info["units"] == "dBZ"
    assert source_input["kind"] == expected_input_kind
    field = to_xarray(result)
    assert field.attrs["units"] == "dBZ"
    assert field.attrs["time_status"] == "source"
    assert datetime.fromisoformat(
        field.attrs["valid_time"].replace("Z", "+00:00")
    ).isoformat() == "2026-09-18T02:45:00+00:00"
    assert field.attrs["geolocation"] == "unknown"
    assert field.shape == (600, 700)


def test_sync_fetch_batch_stream_and_cached_raw_replay_use_source_dbz_mode(
    tmp_path: Path,
) -> None:
    cache_root = tmp_path / "cache"
    ref, _artifact = _cached_fr_frame(cache_root)
    config = _offline_config(cache_root, tmp_path)

    with Client(config=config) as client:
        single = client.fetch(ref, mode="dbz")
        _assert_dbz_raster(single)

        batch = client.fetch_many([ref], mode="dbz")
        assert batch.items[0].status == "success"
        assert batch.items[0].error is None
        _assert_dbz_raster(batch.items[0].data)

        streamed = list(client.iter_fetch([ref], mode="dbz"))
        assert len(streamed) == 1 and streamed[0].status == "success"
        assert streamed[0].error is None
        _assert_dbz_raster(streamed[0].data)

        gray_batch = client.fetch_many([ref], mode="gray")
        assert gray_batch.items[0].status == "success"
        assert gray_batch.items[0].error is None
        assert gray_batch.items[0].data is not None

        gray_streamed = list(client.iter_fetch([ref], mode="gray"))
        assert len(gray_streamed) == 1 and gray_streamed[0].status == "success"
        assert gray_streamed[0].error is None
        assert gray_streamed[0].data is not None

        raw = client.download(
            [ref], output=tmp_path / "raw", raw_only=True, on_error="raise"
        )
        assert raw.counts["written"] == 1
        manifest_path = Path(raw.items[0].output_uri)
        assert manifest_path.is_file()

        replayed = client.replay_raw_manifest(manifest_path, mode="dbz")
        _assert_dbz_raster(replayed)


def test_async_fetch_batch_stream_and_replay_keep_dbz_mode_offline(tmp_path: Path) -> None:
    cache_root = tmp_path / "cache"
    ref, _artifact = _cached_fr_frame(cache_root)
    config = _offline_config(cache_root, tmp_path)

    async def exercise() -> None:
        async with AsyncClient(config=config) as client:
            single = await client.fetch(ref, mode="dbz")
            _assert_dbz_raster(single)

            batch = await client.fetch_many([ref], mode="dbz")
            assert batch.items[0].status == "success"
            assert batch.items[0].error is None
            _assert_dbz_raster(batch.items[0].data)

            streamed = [item async for item in client.aiter_fetch([ref], mode="dbz")]
            assert len(streamed) == 1 and streamed[0].status == "success"
            assert streamed[0].error is None
            _assert_dbz_raster(streamed[0].data)

            gray_batch = await client.fetch_many([ref], mode="gray")
            assert gray_batch.items[0].status == "success"
            assert gray_batch.items[0].error is None
            assert gray_batch.items[0].data is not None

            gray_streamed = [item async for item in client.aiter_fetch([ref], mode="gray")]
            assert len(gray_streamed) == 1 and gray_streamed[0].status == "success"
            assert gray_streamed[0].error is None
            assert gray_streamed[0].data is not None

            raw = await client.download(
                [ref], output=tmp_path / "raw", raw_only=True, on_error="raise"
            )
            assert raw.counts["written"] == 1
            manifest_path = Path(raw.items[0].output_uri)
            replayed = await client.replay_raw_manifest(manifest_path, mode="dbz")
            _assert_dbz_raster(replayed)

    asyncio.run(exercise())


def test_sync_and_async_dbz_download_keep_partial_results_in_input_order(tmp_path: Path) -> None:
    cache_root = tmp_path / "cache"
    good = _cached_fr_frame(cache_root)[0]
    blocked = _seed_fixture_frame(cache_root, "tw", "observation")
    config = _offline_config(cache_root, tmp_path)

    with Client(config=config) as client:
        sync = client.download(
            [good, blocked],
            output=tmp_path / "sync-output",
            mode="dbz",
            format="netcdf",
            on_error="collect",
        )

    async def exercise() -> object:
        async with AsyncClient(config=config) as client:
            return await client.download(
                [good, blocked],
                output=tmp_path / "async-output",
                mode="dbz",
                format="netcdf",
                on_error="collect",
            )

    asynchronous = asyncio.run(exercise())
    assert sync.counts == asynchronous.counts
    assert sync.counts["written"] == 1
    assert sync.counts["failed"] == 1
    assert [item.status for item in sync.items] == ["written", "failed"]
    assert [item.status for item in asynchronous.items] == ["written", "failed"]
    assert sync.items[0].output_uri is not None
    assert asynchronous.items[0].output_uri is not None
    assert Path(f"{sync.items[0].output_uri}.manifest.json").is_file()
    assert Path(f"{asynchronous.items[0].output_uri}.manifest.json").is_file()


def test_source_replay_rejects_raw_sha_corruption_without_network_fallback(
    tmp_path: Path,
) -> None:
    cache_root = tmp_path / "cache"
    ref, artifact = _cached_fr_frame(cache_root)
    config = _offline_config(cache_root, tmp_path)

    with Client(config=config) as client:
        raw = client.download(
            [ref], output=tmp_path / "raw", raw_only=True, on_error="raise"
        )
        manifest_path = Path(raw.items[0].output_uri)
        payload_path = manifest_path.parent / "raw" / artifact["name"]
        payload = payload_path.read_bytes()
        payload_path.write_bytes(payload[:-1] + bytes([payload[-1] ^ 1]))

        with pytest.raises(IntegrityError):
            client.replay_raw_manifest(manifest_path, mode="dbz")


def test_rdcap_replay_rejects_binding_mismatch_after_valid_manifest_hash_update(
    tmp_path: Path,
) -> None:
    ref = _seed_rdcap_frame(tmp_path / "cache")
    manifest_path = _write_rdcap_raw_manifest(tmp_path / "raw", ref)
    binding_path = manifest_path.parent / "raw" / "binding.json"
    binding = json.loads(binding_path.read_text(encoding="utf-8"))
    binding["station"] = "JPISHI"
    binding_bytes = json.dumps(binding, ensure_ascii=False, indent=2).encode("utf-8")
    binding_path.write_bytes(binding_bytes)

    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    descriptor = next(
        item for item in manifest["artifacts"] if item["name"] == "binding.json"
    )
    descriptor["size_bytes"] = len(binding_bytes)
    descriptor["sha256"] = hashlib.sha256(binding_bytes).hexdigest()
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    config = _offline_config(tmp_path / "cache", tmp_path)
    with Client(config=config) as client, pytest.raises(IntegrityError):
        client.replay_raw_manifest(manifest_path, mode="dbz")


@pytest.mark.parametrize(
    ("source", "product", "shape"),
    [
        ("rainviewer", "composite", (1024, 1024)),
        ("tw", "grid", (881, 921)),
        ("rdcap", "reflectivity", (901, 901)),
    ],
)
def test_direct_source_dbz_and_mode_none_replay_keep_the_native_field(
    tmp_path: Path, source: str, product: str, shape: tuple[int, int]
) -> None:
    cache_root = tmp_path / "cache"
    if source == "tw":
        ref = tw_grid_ref()
        seed_tw_grid_cache(cache_root, ref)
    elif source == "rdcap":
        ref = _seed_rdcap_frame(cache_root)
    else:
        ref = _seed_fixture_frame(cache_root, source, product)
    config = _offline_config(cache_root, tmp_path)

    with Client(config=config) as client:
        direct_array = None
        if source == "tw":
            direct = client.fetch(ref, mode="dbz")
            assert direct.data_kind == "native"
            direct_info = json.loads(direct.mode_info_json)
            assert direct_info["actual"] == "dbz"
            assert direct_info["units"] == "dBZ"
            direct_array = to_xarray(direct.native_field)
            assert direct_array.shape == shape
            assert direct_array.attrs["units"] == "dBZ"
            raw_report = client.download(
                [ref], output=tmp_path / "raw", raw_only=True, on_error="raise"
            )
            manifest_path = Path(raw_report.items[0].output_uri)
        elif source == "rdcap":
            manifest_path = _write_rdcap_raw_manifest(
                tmp_path / "fixture-raw", ref
            )
        else:
            manifest_path = _write_fixture_raw_manifest(
                tmp_path / "fixture-raw", ref, source, product
            )
        science = client.replay_raw_manifest(manifest_path)
        assert science.name == "reflectivity"
        science_array = to_xarray(science)
        assert science_array.attrs["units"] == "dBZ"

        replayed = client.replay_raw_manifest(manifest_path, mode="dbz")
        assert replayed.data_kind == "native"
        replayed_array = to_xarray(replayed.native_field)
        assert replayed_array.shape == shape
        np.testing.assert_array_equal(replayed_array.values, science_array.values)
        if direct_array is not None:
            np.testing.assert_array_equal(replayed_array.values, direct_array.values)


def test_blocked_tw_image_cannot_be_returned_as_dbz_or_raw_success(tmp_path: Path) -> None:
    fixture_path = REPO_ROOT / "tests/fixtures/sources/tw/fixture.json"
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    metadata = next(frame for frame in fixture["frames"] if frame["product"] == "observation")
    station = metadata["station"]
    artifacts = metadata["artifacts"]
    locator = {
        "url": metadata["uri"],
        "station": station,
        "artifacts": [],
        "name": artifacts[0]["name"],
        "media_type": artifacts[0]["media_type"],
    }
    ref = FrameRef(
        "tw",
        "observation",
        datetime.fromisoformat(metadata["valid_time"].replace("Z", "+00:00")),
        station=station,
        uri=metadata["uri"],
        locator=locator,
        locator_version=metadata["locator_version"],
        revision=metadata["revision"],
    )
    seed_native_raw_cache(
        tmp_path / "cache",
        ref,
        [
            (
                artifact["name"],
                artifact["media_type"],
                (fixture_path.parent / artifact["path"]).read_bytes(),
            )
            for artifact in artifacts
        ],
    )
    with Client(config=_offline_config(tmp_path / "cache", tmp_path)) as client, pytest.raises(
        DecodeError
    ) as caught:
        client.fetch(ref, mode="dbz")
    assert caught.value.code == "decode_unverified"
    assert caught.value.context.stage == "decode"


def test_async_cancelled_raw_replay_preserves_cancelled_status_without_network_fallback(
    tmp_path: Path,
) -> None:
    cache_root = tmp_path / "cache"
    ref, _artifact = _cached_fr_frame(cache_root)
    config = _offline_config(cache_root, tmp_path)

    with Client(config=config) as client:
        raw = client.download(
            [ref], output=tmp_path / "raw", raw_only=True, on_error="raise"
        )
        manifest_path = Path(raw.items[0].output_uri)

    async def exercise() -> OperationCancelled:
        async with AsyncClient(config=config) as client:
            client.cancel()
            with pytest.raises(OperationCancelled) as caught:
                await client.replay_raw_manifest(manifest_path, mode="dbz")
            return caught.value

    cancelled = asyncio.run(exercise())
    assert cancelled.code == "cancelled"
