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


def test_human_download_report_preserves_identity_location_and_actionable_error(capsys):
    from radiust.cli.reporting import emit
    from radiust.models import DownloadReport, FrameResult

    from tests.support.cli_experience import frame

    report = DownloadReport("download", "local-test", (
        FrameResult(frame(station="ok"), "written", output_uri="/tmp/radiust-fixture.nc"),
        FrameResult(frame(station="offline"), "failed", error={"code": "authentication", "message": "credentials are absent"}),
    ))
    emit(report, command="download")
    output = capsys.readouterr().out
    assert all(token in output for token in (
        "DOWNLOAD", "ok", "offline", "written", "failed", "2026-09-22",
        "/tmp/radiust-fixture.nc", "authentication", "credentials are absent", "credentials",
    ))
    assert "Suggestion:" in output


def test_empty_human_discover_report_explains_absence_without_changing_json(capsys):
    from radiust.cli.reporting import emit

    payload = {"schema_version": 1, "command": "discover", "counts": {"total": 0},
               "items": [], "error": None, "interrupted": False}
    emit(payload, command="discover")
    text = capsys.readouterr().out
    assert "DISCOVER" in text and "No data found" in text
    emit(payload, as_json=True, quiet=True, command="discover")
    assert json.loads(capsys.readouterr().out) == payload


def test_unknown_error_code_gets_no_speculative_recovery_advice(capsys):
    from radiust.cli.reporting import emit

    emit({"schema_version": 1, "command": "download", "counts": {"failed": 1},
          "items": [{"source": "th", "status": "failed",
                     "error": {"code": "unclassified", "message": "inspect logs"}}]},
         command="download")
    text = capsys.readouterr().out
    assert "unclassified" in text and "inspect logs" in text
    assert "Suggestion:" not in text


def test_list_discover_and_download_have_visible_command_identity(monkeypatch):
    from radiust.client import Client

    from tests.support.cli_experience import frame

    monkeypatch.setattr(Client, "discover", lambda *_args: [frame()])
    runner = CliRunner()
    expected = (
        (["list", "sources"], "LIST", "source", "Total:"),
        (["discover", "th"], "DISCOVER", "station", "valid_time"),
        (["download", "th", "--dry-run"], "DOWNLOAD", "planned", "Items:"),
    )
    for argv, title, identity, summary in expected:
        result = runner.invoke(main, argv)
        assert result.exit_code == 0, result.output
        assert result.output.startswith(title + "\n")
        assert identity in result.output and summary in result.output


def test_doctor_config_cache_human_reports_use_labeled_groups(tmp_path):
    runner = CliRunner()
    config = tmp_path / "radiust.yaml"
    config.write_text(
        "storage:\n  output: " + str(tmp_path / "output") +
        "\ncache:\n  dir: " + str(tmp_path / "cache") + "\n",
        encoding="utf-8",
    )
    doctor = runner.invoke(main, ["doctor", "--conf", str(config)])
    setting = runner.invoke(main, ["config", "show", "--conf", str(config)])
    cache = runner.invoke(main, ["cache", "status", "--cache-dir", str(tmp_path / "cache")])
    for response, header in ((doctor, "DOCTOR"), (setting, "CONFIG"), (cache, "CACHE STATUS")):
        assert response.exit_code == 0, response.output
        assert response.output.startswith(header + "\n")
        assert "{" not in response.output
    assert "dependencies:" in doctor.output and "cache_writable:" in doctor.output
    assert "runtime:" in setting.output and "allow_network:" in setting.output
    assert "entries:" in cache.output


def test_config_and_diagnostic_output_never_exposes_authentication_details(tmp_path):
    config = tmp_path / "radiust.yaml"
    config.write_text(
        "sources:\n  th:\n    access_key: sample-secret-key\n    secret_key: sample-secret-token\n",
        encoding="utf-8",
    )
    runner = CliRunner()
    for argv in (["config", "show"], ["doctor", "--source", "th"]):
        response = runner.invoke(main, [*argv, "--conf", str(config), "--verbose"])
        assert response.exit_code == 0, response.output
        assert "sample-secret-key" not in response.output
        assert "sample-secret-token" not in response.output


def test_doctor_and_config_verbose_add_safe_diagnostics_without_changing_json(tmp_path):
    runner = CliRunner()
    conf = tmp_path / "safe.yaml"
    conf.write_text("runtime:\n  allow_network: false\n", encoding="utf-8")
    for argv in (["doctor"], ["config", "show"]):
        plain = runner.invoke(main, [*argv, "--conf", str(conf)])
        verbose = runner.invoke(main, [*argv, "--conf", str(conf), "--verbose"])
        assert (plain.exit_code, verbose.exit_code) == (0, 0)
        assert "diagnostics:" not in plain.output.lower()
        assert "diagnostics:" in verbose.output.lower()
        machine = runner.invoke(main, [*argv, "--conf", str(conf), "--json"])
        verbose_machine = runner.invoke(main, [*argv, "--conf", str(conf), "--verbose", "--json"])
        assert json.loads(machine.output) == json.loads(verbose_machine.output)


def test_cache_quiet_verbose_are_honored_at_root_group_and_leaf(tmp_path):
    runner = CliRunner()
    path = str(tmp_path / "cache")
    for argv in (["--quiet", "cache", "status"], ["cache", "--quiet", "status"],
                 ["cache", "status", "--quiet"]):
        quiet = runner.invoke(main, [*argv, "--cache-dir", path])
        assert quiet.exit_code == 0 and not quiet.output, quiet.output
        machine = runner.invoke(main, [*argv, "--cache-dir", path, "--json"])
        assert machine.exit_code == 0 and json.loads(machine.output)["entries"] == 0
    verbose = runner.invoke(main, ["cache", "status", "--cache-dir", path, "--verbose"])
    assert verbose.exit_code == 0 and "diagnostics:" in verbose.output.lower()
    for argv in (["--quiet", "cache", "status", "--verbose"],
                 ["cache", "--verbose", "status", "--quiet"],
                 ["cache", "status", "--quiet", "--verbose"]):
        conflict = runner.invoke(main, [*argv, "--cache-dir", path])
        assert conflict.exit_code == 2 and "mutually exclusive" in conflict.output.lower()
