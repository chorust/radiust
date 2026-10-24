"""Opt-in, bounded CLI/sync/async CWA raw acquisition and independent PNG readback.

Requires a rebuilt native extension, Pillow, and the user's own sources.tw.api_key.
Raw data stays in --output; only safe frame facts and receipts enter --report.
"""

from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

import radiust
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]


def readback(manifest: Path) -> dict:
    document = json.loads(manifest.read_text())
    frame = document["ref"]
    if frame["source"] != "tw" or frame["product"] != "observation-wide":
        raise ValueError("CWA source or product mismatch")
    if "Authorization" in manifest.read_text():
        raise ValueError("CWA manifest contains authorization data")
    if document["raw_complete"] is not True:
        raise ValueError("CWA raw acquisition is incomplete")
    if {a["name"] for a in document["artifacts"]} != {"O-A0058-001.png", "O-A0058-001.json"}:
        raise ValueError("CWA artifact membership mismatch")
    for receipt in document["artifacts"]:
        path = manifest.parent / "raw" / receipt["name"]
        payload = path.read_bytes()
        if len(payload) != receipt["size_bytes"]:
            raise ValueError("CWA artifact size mismatch")
        if hashlib.sha256(payload).hexdigest() != receipt["sha256"]:
            raise ValueError("CWA artifact digest mismatch")
    metadata = json.loads((manifest.parent / "raw/O-A0058-001.json").read_text())["cwaopendata"]
    if metadata["dataid"] != "O-A0058-001":
        raise ValueError("CWA metadata dataset mismatch")
    valid_time = datetime.fromisoformat(metadata["dataset"]["DateTime"])
    if valid_time != datetime.fromisoformat(frame["valid_time"].replace("Z", "+00:00")):
        raise ValueError("CWA metadata valid time mismatch")
    image_path = manifest.parent / "raw/O-A0058-001.png"
    with Image.open(image_path) as image:
        image.verify()
    with Image.open(image_path) as image:
        image.load()
        shape = [image.height, image.width]
    if shape != frame["locator"]["provider_image_shape"]:
        raise ValueError("CWA provider image shape mismatch")
    return {
        "status": "passed",
        "logical_id": frame["logical_id"],
        "valid_time": frame["valid_time"],
        "shape": shape,
        "artifacts": document["artifacts"],
        "full_png_decode_and_receipts": "passed",
        "geometry_status": frame["locator"]["geometry_status"],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--cli", type=Path, default=ROOT / "target/debug/radiust")
    args = parser.parse_args()
    if not args.live:
        parser.error("--live is required; no provider requests are made without this opt-in")
    args.output.mkdir(parents=True, exist_ok=True)
    query = radiust.Query("tw", product="observation-wide", latest=True)
    config = {
        "runtime": {"allow_network": True},
        "cache": {"enabled": False, "dir": str(args.output / "cache")},
        "storage": {"output": str(args.output / "sdk-output")},
    }
    env = {
        **os.environ,
        "RADIUST_RUNTIME__ALLOW_NETWORK": "true",
        "RADIUST_CACHE__ENABLED": "false",
    }
    started = time.monotonic()
    command = [
        str(args.cli),
        "download",
        "tw",
        "--product",
        "observation-wide",
        "--latest",
        "--raw-only",
        "--no-cache",
        "--output",
        str(args.output / "cli"),
        "--json",
    ]
    result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=180)
    if result.returncode:
        raise RuntimeError("CWA CLI acquisition failed; inspect its safe command report separately")
    cli_report = json.loads(result.stdout)
    if cli_report["counts"]["failed"] != 0 or len(cli_report["items"]) != 1:
        raise ValueError("CWA CLI acquisition report failed acceptance")
    cli = readback(Path(cli_report["items"][0]["output_uri"]))
    with radiust.Client(config=config) as client:
        sync_report = client.download(query, output=args.output / "sync", raw_only=True)
        if len(sync_report.items) != 1 or sync_report.items[0].status not in {"written", "skipped"}:
            raise ValueError("CWA sync acquisition report failed acceptance")
        sync = readback(Path(sync_report.items[0].output_uri))

    async def asynchronous() -> dict:
        async with radiust.AsyncClient(config=config) as client:
            report = await client.download(query, output=args.output / "async", raw_only=True)
            if len(report.items) != 1 or report.items[0].status not in {"written", "skipped"}:
                raise ValueError("CWA async acquisition report failed acceptance")
            return readback(Path(report.items[0].output_uri))

    asynchronous_result = asyncio.run(asynchronous())
    entries = {"cli": cli, "sync_sdk": sync, "async_sdk": asynchronous_result}
    same_frame = len({entry["logical_id"] for entry in entries.values()}) == 1
    receipts_equal = cli["artifacts"] == sync["artifacts"] == asynchronous_result["artifacts"]
    report = {
        "schema_version": 1,
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "source": "tw",
        "product": "observation-wide",
        "dataset_id": "O-A0058-001",
        "metadata_endpoint": "https://opendata.cwa.gov.tw/fileapi/v1/opendataapi/O-A0058-001",
        "evidence_kind": "agent_executed_live_native_cli_sdk_and_independent_pillow_readback",
        "duration_seconds": round(time.monotonic() - started, 3),
        "entries": entries,
        "same_frame": same_frame,
        "receipts_equal": receipts_equal,
        "same_frame_three_entry_acceptance": "passed"
        if same_frame and receipts_equal
        else "not_passed",
        "scientific_admission": False,
        "unverified": [
            "pixel registration/datum",
            "exact RGB-to-dBZ mapping and annotation masks",
            "mutable PNG/metadata same-version binding",
            "real coverage",
            "default source admission",
        ],
        "raw_data_distribution": False,
    }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(
        json.dumps(
            {
                "entries_passed": len(entries),
                "same_frame": same_frame,
                "receipts_equal": receipts_equal,
            }
        )
    )


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        # Avoid printing a third-party exception that might contain a credentialed URL.
        print(json.dumps({"status": "failed", "error_type": type(error).__name__}))
        raise SystemExit(1) from None
