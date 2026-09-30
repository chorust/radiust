import json

from click.testing import CliRunner
from radiust.cli.main import main


def test_historical_cptec_source_stays_outside_the_native_catalog():
    result = CliRunner().invoke(main, ["discover", "br_cptec", "--latest", "--json"])

    assert result.exit_code == 2
    error = json.loads(result.output)["error"]
    assert error["stage"] == "validate"
    assert "unknown source: br_cptec" in error["message"]
