"""Safe, deterministic local and object-store names."""

from __future__ import annotations

from datetime import timezone
from pathlib import Path

from ..errors import OutputConflict
from ..identity import variant_id
from ..models import FrameRef, ProcessingSpec, validate_identifier

ALLOWED_TEMPLATE_FIELDS = {"source", "product", "station", "valid_time", "base_time", "date", "hour", "variant_id", "ext"}


def path_values(ref: FrameRef, output_id: str, spec: ProcessingSpec) -> dict[str, str]:
    valid = ref.valid_time.astimezone(timezone.utc)
    return {
        "source": validate_identifier(ref.source, "source"),
        "product": validate_identifier(ref.product, "product"),
        "station": validate_identifier(ref.station or "composite", "station"),
        "valid_time": valid.strftime("%Y%m%dT%H%M%S") + (f"{valid.microsecond:06d}" if valid.microsecond else "") + "Z",
        "base_time": ref.base_time.astimezone(timezone.utc).strftime("%Y%m%dT%H%M%SZ") if ref.base_time else "",
        "date": valid.strftime("%Y-%m-%d"),
        "hour": valid.strftime("%H"),
        "variant_id": variant_id(output_id),
        "ext": "nc" if spec.format == "netcdf" else spec.format.replace("geotiff", "tif"),
    }


def render_template(template: str, values: dict[str, str]) -> str:
    fields: set[str] = set()
    class SafeDict(dict[str, str]):
        def __missing__(self, key: str) -> str:
            fields.add(key)
            return ""
    try:
        rendered = template.format_map(SafeDict(values))
    except (KeyError, ValueError) as exc:
        raise OutputConflict(f"invalid output template: {exc}") from exc
    if fields or any(field not in ALLOWED_TEMPLATE_FIELDS for field in fields):
        raise OutputConflict("output template contains an unsupported field")
    if not rendered or Path(rendered).is_absolute() or ".." in Path(rendered).parts:
        raise OutputConflict("output template must stay within output root")
    return rendered


def output_stem(ref: FrameRef, output_id: str, spec: ProcessingSpec) -> str:
    values = path_values(ref, output_id, spec)
    stem = f"{values['station']}_{values['valid_time']}"
    if values["base_time"]:
        stem += f"_base-{values['base_time']}"
    return f"{stem}_{values['variant_id']}.{values['ext']}"


def output_location(root: Path, ref: FrameRef, output_id: str, spec: ProcessingSpec, template: str | None = None) -> Path:
    relative = output_relative_location(ref, output_id, spec, template=template)
    candidate = (root / relative).resolve()
    if candidate != root.resolve() and root.resolve() not in candidate.parents:
        raise OutputConflict("output path escapes output root")
    return candidate


def output_relative_location(ref: FrameRef, output_id: str, spec: ProcessingSpec, template: str | None = None) -> str:
    values = path_values(ref, output_id, spec)
    relative = render_template(template, values) if template else f"source={values['source']}/product={values['product']}/date={values['date']}/hour={values['hour']}/{output_stem(ref, output_id, spec)}"
    return relative
