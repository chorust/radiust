from __future__ import annotations

import pytest
from radiust.config import load_config
from radiust.errors import ConfigError


@pytest.fixture(autouse=True)
def isolated_user_config(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path / "home"))


def test_missing_automatic_files_keep_defaults_and_invalid_existing_file_errors(
    tmp_path, monkeypatch
):
    monkeypatch.chdir(tmp_path)
    config = load_config(environ={})
    assert config.values["runtime"]["allow_network"] is False
    assert config.origins["runtime"] == "default"

    (tmp_path / "config.yaml").write_text("runtime: [", encoding="utf-8")
    with pytest.raises(ConfigError, match="invalid YAML"):
        load_config(environ={})


def test_automatic_config_files_merge_user_project_environment_and_sdk_overrides(
    tmp_path, monkeypatch
):
    home = tmp_path / "home"
    project = tmp_path / "project"
    user_config = home / ".config" / "radiust" / "config.yaml"
    user_config.parent.mkdir(parents=True)
    project.mkdir()
    user_config.write_text(
        "runtime:\n  frame_concurrency: 3\n  discovery_workers: 7\n"
        "sources:\n  id_sidarma:\n    api_key: user-private\n    radar_ids: [ACE]\n",
        encoding="utf-8",
    )
    (project / "config.yaml").write_text(
        "runtime:\n  frame_concurrency: 4\nsources:\n  id_sidarma:\n    radar_ids: [JAK]\n",
        encoding="utf-8",
    )
    monkeypatch.chdir(project)
    monkeypatch.setenv("HOME", str(home))

    config = load_config(environ={"HOME": str(home)})
    assert config.values["runtime"]["frame_concurrency"] == 4
    assert config.values["runtime"]["discovery_workers"] == 7
    assert config.values["sources"]["id_sidarma"] == {
        "api_key": "user-private",
        "radar_ids": ["JAK"],
    }
    assert config.origins["sources.id_sidarma.api_key"] == "user_file"
    assert config.origins["sources.id_sidarma.radar_ids"] == "file"
    assert "user-private" not in repr(config.redacted())

    environment = {"HOME": str(home), "RADIUST_RUNTIME__FRAME_CONCURRENCY": "5"}
    config = load_config(environ=environment)
    assert config.values["runtime"]["frame_concurrency"] == 5
    assert config.origins["runtime.frame_concurrency"] == "environment"
    config = load_config({"runtime": {"frame_concurrency": 6}}, environ=environment)
    assert config.values["runtime"]["frame_concurrency"] == 6
    assert config.origins["runtime.frame_concurrency"] == "explicit"

    custom = project / "custom.yaml"
    custom.write_text("output:\n  format: png\n", encoding="utf-8")
    (project / "config.yaml").write_text("invalid project: [", encoding="utf-8")
    config = load_config(path=custom, environ=environment)
    assert config.values["runtime"]["discovery_workers"] == 7
    assert config.values["output"]["format"] == "png"


def test_config_precedence_is_explicit_over_environment_over_file_over_default(
    tmp_path, monkeypatch
):
    path = tmp_path / "radiust.yaml"
    path.write_text("runtime:\n  frame_concurrency: 3\noutput:\n  format: png\n", encoding="utf-8")
    monkeypatch.setenv("RADIUST_RUNTIME__FRAME_CONCURRENCY", "4")

    config = load_config({"runtime": {"frame_concurrency": 5}}, path=path)

    assert config.values["runtime"]["frame_concurrency"] == 5
    assert config.values["output"]["format"] == "png"
    assert "resampling" not in config.values["output"]
    assert config.origins["runtime"] == "explicit"
    assert config.origins["runtime.frame_concurrency"] == "explicit"
    assert config.origins["output"] == "file"
    assert config.origins["output.format"] == "file"


def test_config_rejects_duplicate_and_unknown_fields(tmp_path):
    duplicate = tmp_path / "duplicate.yaml"
    duplicate.write_text(
        "runtime:\n  frame_concurrency: 2\nruntime:\n  frame_concurrency: 3\n", encoding="utf-8"
    )
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
    assert redacted["sources"]["provider"] == {
        "access_key": "<configured>",
        "secret_key": "<configured>",
        "token": "<configured>",
    }


def test_source_environment_keys_are_nested_without_exposing_values():
    config = load_config(
        environ={
            "RADIUST_SOURCES__provider__ACCESS_KEY": "a",
            "RADIUST_SOURCES__provider__SECRET_KEY": "s",
        }
    )

    assert config.values["sources"]["provider"] == {"access_key": "a", "secret_key": "s"}
    assert config.redacted()["sources"]["provider"] == {
        "access_key": "<configured>",
        "secret_key": "<configured>",
    }


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


def test_config_recursively_redacts_nested_source_secrets():
    config = load_config(
        {
            "sources": {
                "provider": {
                    "token": "source-token-private",
                    "request": {
                        "headers": [
                            {"Authorization": "Bearer header-private"},
                            {"cookie": "session-private"},
                        ]
                    },
                }
            }
        },
        environ={},
    )

    redacted = config.redacted()
    assert redacted["sources"]["provider"]["token"] == "<configured>"
    assert redacted["sources"]["provider"]["request"]["headers"] == [
        {"Authorization": "<configured>"},
        {"cookie": "<configured>"},
    ]
    assert "source-token-private" not in repr(redacted)
    assert "header-private" not in repr(redacted)
    assert "session-private" not in repr(redacted)


def test_redaction_tracks_mutable_effective_values():
    config = load_config(environ={})
    config.values["sources"]["provider"] = {"api_key": "added-after-load"}

    assert config.redacted()["sources"]["provider"]["api_key"] == "<configured>"


def test_empty_yaml_and_zero_cache_capacity_keep_sdk_compatibility(tmp_path):
    path = tmp_path / "empty.yaml"
    path.write_text("# intentionally empty\n", encoding="utf-8")

    config = load_config({"cache": {"max_bytes": 0}}, path=path, environ={})

    assert config.values["cache"]["max_bytes"] == 0
    assert config.origins["cache"] == "explicit"


def test_config_rejects_overlapping_cache_and_output_roots(tmp_path):
    output = tmp_path / "data"
    cache = output / "cache"
    with pytest.raises(ConfigError, match="cannot overlap"):
        load_config(
            {"storage": {"output": str(output)}, "cache": {"dir": str(cache)}},
            environ={},
        )
