from __future__ import annotations

import json
from pathlib import Path

from click.testing import CliRunner
from radiust.cli.main import main

ROOT = Path(__file__).parents[2]
INVENTORY = json.loads((ROOT / "migration/inventory.json").read_text(encoding="utf-8"))


def test_source_cli_list_covers_the_head_inventory() -> None:
    result = CliRunner().invoke(main, ["list", "sources", "--json"])

    assert result.exit_code == 0, result.output
    report = json.loads(result.output)
    catalog_ids = {item["id"] for item in report["items"]}
    inventory_ids = {item["id"] for item in INVENTORY["sources"]}
    assert catalog_ids == inventory_ids
    assert len(catalog_ids) == 24

    fixture_ids = set()
    for item in INVENTORY["sources"]:
        manifest = json.loads((ROOT / item["fixture"]).read_text(encoding="utf-8"))
        assert manifest["source"] == item["id"]
        if manifest.get("status") != "blocked":
            assert manifest["frames"], f"{item['id']} fixture must contain a frame"
            fixture_ids.add(item["id"])

    assert fixture_ids <= catalog_ids
