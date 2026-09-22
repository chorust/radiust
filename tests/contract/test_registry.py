import json

import pytest
from radiust.errors import UnsupportedQueryError
from radiust.registry import SourceRegistry, sources


def test_catalog_has_all_head_sources_without_network():
    ids = {item.id for item in sources()}
    assert len(ids) == 24
    assert {"my", "tw", "rainviewer", "bmkg"} <= ids


def test_catalog_construction_does_not_import_builtin_source_adapters(monkeypatch):
    imported: list[str] = []

    def fail_import(name: str):
        imported.append(name)
        raise AssertionError(f"catalog construction imported adapter {name}")

    monkeypatch.setattr("radiust.registry.import_module", fail_import)
    registry = SourceRegistry()
    assert len(registry.infos()) == 24
    assert imported == []


def test_builtin_catalog_is_independent_of_current_directory(tmp_path, monkeypatch):
    shadow_resources = tmp_path / "radiust" / "resources"
    shadow_resources.mkdir(parents=True)
    (shadow_resources / "catalog.json").write_text(
        '{"sources":[{"id":"cwd-shadow","description":"must not load"}]}',
        encoding="utf-8",
    )
    monkeypatch.chdir(tmp_path)
    monkeypatch.syspath_prepend(str(tmp_path))

    registry = SourceRegistry()

    ids = {item.id for item in registry.infos()}
    assert len(ids) == 24
    assert "cwd-shadow" not in ids
    assert {"rainviewer", "id_sidarma", "fr"} <= ids


def test_us2_sources_have_lazy_factories():
    registry = SourceRegistry()
    assert registry.get("rainviewer").__class__.__name__ == "RainViewerSource"
    assert registry.get("id_sidarma").__class__.__name__ == "IdSidarmaSource"
    assert registry.get("fr").__class__.__name__ == "FrSource"


@pytest.mark.source_adapter
def test_my_registry_uses_provider_adapter_without_fixture_fallback():
    from radiust.sources.my import MySource

    source = SourceRegistry().get("my")
    assert isinstance(source, MySource)
    assert source.fixture_path is None


def test_catalog_only_source_does_not_fall_back_to_a_fixture(tmp_path):
    catalog_path = tmp_path / "catalog.json"
    catalog_path.write_text(
        json.dumps({"sources": [{"id": "unimplemented", "description": "offline-only", "products": []}]}),
        encoding="utf-8",
    )
    registry = SourceRegistry(catalog_path)

    with pytest.raises(UnsupportedQueryError, match="no registered adapter"):
        registry.get("unimplemented")


def test_migrated_http_sources_have_lazy_factories():
    registry = SourceRegistry()
    expected = {
        "tw-http": "TwHttpSource",
        "es": "EsSource",
        "pt": "PtSource",
        "sg": "SgSource",
        "th": "ThSource",
    }
    assert {source_id: registry.get(source_id).__class__.__name__ for source_id in expected} == expected


def test_zero_argument_entry_point_factory_can_be_resolved(monkeypatch):
    from types import SimpleNamespace

    from radiust.models import SourceInfo

    info = SourceInfo(id="external-test", description="External source", adapter_version="1", products=())
    created = []

    def create_source():
        source = SimpleNamespace(info=info)
        created.append(source)
        return source

    monkeypatch.setattr("radiust.registry.entry_points", lambda **_kwargs: [SimpleNamespace(name="external-test", load=lambda: create_source)])
    registry = SourceRegistry()
    assert registry.get("external-test").info == info
    assert registry.get("external-test") is created[0]
