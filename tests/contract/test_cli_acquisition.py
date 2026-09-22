from click.testing import CliRunner
from radiust.cli.main import main


def test_cli_rejects_public_network_without_download(tmp_path):
    result = CliRunner().invoke(main, ["download", "my", "--latest", "--dry-run", "--json", "--output", str(tmp_path)])
    assert result.exit_code == 0
    assert '"planned"' in result.output
