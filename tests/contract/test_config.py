from __future__ import annotations

import pytest
from radiust.config import load_config
from radiust.errors import ConfigError


def test_config_precedence_is_explicit_over_environment_over_file_over_default(tmp_path, monkeypatch):
    path = tmp_path / "radiust.yaml"
    path.write_text("runtime:\n  frame_concurrency: 3\noutput:\n  format: png\n", encoding="utf-8")
    monkeypatch.setenv("RADIUST_RUNTIME__FRAME_CONCURRENCY", "4")

    config = load_config({"runtime": {"frame_concurrency": 5}}, path=path)

    assert config.values["runtime"]["frame_concurrency"] == 5
    assert config.values["output"]["format"] == "png"
    assert config.origins["runtime"] == "explicit"
    assert config.origins["output"] == "file"


def test_config_rejects_duplicate_and_unknown_fields(tmp_path):
    duplicate = tmp_path / "duplicate.yaml"
    duplicate.write_text("runtime:\n  frame_concurrency: 2\nruntime:\n  frame_concurrency: 3\n", encoding="utf-8")
    with pytest.raises(ConfigError, match="duplicate"):
        load_config(path=duplicate, environ={})

    with pytest.raises(ConfigError, match="unknown configuration field"):
        load_config({"runtime": {"not_a_limit": 1}}, environ={})


def test_config_rejects_half_credentials_and_redacts_complete_credentials():
    with pytest.raises(ConfigError, match="together"):
        load_config({"sources": {"provider": {"access_key": "a"}}}, environ={})

    config = load_config(
        {"sources": {"provider": {"access_key": "a", "secret_key": "s", "token": "t"}}},
        environ={},
    )
    redacted = config.redacted()
    assert redacted["sources"]["provider"] == {"access_key": "<configured>", "secret_key": "<configured>", "token": "<configured>"}


def test_source_environment_keys_are_nested_without_exposing_values():
    config = load_config(
        environ={
            "RADIUST_SOURCES__provider__ACCESS_KEY": "a",
            "RADIUST_SOURCES__provider__SECRET_KEY": "s",
        }
    )

    assert config.values["sources"]["provider"] == {"access_key": "a", "secret_key": "s"}
    assert config.redacted()["sources"]["provider"] == {"access_key": "<configured>", "secret_key": "<configured>"}


def test_config_rejects_invalid_limits_and_loads_example():
    with pytest.raises(ConfigError, match="positive"):
        load_config({"runtime": {"frame_concurrency": 0}}, environ={})

    config = load_config(path="config/example.yaml", environ={})
    assert config.values["runtime"]["allow_network"] is False
    assert config.origins["runtime.allow_network"] == "file"


def test_remote_output_uri_is_preserved_and_storage_credentials_are_redacted():
    with pytest.raises(ConfigError, match="together"):
        load_config({"storage": {"access_key": "a"}}, environ={})

    config = load_config(
        {
            "storage": {
                "output": "s3://bucket/prefix",
                "access_key": "a",
                "secret_key": "s",
                "endpoint": "http://127.0.0.1:9000",
            }
        },
        environ={},
    )

    assert config.values["storage"]["output"] == "s3://bucket/prefix"
    assert config.output_root is None
    assert config.redacted()["storage"]["access_key"] == "<configured>"
    assert config.redacted()["storage"]["secret_key"] == "<configured>"
