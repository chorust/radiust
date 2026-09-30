from __future__ import annotations

import json
from importlib import metadata
from types import SimpleNamespace

from click.testing import CliRunner
from radiust.cli.main import main


def _list_sources():
    result = CliRunner().invoke(main, ["list", "sources", "--json"])
    assert result.exit_code == 0, result.output
    return json.loads(result.output)


def test_native_catalog_lists_all_head_sources_without_network():
    payload = _list_sources()
    ids = {item["id"] for item in payload["items"]}

    assert payload["counts"]["items"] == 24
    assert len(ids) == 24
    assert {"my", "tw", "rainviewer", "bmkg"} <= ids


def test_native_catalog_does_not_enumerate_or_load_python_entry_points(monkeypatch):
    enumerated = []
    loaded = []

    def listing(**_kwargs):
        enumerated.append(True)
        return [
            SimpleNamespace(
                name="external-test",
                load=lambda: loaded.append("external-test"),
            )
        ]

    monkeypatch.setattr(metadata, "entry_points", listing)
    payload = _list_sources()

    assert len(payload["items"]) == 24
    assert enumerated == []
    assert loaded == []


def test_builtin_catalog_is_independent_of_current_directory(tmp_path, monkeypatch):
    shadow_resources = tmp_path / "radiust" / "resources"
    shadow_resources.mkdir(parents=True)
    (shadow_resources / "catalog.json").write_text(
        '{"sources":[{"id":"cwd-shadow","description":"must not load"}]}',
        encoding="utf-8",
    )
    monkeypatch.chdir(tmp_path)
    monkeypatch.syspath_prepend(str(tmp_path))

    payload = _list_sources()
    ids = {item["id"] for item in payload["items"]}

    assert payload["counts"]["items"] == 24
    assert "cwd-shadow" not in ids
    assert {"rainviewer", "id_sidarma", "fr"} <= ids


def test_native_discovery_rejects_unknown_source_ids():
    result = CliRunner().invoke(
        main, ["discover", "unimplemented", "--latest", "--json"]
    )

    assert result.exit_code == 2, result.output
    error = json.loads(result.output)["error"]
    assert error["stage"] == "validate"
    assert "unknown source: unimplemented" in error["message"]


def test_retired_source_reports_retirement_without_discovery():
    result = CliRunner().invoke(main, ["discover", "uk", "--latest", "--json"])

    assert result.exit_code == 2, result.output
    error = json.loads(result.output)["error"]
    assert error["code"] == "retired"
    assert error["stage"] == "discover"
    assert error["message"] == "source retired"
