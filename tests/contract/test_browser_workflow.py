from pathlib import Path

import yaml


def test_pagasa_loopback_browser_replay_has_a_credential_free_ci_job() -> None:
    workflow = yaml.safe_load(Path(".github/workflows/offline.yml").read_text(encoding="utf-8"))
    job = workflow["jobs"].get("browser-contract")

    assert job is not None
    assert job["runs-on"] == "ubuntu-latest"
    steps = job["steps"]
    commands = [step.get("run", "") for step in steps]
    assert any("uv sync --no-dev --group ci --extra playwright --locked" in command for command in commands)
    assert any("playwright install --with-deps chromium" in command for command in commands)
    replay = next(step for step in steps if "test_ph_browser_replay.py" in step.get("run", ""))
    assert replay["env"]["RADIUST_TEST_ALLOW_LIVE"] == "1"
    assert replay["env"]["RADIUST_TEST_REAL_BROWSER"] == "1"
    assert "secrets." not in repr(job)
