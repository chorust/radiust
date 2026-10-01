import hashlib
import json
from pathlib import Path

from radiust._bridge import native_source_catalog
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
    "rdcap": False,
}


def _json(path: Path):
    return json.loads(path.read_text(encoding="utf-8"))


def _assert_fixture_hashes(manifest_path: Path, fixture: dict) -> None:
    for frame in fixture.get("frames", []):
        for artifact in frame.get("artifacts", []):
            raw_path = manifest_path.parent / artifact["path"]
            payload = raw_path.read_bytes()
            if "size_bytes" in artifact:
                assert len(payload) == artifact["size_bytes"]
            assert hashlib.sha256(payload).hexdigest() == artifact["sha256"]


def test_display_migration_inventory_keeps_all_current_source_statuses_and_paths_separate():
    """Structural coverage is necessary but never equivalent to golden acceptance."""
    display = _json(ROOT / "migration/legacy-display-inventory.json")
    acquisition = _json(ROOT / "migration/inventory.json")
    catalog = _json(ROOT / "python/radiust/resources/catalog.json")
    products = {
        source["id"]: {product["id"] for product in source["products"]}
        for source in catalog["sources"]
    }
    baseline_status = {item["id"]: item["status"] for item in acquisition["sources"]}
    assert display["coverage_status"].startswith("partial:")
    assert sum(item["status"] == "passed" for item in display["paths"]) == 15
    assert sum(item["status"] == "difference_pending" for item in display["paths"]) == 0
    assert sum(item["status"] == "blocked" for item in display["paths"]) == 8
    assert "WU" not in display["source_aliases"]
    assert set(display["excluded_display_sources"]) == {"opensnow", "wunderground"}
    assert len(display["paths"]) == 23
    assert len({entry["path_id"] for entry in display["paths"]}) == 23
    assert len({entry["source"] for entry in display["paths"]}) == 20
    for row in display["paths"]:
        source, product, path_id = row["source"], row["product"], row["path_id"]
        assert source in baseline_status and product in products[source]
        assert path_id == f"{source}/{product}" or path_id.startswith(f"{source}/{product}/")
        assert row["status"] in {"passed", "difference_pending", "blocked"}
        assert row["scientific_status_unchanged"] is True
        migration = _json(ROOT / "migration/sources" / f"{source}.json")
        related = {entry["path_id"]: entry for entry in migration["display_migration"]["paths"]}
        assert migration["source"] == source
        assert isinstance(migration["status"], str) and migration["status"]
        assert related[path_id]["status"] == row["status"]
        assert related[path_id]["scientific_status_unchanged"] is True
        if row["status"] == "passed":
            assert not row["blocked_reasons"]
            assert row["old_config_verified"] and row["source_baseline_verified"]
            assert related[path_id]["input_hashes"] and related[path_id]["baseline_hash"]
        elif row["status"] == "blocked":
            assert row["blocked_reasons"]
            assert related[path_id]["input_hashes"] == []
            assert related[path_id]["baseline_hash"] is None
        else:
            assert not row["blocked_reasons"]
            assert related[path_id]["input_hashes"]
            assert related[path_id]["baseline_hash"]
            assert not row["source_baseline_verified"]


def test_head_inventory_is_backed_by_the_rust_catalog_and_python_metadata_facade():
    inventory = _json(ROOT / "migration/inventory.json")
    native = native_source_catalog()
    native_sources = {item["id"]: item for item in native["sources"]}
    facade_sources = {item.id: item for item in sources()}
    inventory_ids = {item["id"] for item in inventory["sources"]}

    assert len(inventory["sources"]) == 24
    assert inventory_ids | {"rdcap"} == set(native_sources) == set(facade_sources)
    for item in inventory["sources"]:
        source_id = item["id"]
        native_info = native_sources[source_id]
        facade_info = registry.get_info(source_id)
        resource_path = ROOT / "python/radiust/resources/sources" / f"{source_id}.json"
        fixture_path = ROOT / item["fixture"]
        migration_path = ROOT / item["migration"]
        resource = _json(resource_path)
        fixture = _json(fixture_path)
        migration = _json(migration_path)

        assert resource.get("schema_version") == 1
        assert resource.get("source") == source_id
        assert resource.get("adapter_version") == native_info.get("adapter_version")
        assert facade_info.adapter_version == native_info.get("adapter_version")
        assert fixture.get("source") == source_id
        assert migration.get("source") == source_id
        assert migration.get("adapter") == f"python/radiust/sources/{source_id.replace('-', '_')}.py"
        assert not (ROOT / migration["adapter"]).exists(), source_id
        assert facade_info.availability == native_info["availability"]


def test_catalog_resources_keep_historical_capability_without_python_adapter_classes():
    native = {item["id"]: item for item in native_source_catalog()["sources"]}
    assert set(native) == set(HEAD_HISTORICAL_CAPABILITY)
    for source_id, expected in HEAD_HISTORICAL_CAPABILITY.items():
        if source_id == "rdcap":
            products = {item["id"]: item for item in native[source_id]["products"]}
            assert products and all(product["historical"] is expected for product in products.values())
            continue
        resource = _json(ROOT / "python/radiust/resources/sources" / f"{source_id}.json")
        assert resource.get("historical") is expected, source_id
        products = {item["id"]: item for item in native[source_id]["products"]}
        assert products
        assert all(product["historical"] is expected for product in products.values()), source_id


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


def test_fixture_manifests_are_hashed_or_explicitly_blocked():
    inventory = _json(ROOT / "migration/inventory.json")
    for item in inventory["sources"]:
        fixture_path = ROOT / item["fixture"]
        fixture = _json(fixture_path)
        assert fixture.get("schema_version") == 1
        assert fixture.get("source") == item["id"]
        if fixture.get("status") == "blocked":
            assert fixture.get("blockers")
            assert fixture.get("evidence")
            assert fixture.get("frames") == []
        else:
            assert fixture.get("frames")
            _assert_fixture_hashes(fixture_path, fixture)


def test_historical_provenance_survives_without_requiring_python_adapters():
    inventory = _json(ROOT / "migration/inventory.json")
    catalog_ids = {item["id"] for item in native_source_catalog()["sources"]}
    for item in inventory["historical"]:
        source_id = item["id"]
        history_path = ROOT / item["evidence"]
        history = _json(history_path)
        fixture_path = ROOT / item["fixture"]
        fixture = _json(fixture_path)
        migration = _json(ROOT / item["migration"])
        assert item["status"] == "adapter-acquisition-verified-science-geometry-blocked"
        assert source_id not in catalog_ids
        assert history["source"] == source_id == fixture["source"] == migration["source"]
        assert history["fixture"] == item["fixture"]
        assert history["migration"] == item["migration"]
        assert history["adapter"] == migration["adapter"]
        assert history["resource"] == migration["resource"]
        assert isinstance(history.get("evidence"), list) and history["evidence"]
        assert not (ROOT / history["adapter"]).exists()
        assert (ROOT / history["resource"]).is_file()
        assert (ROOT / history["test"]).is_file()
        assert fixture.get("frames")
        _assert_fixture_hashes(fixture_path, fixture)


def test_unverified_sources_are_not_advertised_as_available():
    inventory = _json(ROOT / "migration/inventory.json")
    catalog = {item["id"]: item for item in native_source_catalog()["sources"]}

    for item in inventory["sources"]:
        if item["status"] != "contract_passed":
            assert catalog[item["id"]]["availability"] != "available"
    assert (ROOT / "migration/blockers/my.md").exists()
    for item in inventory["historical"]:
        assert item["status"] == "adapter-acquisition-verified-science-geometry-blocked"
        assert (ROOT / item["evidence"]).is_file()
        assert (ROOT / item["fixture"]).is_file()
        fixture = _json(ROOT / item["fixture"])
        assert fixture.get("source") == item["id"]
        assert fixture.get("frames")
