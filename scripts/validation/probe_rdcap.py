"""Read RDCAP public single-station data and decode its observed CRS grid format.

Research utility; not a radiust source adapter. Requires numpy and Pillow.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import tempfile
import urllib.parse
from pathlib import Path

import numpy as np
from PIL import Image

BASE = "https://rdcap.cwa.gov.tw"


def decode(text: str) -> tuple[np.ndarray, np.ndarray, dict]:
    lines = text.strip().splitlines()
    fields = lines[0].split(",")
    if len(fields) != 11 or fields[2] != "T" or fields[7] != "int16" or fields[10] != "EPSG:4326":
        raise ValueError("Only the verified T/int16/EPSG:4326 format is supported")
    width, height = map(int, fields[:2])
    west, first_lat, last_lon, last_lat = map(float, fields[3:7])
    default, invalid = map(int, fields[8:10])
    transform = re.fullmatch(r"linearTransform\(([-+.\deE]+),([-+.\deE]+)\)", lines[1])
    if not transform:
        raise ValueError("Unsupported value transform")
    scale, offset = map(float, transform.groups())
    legend = np.array([float(v) for v in lines[2].split(",")]).reshape(-1, 4)
    entries = dict(line.split(":", 1) for line in lines[3:])
    nnz = int(entries["ndv"])
    cols = np.array([int(v) for v in entries["colIdx"].split(",") if v], dtype=np.int64)
    ptr = np.array([int(v) for v in entries["rowPtr"].split(",") if v], dtype=np.int64)
    vals = np.array([int(v) for v in entries["vals"].split(",") if v], dtype=np.int64)
    if len(cols) != nnz or len(vals) != nnz or len(ptr) != height + 1:
        raise ValueError("CRS array lengths do not match the header")
    if ptr[0] != 0 or ptr[-1] != nnz or np.any(np.diff(ptr) < 0):
        raise ValueError("Invalid CRS row pointers")
    if np.any((cols < 0) | (cols >= width)) or np.any((vals < -32768) | (vals > 32767)):
        raise ValueError("Column index or int16 value out of bounds")
    raw = np.full((height, width), default, dtype=np.int16)
    for row in range(height):
        begin, end = ptr[row : row + 2]
        row_cols = cols[begin:end]
        if np.any(np.diff(row_cols) <= 0):
            raise ValueError("Unsorted or duplicate CRS columns")
        raw[row, row_cols] = vals[begin:end]
    if first_lat < last_lat:
        raw = raw[::-1].copy()
    dbz = raw.astype(np.float32) * scale + offset
    dbz[raw == invalid] = np.nan
    # In all three observed grids 9999 draws the white range circle/centre mark.
    # This is an empirical annotation classification, not an official sentinel definition.
    dbz[raw == 9999] = np.nan
    # KXD colors use lower-inclusive thresholds; invalid and below-first-bin are transparent.
    color_index = np.searchsorted(legend[:, 3], raw, side="right") - 1
    visible = (raw != invalid) & (color_index >= 0)
    rgba = np.zeros((height, width, 4), dtype=np.uint8)
    rgba[visible, :3] = legend[color_index[visible], :3].astype(np.uint8)
    rgba[visible, 3] = 255
    dx = (last_lon - west) / (width - 1)
    dy = abs(last_lat - first_lat) / (height - 1)
    north = max(first_lat, last_lat)
    meta = {
        "width": width, "height": height, "crs": fields[10], "anchor": "top-left",
        "row_order": "north-to-south", "geotransform": [west, dx, 0, north, 0, -dy],
        "bounds": [west, north - height * dy, west + width * dx, north],
        "default_raw": default, "invalid_raw": invalid, "scale": scale, "offset": offset,
        "stored_values": nnz, "valid_cells": int(np.count_nonzero(raw != invalid)),
        "annotation_raw_candidate": 9999, "annotation_cells": int(np.count_nonzero(raw == 9999)),
        "echo_cells": int(np.isfinite(dbz).sum()),
        "thresholds_dbz": (legend[:, 3] * scale + offset).tolist(),
        "colors": ["#" + "".join(f"{int(c):02x}" for c in rgb) for rgb in legend[:, :3]],
        "min_dbz": float(np.nanmin(dbz)) if np.isfinite(dbz).any() else None,
        "max_dbz": float(np.nanmax(dbz)) if np.isfinite(dbz).any() else None,
    }
    return dbz, rgba, meta


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--country", choices=["TWN", "JPN", "PHL"], default="TWN")
    parser.add_argument("--station", default="RCHL")
    parser.add_argument("--decode-file", type=Path, help="Replay an already saved decoded CSV")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--insecure", action="store_true", help="Allow this research site's untrusted certificate chain")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    timestamp = None
    if args.decode_file:
        text = args.decode_file.read_text()
    else:
        referer = f"{BASE}/data_access/radar_display/{args.country}/{args.station}"
        headers = {"Referer": referer, "User-Agent": "Mozilla/5.0", "Accept": "application/json, text/plain, */*"}
        body = urllib.parse.urlencode({"country": args.country, "radar_name": args.station, "datetime": ""})
        # curl succeeded in this environment where urllib's TLS connection was closed upstream.
        with tempfile.TemporaryDirectory(prefix="rdcap-") as directory:
            cookie = str(Path(directory) / "cookies.txt")

            def read(url: str, extra: list[str]) -> bytes:
                command = ["curl", "--silent", "--show-error", "--fail-with-body", "--compressed", "--max-time", "40", "--cookie", cookie, "--cookie-jar", cookie]
                if args.insecure:
                    command.append("--insecure")
                for key, value in headers.items():
                    command.extend(["--header", f"{key}: {value}"])
                result = subprocess.run(command + extra + [url], capture_output=True)
                if result.returncode:
                    raise RuntimeError(f"curl failed ({result.returncode}): {result.stderr.decode(errors='replace').strip()}")
                return result.stdout

            index_body = read(f"{BASE}/data_access/get_radar_data", ["--header", "X-Requested-With: XMLHttpRequest", "--data", body])
            (args.out / "index.json").write_bytes(index_body)
            index = json.loads(index_body)
            frame = max(index["list"], key=lambda item: int(item["key"]))
            if len(frame["url"]) != 1:
                raise ValueError("Expected a single scalar grid URL")
            url = frame["url"][0]
            parsed = urllib.parse.urlsplit(url)
            if parsed.scheme != "https" or parsed.netloc != "rdcap.cwa.gov.tw" or parsed.path != "/file":
                raise ValueError("Unexpected provider file URL")
            # The first response consumes the observed ticket: persist before any decoding.
            file_body = read(url, [])
        (args.out / "file-response.json").write_bytes(file_body)
        text = json.loads(file_body)
        if not isinstance(text, str) or not text:
            raise ValueError("Empty file response: obtain a fresh index before retrying")
        timestamp = int(frame["key"])
    (args.out / "frame.csv").write_text(text)
    dbz, rgba, meta = decode(text)
    meta["timestamp_ms"] = timestamp
    # Preserve the exact source separately from the annotation-masked scientific view.
    np.savez_compressed(args.out / "frame.npz", dbz=dbz, geotransform=meta["geotransform"])
    Image.fromarray(rgba).save(args.out / "echo.png")
    (args.out / "grid.json").write_text(json.dumps(meta, ensure_ascii=False, indent=2))
    print(json.dumps(meta, ensure_ascii=False))


if __name__ == "__main__":
    main()
