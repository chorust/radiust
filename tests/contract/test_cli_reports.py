from __future__ import annotations

import json

from click.testing import CliRunner
from radiust.cli.main import main
from radiust.cli.reporting import emit, exit_code
from radiust.models import DownloadReport, FrameResult

from tests.support.cli_experience import frame


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


def test_single_download_report_envelope_preserves_types_and_redacts_nested_errors(capsys):
    report = DownloadReport(
        "download", "test-run",
        (FrameResult(frame(), "failed", error={"code": "error", "message": "token=private", "retryable": False}),),
        query={"source": "th", "latest": True},
    )
    emit(report, as_json=True, quiet=True)
    payload = json.loads(capsys.readouterr().out)
    assert (payload["schema_version"], payload["command"], payload["run_id"]) == (1, "download", "test-run")
    assert payload["counts"]["failed"] == 1
    assert payload["query"]["latest"] is True
    assert payload["items"][0]["error"]["retryable"] is False
    assert "private" not in repr(payload)


def test_single_source_exit_codes_remain_unchanged():
    ref = frame()
    failure = FrameResult(ref, "failed", error={"code": "upstream_failed"})
    no_data = FrameResult(ref, "failed", error={"code": "no_data"})
    success = FrameResult(ref, "written")
    assert exit_code(DownloadReport("download", "run", (success,))) == 0
    assert exit_code(DownloadReport("download", "run", (failure,))) == 5
    assert exit_code(DownloadReport("download", "run", (no_data,))) == 3
    assert exit_code(DownloadReport("download", "run", (success, failure))) == 4
    assert exit_code(DownloadReport("download", "run", (failure,), interrupted=True)) == 130
