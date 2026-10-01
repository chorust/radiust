"""Explicit conversions from Rust-owned science objects to optional xarray values."""

from __future__ import annotations

import json
from typing import Any


def to_xarray(value: Any) -> Any:
    """Convert a bound ``RadarField`` or ``RadarDataset`` to xarray on request.

    Rust keeps the source values until this function is called. Conversion
    allocates NumPy arrays and retains the ``uint16`` quality flags alongside
    each field; it is intentionally not part of discovery or raw acquisition.
    """
    try:
        import numpy as np
        import xarray as xr
    except ImportError as exc:  # pragma: no cover - depends on optional install
        raise ImportError("to_xarray() requires NumPy and xarray") from exc

    if callable(getattr(value, "field", None)) and hasattr(value, "field_count"):
        fields = [value.field(index) for index in range(value.field_count)]
        dataset_source = value.source
        dataset_time = value.valid_time
    else:
        document = _document(value)
        fields = document.get("fields") if isinstance(document, dict) else None
        dataset_source = document.get("source", "") if isinstance(document, dict) else ""
        dataset_time = document.get("valid_time", "") if isinstance(document, dict) else ""

    if isinstance(fields, list):
        if not fields:
            raise ValueError("RadarDataset contains no fields")
        field_records = [_field_metadata(field) for field in fields]
        first_grid = field_records[0]["grid"]
        if any(field["grid"] != first_grid for field in field_records[1:]):
            raise ValueError("RadarDataset fields must share one grid for xarray conversion")
        data_vars: dict[str, Any] = {}
        for field, record in zip(fields, field_records, strict=True):
            array, quality, dims, attrs, coords = _field_arrays(field, np)
            data_vars[record["name"]] = (dims, array, attrs)
            data_vars[f"{record['name']}_quality"] = (
                dims,
                quality,
                {
                    "long_name": f"quality flags for {record['name']}",
                    "flag_dtype": "uint16",
                    **_quality_flag_attributes(record, quality, np),
                },
            )
        _, _, _, _, coords = _field_arrays(fields[0], np)
        dataset = xr.Dataset(
            data_vars=data_vars,
            coords=coords,
            attrs={
                "source": dataset_source,
                "valid_time": dataset_time,
            },
        )
        return dataset

    if hasattr(value, "metadata_json"):
        array, quality, dims, attrs, coords = _field_arrays(value, np)
        record = _field_metadata(value)
        coords["quality"] = xr.DataArray(
            quality,
            dims=dims,
            attrs=_quality_flag_attributes(record, quality, np),
        )
        return xr.DataArray(array, dims=dims, coords=coords, attrs=attrs, name=attrs["long_name"])

    document = _document(value)
    if isinstance(document, dict) and {"name", "values", "quality", "shape", "grid"} <= document.keys():
        array, quality, dims, attrs, coords = _field_arrays(document, np)
        coords["quality"] = xr.DataArray(
            quality,
            dims=dims,
            attrs=_quality_flag_attributes(document, quality, np),
        )
        return xr.DataArray(array, dims=dims, coords=coords, attrs=attrs, name=document["name"])

    raise TypeError("to_xarray expects a Rust RadarField or RadarDataset")


def _document(value: Any) -> dict[str, Any]:
    serializer = getattr(value, "to_json", None)
    if not callable(serializer):
        raise TypeError("to_xarray expects a Rust RadarField or RadarDataset")
    try:
        document = json.loads(serializer())
    except (TypeError, ValueError) as exc:
        raise ValueError("Rust science object returned invalid JSON") from exc
    if not isinstance(document, dict):
        raise ValueError("Rust science object returned invalid JSON")
    return document


def _field_metadata(field: Any) -> dict[str, Any]:
    if isinstance(field, dict):
        return field
    try:
        metadata = json.loads(field.metadata_json())
    except (AttributeError, TypeError, ValueError) as exc:
        raise ValueError("Rust RadarDataset contains an invalid field") from exc
    if not isinstance(metadata, dict):
        raise ValueError("Rust RadarDataset contains an invalid field")
    return metadata


def _quality_flag_attributes(record: dict[str, Any], quality: Any, np: Any) -> dict[str, Any]:
    masks = [1, 2, 4, 8, 16, 32]
    meanings = "missing outside_coverage unknown_color recovered interpolated below_detection"
    provenance = record.get("provenance", [])
    has_rdcap_annotation = any(entry == "source=rdcap" for entry in provenance) if isinstance(provenance, list) else False
    if has_rdcap_annotation or np.any(np.asarray(quality, dtype="uint16") & np.uint16(64)):
        masks.append(64)
        meanings += " source_annotation"
    return {"flag_masks": np.asarray(masks, dtype="uint16"), "flag_meanings": meanings}


def _field_arrays(field: Any, np: Any) -> tuple[Any, Any, tuple[str, ...], dict[str, Any], dict[str, Any]]:
    try:
        metadata = _field_metadata(field)
        shape = tuple(int(size) for size in metadata["shape"])
        if hasattr(field, "values_le_bytes") and hasattr(field, "quality_le_bytes"):
            values = np.frombuffer(field.values_le_bytes(), dtype="<f4").reshape(shape).copy()
            quality = np.frombuffer(field.quality_le_bytes(), dtype="<u2").reshape(shape).copy()
        else:
            values = np.asarray(field["values"], dtype=np.float32).reshape(shape)
            quality = np.asarray(field["quality"], dtype=np.uint16).reshape(shape)
        grid = metadata["grid"]
        if len(shape) == 2:
            dims = ("y", "x")
        else:
            dims = tuple(f"dim_{index}" for index in range(len(shape)))
        coords: dict[str, Any] = {}
        for name, dim, size in (("y", dims[-2] if len(dims) >= 2 else "", shape[-2] if len(shape) >= 2 else 0),
                                ("x", dims[-1], shape[-1])):
            values_for_axis = grid.get(name)
            if dim and isinstance(values_for_axis, list) and len(values_for_axis) == size:
                coords[dim] = np.asarray(values_for_axis)
        attrs: dict[str, Any] = {
            "valid_time": metadata.get("valid_time", ""),
            "provenance": json.dumps(metadata.get("provenance", []), sort_keys=True),
            "long_name": metadata["name"],
        }
        if metadata.get("units") is not None:
            attrs["units"] = metadata["units"]
        if grid.get("crs") is not None:
            attrs["crs"] = grid["crs"]
        if grid.get("affine") is not None:
            attrs["affine"] = json.dumps(grid["affine"])
    except (KeyError, TypeError, ValueError, OverflowError) as exc:
        raise ValueError("Rust RadarField shape or grid metadata is invalid") from exc
    return values, quality, dims, attrs, coords


__all__ = ["to_xarray"]
