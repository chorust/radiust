from datetime import datetime, timezone

from click.testing import CliRunner
from radiust import Client, Query
from radiust.cli.main import main


def test_offline_discover_fetch_and_repeat(tmp_path):
    query = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))
    with Client() as client:
        refs = client.discover(query)
        field = client.fetch(query)
        first = client.download(query, output=tmp_path)
        second = client.download(query, output=tmp_path)
    assert len(refs) == 1
    assert field.data.shape[0] > 0
    assert first.counts["written"] == 1
    assert second.counts["skipped"] == 1


def test_cli_json_is_one_object(tmp_path):
    runner = CliRunner()
    result = runner.invoke(main, ["download", "my", "--at", "2025-12-29T06:50:01Z", "--output", str(tmp_path), "--json"])
    assert result.exit_code == 0, result.output
    import json

    payload = json.loads(result.output)
    assert payload["schema_version"] == 1
    assert payload["counts"]["written"] == 1
