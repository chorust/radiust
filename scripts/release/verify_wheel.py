"""Install an exact wheel in a clean virtual environment outside the checkout."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path
from zipfile import ZipFile

SMOKE = '''
import importlib.metadata
import importlib.util
import json
import subprocess
import sys
from pathlib import Path
import radiust
from radiust import _core
from radiust.registry import sources

expected = sys.argv[1]
assert importlib.metadata.version("radiust") == expected
assert radiust.__version__ == expected
assert _core.version() == expected
assert importlib.util.find_spec("numpy") is None
assert importlib.util.find_spec("xarray") is None
assert importlib.util.find_spec("radiust.sources") is None
assert callable(radiust.Client) and callable(radiust.AsyncClient)
assert sources()
commands = [["--version"], ["list", "sources", "--json"],
            ["list", "stations", "id", "--json"],
            ["list", "stations", "id_sidarma", "--json"]]
for command in commands:
    result = subprocess.run([sys.executable, "-m", "radiust", *command],
                            capture_output=True, text=True, check=True)
    if command == ["--version"]:
        assert result.stdout.strip() == "radiust " + expected
    else:
        assert json.loads(result.stdout)["error"] is None
console = subprocess.run([str(Path(sys.executable).with_name("radiust")), "--version"],
                         capture_output=True, text=True, check=True)
assert console.stdout.strip() == "radiust " + expected
print(json.dumps({"version": expected, "installed_from": radiust.__file__,
                  "sdk": "passed", "cli": "passed", "provider_network": False}))
'''


def audit_contents(wheel: Path) -> None:
    with ZipFile(wheel) as archive:
        fixtures = [name for name in archive.namelist() if "tests/fixtures/" in name]
    if fixtures:
        raise ValueError(f"validation-only fixtures cannot be distributed: {fixtures}")


def verify(wheel: Path | None, interpreter: str, version: str) -> dict:
    if wheel is not None:
        wheel = wheel.resolve()
        audit_contents(wheel)
    env = dict(os.environ)
    env.pop("PYTHONPATH", None)
    env.pop("PYTHONHOME", None)
    env["PYTHONNOUSERSITE"] = "1"
    with tempfile.TemporaryDirectory(prefix="radiust-wheel-install-") as temporary:
        isolated = Path(temporary)
        venv = isolated / "venv"
        subprocess.run([interpreter, "-m", "venv", str(venv)], cwd=isolated, env=env, check=True)
        python = venv / "bin/python"
        arguments = ["--no-index", str(wheel)] if wheel is not None else ["--index-url", "https://pypi.org/simple", "--only-binary=:all:", f"radiust=={version}"]
        subprocess.run([str(python), "-m", "pip", "--isolated", "install", "--no-deps", *arguments], cwd=isolated, env=env, check=True)
        result = subprocess.run([str(python), "-I", "-c", SMOKE, version], cwd=isolated, env=env, check=True, capture_output=True, text=True)
        return json.loads(result.stdout)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("wheel", type=Path, nargs="?")
    parser.add_argument("--from-index", action="store_true")
    parser.add_argument("--python", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    if (args.wheel is None) != args.from_index:
        parser.error("supply either a wheel path or --from-index")
    report = verify(args.wheel, args.python, args.version)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
