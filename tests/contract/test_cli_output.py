from __future__ import annotations

import json
from datetime import datetime, timezone

import pytest
from click.testing import CliRunner
from radiust import Client, Query
from radiust.cli.main import main
from radiust.errors import StorageError
from radiust.storage.commit import InMemoryRemoteBackend


@pytest.mark.parametrize("source", ["au", "id_sidarma", "tw"])
def test_list_products_json_serializes_frozen_product_metadata(source):
    result = CliRunner().invoke(main, ["list", "products", source, "--json"])

    assert result.exit_code == 0, result.output
    payload = json.loads(result.output)
    assert payload["command"] == "list"
    assert payload["items"]
    assert all("historical" in product for product in payload["items"])


def test_download_accepts_output_and_cache_options(tmp_path):
    result = CliRunner().invoke(
        main,
        [
            "download",
            "my",
            "--at",
            "2025-12-29T06:50:01Z",
            "--output",
            str(tmp_path / "output"),
            "--output-template",
            "{source}/{product}/{valid_time}.{ext}",
            "--cache-dir",
            str(tmp_path / "cache"),
            "--no-cache",
            "--json",
        ],
    )

    assert result.exit_code == 0, result.output
    assert json.loads(result.output)["counts"]["written"] == 1


def test_download_rejects_geographic_options_without_complete_grid_request():
    result = CliRunner().invoke(
        main,
        [
            "download",
            "my",
            "--at",
            "2025-12-29T06:50:01Z",
            "--bbox",
            "100,0,110,10",
        ],
    )

    assert result.exit_code == 2
    assert "grid" in result.output.lower()


def test_remote_output_uri_uses_the_remote_commit_adapter():
    query = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))
    with Client(_remote_backend=InMemoryRemoteBackend()) as client:
        report = client.download(query, output="s3://bucket/prefix")

    assert report.counts["written"] == 1
    assert report.items[0].output_uri.startswith("s3://bucket/prefix/")


def test_unsupported_remote_output_fails_before_discovery():
    query = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))

    with Client() as client, pytest.raises(StorageError, match="s3:// or oss://"):
        client.download(query, output="https://objects.example.invalid/prefix")
