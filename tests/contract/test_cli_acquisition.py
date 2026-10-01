import json

from click.testing import CliRunner
from radiust.cli.main import main


def test_cli_rejects_public_network_without_download(tmp_path):
    config = tmp_path / "offline.yaml"
    config.write_text("runtime:\n  allow_network: false\n", encoding="utf-8")
    output = tmp_path / "output"
    result = CliRunner().invoke(
        main,
        ["--conf", str(config), "download", "my", "--latest", "--dry-run", "--json", "--output", str(output)],
    )
    assert result.exit_code == 2
    payload = json.loads(result.output)
    assert payload["error"]["message"] == "Unexpected operation failure"
    assert payload["error"]["stage"] == "validate"
    assert not output.exists()
