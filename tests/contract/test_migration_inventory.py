import json
from pathlib import Path

import pytest
from radiust.registry import registry, sources

ROOT = Path(__file__).parents[2]

HEAD_HISTORICAL_CAPABILITY = {
    "au": True,
    "bmkg": False,
    "ca": True,
    "cam": True,
    "es": True,
    "fr": False,
    "id": True,
    "id_sidarma": False,
    "kr": True,
    "my": False,
    "nz": True,
    "opensnow": False,
    "ph": True,
    "pt": True,
    "rainviewer": True,
    "sg": True,
    "th": False,
    "th_royalrain": True,
    "tw": False,
    "tw-http": True,
    "uk": False,
    "vn": True,
    "windy": False,
    "wunderground": False,
}


def test_head_inventory_and_catalog_have_the_same_source_ids():
    inventory = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))
    catalog_ids = {item.id for item in sources()}
    inventory_ids = {item["id"] for item in inventory["sources"]}

    assert len(inventory["sources"]) == 24
    assert inventory_ids == catalog_ids
    for item in inventory["sources"]:
        assert (ROOT / item["fixture"]).is_file()
        assert (ROOT / item["migration"]).is_file()


def test_every_head_source_has_a_registered_adapter_resource_and_source_contract():
    inventory = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))
    for item in inventory["sources"]:
        source_id = item["id"]
        module_name = source_id.replace("-", "_")
        assert (ROOT / "python/radiust/sources" / f"{module_name}.py").is_file(), source_id
        resource_path = ROOT / "python/radiust/resources/sources" / f"{source_id}.json"
        assert resource_path.is_file(), source_id
        resource = json.loads(resource_path.read_text(encoding="utf-8"))
        assert resource.get("schema_version") == 1, source_id
        assert resource.get("source") == source_id, source_id
        assert resource.get("adapter_version") == registry.get_info(source_id).adapter_version, source_id
        assert (ROOT / "tests/sources" / f"test_{module_name}.py").is_file(), source_id
        assert registry.get(source_id).info.id == source_id


def test_source_resources_have_unique_json_keys():
    for path in sorted((ROOT / "python/radiust/resources/sources").glob("*.json")):
        duplicate_keys: list[str] = []

        def collect(items, duplicate_keys=duplicate_keys):
            seen: set[str] = set()
            for key, _value in items:
                if key in seen:
                    duplicate_keys.append(key)
                seen.add(key)
            return dict(items)

        json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=collect)
        assert not duplicate_keys, f"{path.name} repeats resource keys: {duplicate_keys}"


@pytest.mark.source_adapter
def test_catalog_and_resource_declare_each_source_historical_capability():
    catalog = {item.id: item for item in sources()}
    assert set(catalog) == set(HEAD_HISTORICAL_CAPABILITY)
    for source_id, expected in HEAD_HISTORICAL_CAPABILITY.items():
        resource = json.loads(
            (ROOT / "python/radiust/resources/sources" / f"{source_id}.json").read_text(
                encoding="utf-8"
            )
        )
        adapter_historical = bool(getattr(registry.get(source_id), "historical", True))
        assert resource.get("historical") is adapter_historical, source_id
        assert all(product.historical is expected for product in catalog[source_id].products), source_id


@pytest.mark.source_adapter
def test_source_resource_historical_capability_matches_adapter():
    inventory = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))
    for item in inventory["sources"]:
        source_id = item["id"]
        resource = json.loads(
            (ROOT / "python/radiust/resources/sources" / f"{source_id}.json").read_text(
                encoding="utf-8"
            )
        )
        adapter = registry.get(source_id)
        assert bool(resource.get("historical", True)) == bool(
            getattr(adapter, "historical", True)
        ), source_id


def test_every_head_source_manifest_and_migration_record_bind_to_inventory_id():
    inventory = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))
    for item in inventory["sources"]:
        source_id = item["id"]
        fixture = json.loads((ROOT / item["fixture"]).read_text(encoding="utf-8"))
        migration = json.loads((ROOT / item["migration"]).read_text(encoding="utf-8"))
        assert fixture["source"] == source_id
        assert migration["source"] == source_id
        assert migration["adapter"] == f"python/radiust/sources/{source_id.replace('-', '_')}.py"


def test_historical_inventory_records_keep_adapter_resource_fixture_and_test_links():
    inventory = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))
    for item in inventory["historical"]:
        source_id = item["id"]
        history = json.loads((ROOT / item["evidence"]).read_text(encoding="utf-8"))
        for key in ("source", "adapter", "resource", "fixture", "migration", "test"):
            assert key in history, f"{source_id} history has no {key} link"
        assert history["source"] == source_id
        assert (ROOT / history["adapter"]).is_file()
        assert (ROOT / history["resource"]).is_file()
        assert (ROOT / history["fixture"]).is_file()
        assert (ROOT / history["migration"]).is_file()
        assert (ROOT / history["test"]).is_file()


def test_each_inventory_fixture_is_either_hashed_or_explicitly_blocked():
    inventory = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))
    for item in inventory["sources"]:
        fixture = json.loads((ROOT / item["fixture"]).read_text(encoding="utf-8"))
        assert fixture.get("schema_version") == 1
        assert fixture.get("source") == item["id"]
        if fixture.get("status") == "blocked":
            assert fixture.get("blockers")
            assert fixture.get("evidence")
            assert fixture.get("frames") == []
        else:
            assert fixture.get("frames")


def test_blocked_source_migration_records_have_explicit_blockers_and_evidence():
    for path in sorted((ROOT / "migration/sources").glob("*.json")):
        record = json.loads(path.read_text(encoding="utf-8"))
        if "blocked" in str(record.get("status", "")):
            assert record.get("blockers"), f"{record.get('source')} has no blocker detail"
            assert record.get("evidence"), f"{record.get('source')} has no evidence links"


def test_unverified_sources_are_not_advertised_as_available():
    inventory = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))
    catalog = {item.id: item for item in sources()}

    for item in inventory["sources"]:
        if item["status"] != "contract_passed":
            assert catalog[item["id"]].availability != "available"
    assert (ROOT / "migration/blockers/my.md").exists()
    for item in inventory["historical"]:
        assert item["status"] in {
            "product-confirmed-adapter-pending",
            "adapter-acquisition-verified-science-geometry-blocked",
            "adapter-implemented-upstream-data-blocked",
        }
        assert (ROOT / item["evidence"]).is_file()
        if item["status"] in {
            "adapter-acquisition-verified-science-geometry-blocked",
            "adapter-implemented-upstream-data-blocked",
        }:
            assert (ROOT / item["fixture"]).is_file()
            assert (ROOT / item["adapter"]).is_file()
            assert item.get("migration"), f"{item['id']} has no source migration record"
            assert (ROOT / item["migration"]).is_file()
            if "resource" in item:
                assert (ROOT / item["resource"]).is_file()
            fixture = json.loads((ROOT / item["fixture"]).read_text(encoding="utf-8"))
            assert fixture.get("source") == item["id"]
            if fixture.get("status") == "blocked":
                assert fixture.get("blockers")
                assert fixture.get("evidence")
                assert fixture.get("frames") == []
