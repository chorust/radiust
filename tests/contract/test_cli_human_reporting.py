"""The Rust-backed CLI keeps human reports labeled."""

from click.testing import CliRunner
from radiust.cli.main import main


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


def test_quiet_keeps_json():
    result = CliRunner().invoke(main, ["--quiet", "list", "sources", "--json"])
    assert result.exit_code == 0
    assert '"schema_version"' in result.output
