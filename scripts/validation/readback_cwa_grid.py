"""Offline independent comparison of a saved CWA numeric JSON and its NetCDF export.

Requires NumPy, xarray and h5netcdf. No network requests or credentials are used.
This validates native numerical preservation, not external calibration or coverage.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path

import numpy as np
import xarray as xr


def readback(raw: Path, netcdf: Path) -> dict:
    document = json.loads(raw.read_text())["cwaopendata"]
    assert document["dataid"] == "O-A0059-001"
    dataset = document["dataset"]
    parameters = dataset["datasetInfo"]["parameterSet"]
    height, width = int(parameters["GridDimensionY"]), int(parameters["GridDimensionX"])
    assert parameters["Reflectivity"] == "dBZ"
    assert "TWD67" in dataset["contents"]["contentDescription"]
    values = np.array(dataset["contents"]["content"].split(","), dtype=np.float32)
    assert values.size == height * width and np.isfinite(values).all()
    values = values.reshape(height, width)
    quality = np.zeros(values.shape, dtype=np.uint16)
    quality[values == -99] = 1
    quality[values == -999] = 2
    values[quality != 0] = np.nan
    step = float(parameters["GridResolution"])
    x = float(parameters["StartPointLongitude"]) + np.arange(width) * step
    y = float(parameters["StartPointLatitude"]) + np.arange(height) * step
    valid_time = datetime.fromisoformat(parameters["DateTime"]).astimezone(timezone.utc)
    with xr.open_dataset(netcdf, engine="h5netcdf") as output:
        np.testing.assert_array_equal(output.reflectivity.values, values)
        np.testing.assert_array_equal(output.quality.values, quality)
        np.testing.assert_array_equal(output.x.values, x)
        np.testing.assert_array_equal(output.y.values, y)
        assert output.reflectivity.attrs["units"] == "dBZ"
        assert output.crs.attrs["spatial_ref"] == "EPSG:3821"
        assert datetime.fromisoformat(output.attrs["radiust_valid_time"]) == valid_time
    return {
        "schema_version": 1,
        "status": "passed",
        "evidence_kind": "independent_numpy_xarray_native_numerical_readback",
        "source": "tw",
        "product": "O-A0059-001",
        "valid_time": valid_time.isoformat(),
        "shape": [height, width],
        "cells_compared": values.size,
        "finite_cells": int(np.isfinite(values).sum()),
        "missing_cells": int((quality == 1).sum()),
        "outside_or_quality_removed_cells": int((quality == 2).sum()),
        "native_crs": "EPSG:3821",
        "units": "dBZ",
        "values_quality_coordinates_exact": True,
        "raw_sha256": hashlib.sha256(raw.read_bytes()).hexdigest(),
        "netcdf_sha256": hashlib.sha256(netcdf.read_bytes()).hexdigest(),
        "unverified": [
            "external physical calibration",
            "TWD67 to WGS84 conversion",
            "image product mapping/geometry",
            "real coverage and default source admission",
            "other times and three-entry numerical replay",
        ],
        "raw_data_distribution": False,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("raw", type=Path)
    parser.add_argument("netcdf", type=Path)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    report = readback(args.raw, args.netcdf)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "cells_compared": report["cells_compared"]}))
