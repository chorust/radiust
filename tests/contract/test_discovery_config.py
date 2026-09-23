import pytest
from radiust.config import load_config
from radiust.errors import ConfigError


def test_discovery_deadline_default_and_override():
    assert load_config(environ={}).values["runtime"]["discovery_deadline"] == 300.0
    assert load_config({"runtime": {"discovery_deadline": 0.5}}, environ={}).values["runtime"]["discovery_deadline"] == 0.5


def test_discovery_deadline_file_environment_override_and_other_deadline(tmp_path):
    path = tmp_path / "radiust.yaml"
    path.write_text("runtime:\n  discovery_deadline: 42\n  frame_deadline: 77\n")
    config = load_config(path=path, environ={"RADIUST_RUNTIME__DISCOVERY_DEADLINE": "6"})
    assert config.values["runtime"]["discovery_deadline"] == 6
    assert config.values["runtime"]["frame_deadline"] == 77
    explicit = load_config({"runtime": {"discovery_deadline": 2}}, path=path,
                           environ={"RADIUST_RUNTIME__DISCOVERY_DEADLINE": "6"})
    assert explicit.values["runtime"]["discovery_deadline"] == 2


@pytest.mark.parametrize("bad", [0, -1, "nan", "inf"])
def test_discovery_deadline_rejects_invalid_values(bad):
    with pytest.raises(ConfigError):
        load_config({"runtime": {"discovery_deadline": bad}}, environ={})


def test_client_context_keeps_configured_temp_parent_and_other_files(tmp_path):
    from radiust.client import Client

    parent = tmp_path / "shared-temp"
    parent.mkdir()
    sentinel = parent / "keep.txt"
    sentinel.write_text("caller-owned", encoding="utf-8")
    config = load_config({"runtime": {"temp_root": str(parent)}, "cache": {"enabled": False}}, environ={})
    with Client(config=config) as client:
        context = client._context("fake")
        owned = context.temp_root
        assert owned is not None and owned != parent and owned.parent == parent
        context.close()
    assert sentinel.read_text(encoding="utf-8") == "caller-owned"
    assert not owned.exists()
