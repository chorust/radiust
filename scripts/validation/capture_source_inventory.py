#!/usr/bin/env python3
"""Write a deterministic snapshot of the built-in catalog and legal fixtures."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
CATALOG = ROOT / "python/radiust/resources/catalog.json"
FIXTURES = ROOT / "tests/fixtures/sources"
OUTPUT = ROOT / "tests/fixtures/rust-migration/sources.json"
NON_CATALOG_ADAPTERS = ["br_cptec", "br_sipam"]


def _fixture_hashes(source_id: str) -> list[dict[str, Any]]:
    manifest = FIXTURES / source_id / "fixture.json"
    if not manifest.is_file():
        return []
    document = json.loads(manifest.read_text(encoding="utf-8"))
    result: list[dict[str, Any]] = []
    for frame in document.get("frames", []):
        artifacts = []
        for artifact in frame.get("artifacts", []):
            path = manifest.parent / artifact["path"]
            actual = hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else None
            declared = artifact.get("sha256")
            if declared and actual and declared != actual:
                raise ValueError(f"fixture hash mismatch: {path}")
            artifacts.append({
                "name": artifact.get("name"),
                "sha256": declared or actual,
                "present": path.is_file(),
            })
        result.append({
            "product": frame.get("product"),
            "station": frame.get("station"),
            "valid_time": frame.get("valid_time"),
            "revision": frame.get("revision"),
            "artifacts": artifacts,
            "scientific_reference_status": frame.get("metadata", {}).get(
                "scientific_reference_status"
            ),
        })
    return result


def snapshot() -> dict[str, Any]:
    catalog = json.loads(CATALOG.read_text(encoding="utf-8"))
    sources = []
    targets = 0
    for item in catalog["sources"]:
        products = [product["id"] for product in item.get("products", [])]
        stations = item.get("stations", [])
        expanded = []
        if not products:
            expanded.append({"product": None, "station": None})
        for product in item.get("products", []):
            eligible = [
                station["id"] for station in stations
                if not station.get("product_ids")
                or product["id"] in station["product_ids"]
            ]
            if eligible:
                expanded.extend({"product": product["id"], "station": station} for station in eligible)
            else:
                expanded.append({"product": product["id"], "station": None})
        targets += len(expanded)
        sources.append({
            "id": item["id"],
            "availability": item.get("availability", "available"),
            "required_extras": sorted(item.get("required_extras", [])),
            "products": products,
            "targets": expanded,
            "fixtures": _fixture_hashes(item["id"]),
        })
    return {
        "schema_version": 1,
        "catalog_source": "python/radiust/resources/catalog.json",
        "builtin_source_count": len(sources),
        "expanded_target_count": targets,
        "non_catalog_adapter_modules": NON_CATALOG_ADAPTERS,
        "sources": sources,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=OUTPUT)
    args = parser.parse_args()
    data = snapshot()
    if data["builtin_source_count"] != 24 or data["expanded_target_count"] != 26:
        parser.error(
            "catalog changed: expected 24 built-in source IDs and 26 expanded targets; "
            "review the migration scope before updating this snapshot"
        )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(data, ensure_ascii=False, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"captured {data['builtin_source_count']} sources and {data['expanded_target_count']} targets: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
