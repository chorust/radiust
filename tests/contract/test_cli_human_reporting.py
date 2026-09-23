"""Human reports retain source and error identity at narrow widths."""

from click.testing import CliRunner
from radiust.cli.main import main
from radiust.cli.reporting import emit


def test_cjk_table_padding_and_narrow_lines_use_display_columns():
    from radiust.cli.layout import cell_width, render

    rows = [{"source": "\u4e2d\u56fd", "status": "ready"}, {"source": "AU", "status": "failed"}]
    table = render(rows, command="list", columns=80).splitlines()
    assert len({cell_width(line.split(" | ")[0]) for line in table if " | " in line}) == 1
    narrow = render([{"description": "\u96f7\u8fbe" * 45}], command="list", columns=40)
    assert all(cell_width(line) <= 40 for line in narrow.splitlines())
    assert narrow.replace("\n", "").replace(" ", "").count("\u96f7\u8fbe") == 45


def test_list_and_config_are_labeled():
    sources = CliRunner().invoke(main, ["list", "sources"])
    assert sources.exit_code == 0
    assert "source" in sources.output.lower()
    assert "total" in sources.output.lower()
    assert "SourceInfo(" not in sources.output
    config = CliRunner().invoke(main, ["config", "show"])
    assert config.exit_code == 0
    assert "runtime" in config.output
    assert "{" not in config.output


def test_narrow_width_keeps_identity_and_reason(capsys, monkeypatch):
    monkeypatch.setenv("COLUMNS", "40")
    emit([{"source": "长长的雷达站", "product": "composite", "station": "station-very-long-id", "status": "failed", "error": {"message": "upstream failed with a very long explanation"}}], command="discover")
    result = capsys.readouterr().out
    assert "长长的雷达站" in result
    assert "station-very-long-id" in result
    assert "upstream failed with a very long explanation" in " ".join(result.split())
    assert "{" not in result


def test_human_report_omits_absent_optional_fields(capsys):
    from radiust.models import DownloadReport, FrameResult

    from tests.support.cli_experience import frame

    emit(DownloadReport("download", "review", (FrameResult(frame(), "written"),)))
    output = capsys.readouterr().out
    assert "status: written" in output
    assert "error:" not in output
    assert "output_uri:" not in output


def test_quiet_keeps_json():
    result = CliRunner().invoke(main, ["--quiet", "list", "sources", "--json"])
    assert result.exit_code == 0
    assert '"schema_version"' in result.output
