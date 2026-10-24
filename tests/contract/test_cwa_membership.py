"""CWA membership configuration and product-scoped preflight stay offline."""

import hashlib
import json
import subprocess
import sys
from pathlib import Path

import pytest
import radiust
from radiust.config import load_config


def test_membership_environment_overrides_yaml_and_is_redacted(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    path = tmp_path / "cwa.yaml"
    path.write_text("sources:\n  tw:\n    api_key: synthetic-file-key\n")
    config = load_config(path=path, environ={"RADIUST_SOURCES__TW__API_KEY": "synthetic-env-key"})
    assert config.values["sources"]["tw"]["api_key"] == "synthetic-env-key"
    assert config.origins["sources.tw.api_key"] == "environment"
    assert config.redacted()["sources"]["tw"]["api_key"] == "<configured>"


def test_only_membership_product_requires_key_before_network_preflight(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    config = load_config(
        {"runtime": {"allow_network": False}, "sources": {"tw": {"api_key": ""}}},
        environ={},
    )
    with radiust.Client(config=config) as client:
        for product, expected in [
            ("observation-wide", "missing_credentials"),
            ("observation", "network_restricted"),
            ("grid", "network_restricted"),
        ]:
            report = client.discover_report(radiust.Query("tw", product=product, latest=True))
            assert len(report.items) == 1
            assert report.items[0].status == expected


@pytest.mark.asyncio
async def test_async_membership_preflight_matches_sync(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    config = load_config(
        {"runtime": {"allow_network": False}, "sources": {"tw": {"api_key": ""}}},
        environ={},
    )
    async with radiust.AsyncClient(config=config) as client:
        report = await client.discover_report(
            radiust.Query("tw", product="observation-wide", latest=True)
        )
        assert len(report.items) == 1
        assert report.items[0].status == "missing_credentials"


@pytest.mark.parametrize(
    "corruption",
    [
        None,
        "digest",
        "product",
        "time",
        "shape",
        "source",
        "complete",
        "names",
        "size",
        "dataset",
        "authorization",
    ],
)
def test_acceptance_readback_survives_optimization(tmp_path, corruption):
    from PIL import Image

    raw = tmp_path / "raw"
    raw.mkdir()
    Image.new("RGB", (3, 2)).save(raw / "O-A0058-001.png")
    metadata = {
        "cwaopendata": {
            "dataid": "O-A0058-001",
            "dataset": {"DateTime": "2026-01-01T00:00:00+00:00"},
        }
    }
    if corruption == "dataset":
        metadata["cwaopendata"]["dataid"] = "wrong"
    (raw / "O-A0058-001.json").write_text(json.dumps(metadata))
    document = {
        "ref": {
            "source": "tw",
            "product": "observation-wide",
            "logical_id": "synthetic",
            "valid_time": "2026-01-01T00:00:00Z",
            "locator": {"provider_image_shape": [2, 3], "geometry_status": "unverified"},
        },
        "raw_complete": True,
        "artifacts": [
            {
                "name": path.name,
                "size_bytes": len(path.read_bytes()),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
            for path in sorted(raw.iterdir())
        ],
    }
    if corruption == "digest":
        document["artifacts"][0]["sha256"] = "0" * 64
    elif corruption in {"product", "source"}:
        document["ref"][corruption] = "wrong"
    elif corruption == "time":
        document["ref"]["valid_time"] = "2026-01-01T00:01:00Z"
    elif corruption == "shape":
        document["ref"]["locator"]["provider_image_shape"] = [3, 2]
    elif corruption == "complete":
        document["raw_complete"] = False
    elif corruption == "names":
        document["artifacts"] = document["artifacts"][:1]
    elif corruption == "size":
        document["artifacts"][0]["size_bytes"] += 1
    elif corruption == "authorization":
        document["Authorization"] = "synthetic-sensitive-marker"
    manifest = tmp_path / "manifest.json"
    manifest.write_text(json.dumps(document))
    script = Path(__file__).resolve().parents[2] / "scripts/validation/validate_cwa_membership.py"
    result = subprocess.run(
        [
            sys.executable,
            "-O",
            "-c",
            "import runpy, sys; module = runpy.run_path(sys.argv[1]); "
            "module['readback'](__import__('pathlib').Path(sys.argv[2]))",
            str(script),
            str(manifest),
        ],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if corruption is None:
        assert result.returncode == 0, result.stderr
    else:
        assert result.returncode != 0, f"optimized readback accepted {corruption} corruption"
        assert "synthetic-sensitive-marker" not in result.stderr


@pytest.mark.parametrize(
    "failure",
    ["cli_failed", "cli_count", "sync_count", "sync_status", "async_count", "async_status"],
)
def test_acceptance_report_checks_survive_optimization(tmp_path, failure):
    script = Path(__file__).resolve().parents[2] / "scripts/validation/validate_cwa_membership.py"
    # Exercise main's real report checks while replacing all acquisition boundaries.
    code = """
import json
from pathlib import Path
import runpy
import sys
from types import SimpleNamespace

script, output, failure = sys.argv[1:]
module = runpy.run_path(script)
main = module['main']
namespace = main.__globals__
namespace['readback'] = lambda path: {'logical_id': 'synthetic', 'artifacts': []}

def report(stage):
    status = 'failed' if failure == stage + '_status' else 'written'
    items = [] if failure == stage + '_count' else [SimpleNamespace(status=status, output_uri='synthetic')]
    return SimpleNamespace(items=items)

class Client:
    def __init__(self, **kwargs): pass
    def __enter__(self): return self
    def __exit__(self, *args): pass
    def download(self, *args, **kwargs): return report('sync')

class AsyncClient:
    def __init__(self, **kwargs): pass
    async def __aenter__(self): return self
    async def __aexit__(self, *args): pass
    async def download(self, *args, **kwargs): return report('async')

namespace['radiust'] = SimpleNamespace(Query=lambda *args, **kwargs: None, Client=Client, AsyncClient=AsyncClient)
cli_report = {'counts': {'failed': int(failure == 'cli_failed')}, 'items': [] if failure == 'cli_count' else [{'output_uri': 'synthetic'}]}
namespace['subprocess'] = SimpleNamespace(run=lambda *args, **kwargs: SimpleNamespace(returncode=0, stdout=json.dumps(cli_report)))
sys.argv = [script, '--live', '--output', output, '--report', str(Path(output) / 'report.json')]
try:
    main()
except ValueError as error:
    print(str(error))
    sys.exit(7)
"""
    result = subprocess.run(
        [sys.executable, "-O", "-c", code, str(script), str(tmp_path), failure],
        capture_output=True,
        text=True,
        timeout=30,
    )
    stage = failure.split("_")[0]
    if stage == "cli":
        stage = "CLI"
    assert result.returncode == 7, result.stderr
    assert result.stdout.strip() == f"CWA {stage} acquisition report failed acceptance"
    assert not (tmp_path / "report.json").exists()
