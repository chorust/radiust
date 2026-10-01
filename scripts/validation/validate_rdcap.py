#!/usr/bin/env python3
"""Run repeatable RDCAP offline contracts or a pinned live SDK validation.

Offline mode never accesses the network. Live mode requires an explicit config
whose runtime.allow_network value is true and uses the standard Rust-backed SDK.
The summary contains safe frame identity and content digests, never locators.
"""

from __future__ import annotations

import argparse
import importlib
import json
import os
import re
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
STATIONS = ("TWN/RCHL", "JPN/ISHI", "PHL/SUBI")
FORMATS = ("png", "netcdf", "geotiff", "zarr")
READBACK_MODULES = ("numpy", "PIL", "xarray", "h5netcdf", "rasterio", "zarr")


def _safe_text(value: str) -> str:
    value = re.sub(r"https?://\S+", "<url-redacted>", value)
    value = re.sub(r"(?i)(ft|ticket|token|credential)=([^&\s]+)", r"\1=<redacted>", value)
    value = re.sub(r"(?i)(authorization:\s*bearer\s+)\S+", r"\1<redacted>", value)
    return value[-4000:]


def _run(command: list[str], *, env: dict[str, str] | None = None, timeout: int = 300) -> dict[str, Any]:
    started = time.monotonic()
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {
            "status": "failed",
            "exit_code": None,
            "duration_seconds": round(time.monotonic() - started, 3),
            "detail": _safe_text(str(exc)),
        }
    output = result.stdout + "\n" + result.stderr
    return {
        "status": "passed" if result.returncode == 0 else "failed",
        "exit_code": result.returncode,
        "duration_seconds": round(time.monotonic() - started, 3),
        "detail": _safe_text(output) if result.returncode else None,
    }


def _preflight() -> dict[str, str]:
    missing: dict[str, str] = {}
    for module in READBACK_MODULES:
        try:
            importlib.import_module(module)
        except ImportError as exc:
            missing[module] = str(exc)
    return missing


def _offline(out: Path) -> dict[str, Any]:
    missing = _preflight()
    if missing:
        return {
            "status": "not_verified",
            "reason": "independent readback dependencies are missing",
            "missing_dependencies": sorted(missing),
            "steps": {},
        }

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    formats_out = out / f"rust-formats-{stamp}"
    formats_out.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env["RADIUST_RDCAP_FORMATS_OUT"] = str(formats_out)
    rust = _run(
        ["cargo", "test", "-p", "radiust-core", "--test", "rdcap_contract"],
        env=env,
        timeout=600,
    )
    python = _run(
        [
            "uv", "run", "--locked", "--group", "ci", "--group", "dev",
            "--extra", "geotiff", "--extra", "zarr", "--",
            "python", "-m", "pytest", "-q", "tests/test_rdcap_formats.py",
        ],
        env={**env, "RADIUST_RDCAP_FORMATS_OUT": str(formats_out)},
        timeout=600,
    )
    steps = {"rust_rdcap_contract": rust, "python_independent_readback": python}
    passed = all(step["status"] == "passed" for step in steps.values())
    return {
        "status": "passed" if passed else "failed",
        "formats_output": str(formats_out),
        "steps": steps,
        "evidence_scope": "offline reconstructed content fixtures; not live acquisition evidence",
    }


def _numpy_field(field: Any) -> tuple[Any, Any, dict[str, Any]]:
    import numpy as np

    metadata = json.loads(field.metadata_json())
    values = np.asarray(field.copy_values(), dtype=np.float32).reshape(field.shape)
    quality = np.frombuffer(field.quality_le_bytes(), dtype="<u2").reshape(field.shape)
    return values, quality, metadata


def _assert_equal(expected: Any, actual: Any, label: str) -> None:
    import numpy as np

    if expected.shape != actual.shape:
        raise AssertionError(f"{label} dimensions differ")
    if not np.array_equal(expected, actual, equal_nan=True):
        if expected.dtype.kind == "f" and actual.dtype.kind == "f":
            if not np.allclose(expected, actual, rtol=0, atol=1e-5, equal_nan=True):
                raise AssertionError(f"{label} values differ")
        else:
            raise AssertionError(f"{label} values differ")


def _readback(fmt: str, path: Path, values: Any, quality: Any, metadata: dict[str, Any]) -> dict[str, Any]:
    import numpy as np

    if fmt == "png":
        from PIL import Image

        image = Image.open(path).convert("RGBA")
        if (image.height, image.width) != values.shape:
            raise AssertionError("PNG dimensions differ from decoded field")
        sidecars = sorted(path.parent.glob("*.render.json"))
        if not sidecars:
            raise AssertionError("PNG render sidecar is missing")
        sidecar = json.loads(sidecars[0].read_text())
        if sidecar.get("palette", {}).get("id") != "rdcap-reflectivity-v1":
            raise AssertionError("PNG palette identity is missing")
        rgba = np.asarray(image)
        if np.any(rgba[..., 3][~np.isfinite(values)] != 0):
            raise AssertionError("missing PNG cells are not transparent")
        return {"status": "passed", "shape": list(values.shape), "palette_id": sidecar["palette"]["id"]}

    if fmt == "netcdf":
        import xarray as xr

        with xr.open_dataset(path, engine="h5netcdf", decode_times=False) as dataset:
            _assert_equal(values, np.asarray(dataset["reflectivity"].values), "NetCDF values")
            _assert_equal(quality, np.asarray(dataset["quality"].values), "NetCDF quality")
            masks = np.asarray(dataset["quality"].attrs.get("flag_masks", []), dtype=np.uint16)
            if not np.array_equal(masks, np.asarray([1, 2, 4, 8, 16, 32, 64], dtype=np.uint16)):
                raise AssertionError("NetCDF quality flag masks differ")
        return {"status": "passed", "shape": list(values.shape), "quality_bit_64": bool(np.any(quality & 64))}

    if fmt == "geotiff":
        import rasterio

        quality_path = path.with_name(path.stem + "_quality.tif")
        with rasterio.open(path) as dataset:
            actual_values = dataset.read(1).astype(np.float32)
            if dataset.crs is None or dataset.crs.to_epsg() != 4326:
                raise AssertionError("GeoTIFF CRS is not EPSG:4326")
        with rasterio.open(quality_path) as dataset:
            actual_quality = dataset.read(1).astype(np.uint16)
        _assert_equal(values, actual_values, "GeoTIFF values")
        _assert_equal(quality, actual_quality, "GeoTIFF quality")
        provenance = json.loads(path.with_name(path.stem + "_provenance.json").read_text())
        if provenance.get("quality_flag_masks") != [1, 2, 4, 8, 16, 32, 64]:
            raise AssertionError("GeoTIFF quality masks differ")
        return {"status": "passed", "shape": list(values.shape), "crs": "EPSG:4326"}

    if fmt == "zarr":
        import xarray as xr

        with xr.open_zarr(path, consolidated=False) as dataset:
            _assert_equal(values, np.asarray(dataset["reflectivity"].values), "Zarr values")
            _assert_equal(quality, np.asarray(dataset["quality"].values), "Zarr quality")
            masks = np.asarray(dataset["quality"].attrs.get("flag_masks", []), dtype=np.uint16)
            if not np.array_equal(masks, np.asarray([1, 2, 4, 8, 16, 32, 64], dtype=np.uint16)):
                raise AssertionError("Zarr quality flag masks differ")
        return {"status": "passed", "shape": list(values.shape), "crs": metadata["grid"].get("crs")}
    raise ValueError(f"unsupported format: {fmt}")


def _live(out: Path, conf: Path | None) -> dict[str, Any]:
    if conf is None:
        return {"status": "not_verified", "reason": "live mode requires --conf with runtime.allow_network: true"}
    try:
        import radiust
        from radiust.config import load_config
    except ImportError as exc:
        return {"status": "not_verified", "reason": f"Rust-backed radiust SDK unavailable: {exc}"}
    try:
        config = load_config(path=conf)
    except Exception as exc:
        return {"status": "failed", "reason": _safe_text(str(exc))}
    if config.values.get("runtime", {}).get("allow_network") is not True:
        return {"status": "not_verified", "reason": "configuration does not explicitly enable runtime.allow_network"}

    import numpy as np

    station_results: list[dict[str, Any]] = []
    try:
        with radiust.Client(config=config) as client:
            for station in STATIONS:
                station_dir = out / station.replace("/", "-")
                station_dir.mkdir(parents=True, exist_ok=False)
                report = client.discover_report(radiust.Query("rdcap", stations=(station,), latest=True))
                candidates = [index for index, item in enumerate(report.items) if item.status == "success"]
                if len(candidates) != 1:
                    discovery_errors = []
                    for discovery_item in report.items:
                        if discovery_item.error:
                            discovery_errors.append({
                                "station": discovery_item.target.station,
                                "code": discovery_item.error.get("code"),
                                "stage": discovery_item.error.get("stage"),
                                "retryable": discovery_item.error.get("retryable"),
                                "message": _safe_text(str(discovery_item.error.get("message", ""))),
                            })
                    station_results.append({
                        "station": station,
                        "status": "failed",
                        "discovery_statuses": [item.status for item in report.items],
                        "discovery_errors": discovery_errors,
                        "reason": "standard SDK did not return exactly one current frame",
                    })
                    continue
                index = candidates[0]
                item = report.items[index]
                ref = report.frame(index)
                raw_report = client.download(ref, output=station_dir / "raw", raw_only=True)
                if len(raw_report.items) != 1 or raw_report.items[0].status not in {"written", "skipped"}:
                    raise RuntimeError(f"raw acquisition failed for {station}")
                manifest_path = Path(raw_report.items[0].output_uri or "")
                if not manifest_path.is_file():
                    raise RuntimeError(f"raw manifest missing for {station}")
                raw_manifest = json.loads(manifest_path.read_text())
                raw_hashes = {entry["name"]: entry["sha256"] for entry in raw_manifest.get("artifacts", [])}
                if not raw_hashes:
                    raise RuntimeError(f"raw manifest has no content digest for {station}")
                field = client.replay_raw_manifest(manifest_path)
                values, quality, metadata = _numpy_field(field)
                output_results: dict[str, Any] = {}
                for fmt in FORMATS:
                    fmt_dir = station_dir / fmt
                    result = client.write(field, output=fmt_dir, format=fmt, ref=ref)
                    if len(result.items) != 1 or result.items[0].status not in {"written", "skipped"}:
                        raise RuntimeError(f"{fmt} writer failed for {station}")
                    artifact_path = Path(result.items[0].output_uri or "")
                    if not artifact_path.exists():
                        raise RuntimeError(f"{fmt} output is missing for {station}")
                    output_results[fmt] = _readback(fmt, artifact_path, values, quality, metadata)
                    repeated = client.write(field, output=fmt_dir, format=fmt, ref=ref)
                    if len(repeated.items) != 1 or repeated.items[0].status != "skipped":
                        raise RuntimeError(f"{fmt} repeated write was not skipped for {station}")
                station_results.append({
                    "station": station,
                    "status": "passed",
                    "valid_time": item.valid_time,
                    "logical_id": ref.logical_id,
                    "shape": list(values.shape),
                    "crs": metadata["grid"].get("crs"),
                    "raw_artifact_sha256": raw_hashes,
                    "quality_65_count": int(np.count_nonzero(quality == 65)),
                    "finite_weak_value_count": int(np.count_nonzero(np.isfinite(values) & (values < 5))),
                    "formats": output_results,
                })
    except Exception as exc:
        return {
            "status": "failed",
            "reason": _safe_text(f"{type(exc).__name__}: {exc}"),
            "stations": station_results,
            "online_security": "standard rust-backed SDK; system TLS validation; no insecure override",
        }
    passed = len(station_results) == 3 and all(item["status"] == "passed" for item in station_results)
    return {
        "status": "passed" if passed else "failed",
        "stations": station_results,
        "online_security": "standard rust-backed SDK; system TLS validation; no insecure override",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("offline", "live"), required=True)
    parser.add_argument("--conf", type=Path, help="explicit config; required for live mode")
    parser.add_argument("--out", type=Path, required=True, help="directory for safe validation evidence")
    args = parser.parse_args()
    out = args.out.expanduser().resolve()
    out.mkdir(parents=True, exist_ok=True)
    summary: dict[str, Any] = {
        "schema_version": 1,
        "mode": args.mode,
        "run_at": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
        "radiust_version": "0.1.0",
        "python_version": sys.version.split()[0],
    }
    summary["result"] = _offline(out) if args.mode == "offline" else _live(out, args.conf)
    summary_path = out / "summary.json"
    summary_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"summary": str(summary_path), "status": summary["result"]["status"]}, ensure_ascii=False))
    return 0 if summary["result"]["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
