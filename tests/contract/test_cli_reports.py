from __future__ import annotations

import json

from click.testing import CliRunner
from radiust.cli.main import main


def test_cli_machine_reports_are_single_json_objects():
    result = CliRunner().invoke(main, ["list", "sources", "--json"])

    assert result.exit_code == 0, result.output
    payload = json.loads(result.output)
    assert payload["schema_version"] == 1
    assert payload["command"] == "list"
    assert isinstance(payload["items"], list)


def test_cache_cli_status_and_non_tty_clear_requires_yes(tmp_path):
    runner = CliRunner()
    status = runner.invoke(main, ["cache", "status", "--cache-dir", str(tmp_path / "cache"), "--json"])
    assert status.exit_code == 0, status.output
    assert json.loads(status.output)["entries"] == 0

    clear = runner.invoke(main, ["cache", "clear", "--cache-dir", str(tmp_path / "cache")])
    assert clear.exit_code != 0
    assert "--yes" in clear.output
