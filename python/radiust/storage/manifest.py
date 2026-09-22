"""Manifest creation and completeness validation."""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from ..errors import IntegrityError
from ..identity import digest

_HEX64 = re.compile(r"^[0-9a-f]{64}$")


def _receipt(path: Path, relative: str, *, role: str, media_type: str) -> dict[str, Any]:
    data = path.read_bytes()
    return {"name": path.name, "relative_uri": relative, "role": role, "media_type": media_type, "size_bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}


@dataclass(frozen=True, slots=True)
class Manifest:
    schema_version: int
    logical_id: str
    revision: str
    output_id: str
    processing_spec: Mapping[str, Any]
    processing_hash: str
    artifacts: tuple[Mapping[str, Any], ...]
    raw_complete: bool
    created_at: str
    generation: str | None = None
    supersedes: Mapping[str, Any] | None = None

    def as_dict(self) -> dict[str, Any]:
        return {
            "schema_version": self.schema_version,
            "logical_id": self.logical_id,
            "revision": self.revision,
            "output_id": self.output_id,
            "processing_spec": dict(self.processing_spec),
            "processing_hash": self.processing_hash,
            "artifacts": [dict(item) for item in self.artifacts],
            "raw_complete": self.raw_complete,
            "created_at": self.created_at,
            "generation": self.generation,
            "supersedes": dict(self.supersedes) if self.supersedes else None,
        }

    def validate(self) -> None:
        if self.schema_version != 1:
            raise IntegrityError(f"unsupported manifest schema: {self.schema_version}")
        for name, value in {
            "logical_id": self.logical_id,
            "revision": self.revision,
            "output_id": self.output_id,
            "processing_hash": self.processing_hash,
        }.items():
            if not _HEX64.fullmatch(value):
                raise IntegrityError(f"manifest {name} must be a full lowercase SHA-256")
        if not isinstance(self.processing_spec, Mapping):
            raise IntegrityError("manifest processing_spec must be a mapping")
        if digest(self.processing_spec) != self.processing_hash:
            raise IntegrityError("manifest processing hash does not match processing_spec")
        try:
            created_at = datetime.fromisoformat(self.created_at.replace("Z", "+00:00"))
        except (AttributeError, ValueError) as exc:
            raise IntegrityError("manifest created_at must be ISO-8601") from exc
        if created_at.tzinfo is None or created_at.utcoffset() is None:
            raise IntegrityError("manifest created_at must include a timezone")
        if not self.artifacts:
            raise IntegrityError("manifest must contain artifacts")
        names = [str(item.get("name")) for item in self.artifacts]
        if len(names) != len(set(names)):
            raise IntegrityError("manifest artifact names must be unique")
        for item in self.artifacts:
            if not isinstance(item, Mapping):
                raise IntegrityError("manifest artifact must be a mapping")
            relative = Path(str(item.get("relative_uri", "")))
            if relative.is_absolute() or ".." in relative.parts:
                raise IntegrityError("manifest artifact path escapes its group")
            if not str(item.get("name", "")) or not str(item.get("relative_uri", "")):
                raise IntegrityError("manifest artifact must have a name and relative_uri")
            try:
                size_bytes = int(item.get("size_bytes", -1))
            except (TypeError, ValueError) as exc:
                raise IntegrityError("manifest artifact size_bytes must be an integer") from exc
            if size_bytes < 0:
                raise IntegrityError("manifest artifact size_bytes must be non-negative")
            if not _HEX64.fullmatch(str(item.get("sha256", ""))):
                raise IntegrityError("manifest artifact sha256 must be full length")
        if self.generation is not None and not self.generation:
            raise IntegrityError("manifest generation cannot be empty")
        if self.supersedes is not None and not isinstance(self.supersedes, Mapping):
            raise IntegrityError("manifest supersedes must be a mapping")


def new_manifest(*, logical_id: str, revision: str, output_id: str, processing_spec: Mapping[str, Any], processing_hash: str, artifacts: list[Mapping[str, Any]], raw_complete: bool, generation: str | None = None, supersedes: Mapping[str, Any] | None = None) -> Manifest:
    manifest = Manifest(1, logical_id, revision, output_id, processing_spec, processing_hash, tuple(artifacts), raw_complete, datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"), generation, supersedes)
    manifest.validate()
    return manifest


def load_manifest(path: Path) -> Manifest:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError) as exc:
        raise IntegrityError(f"invalid manifest: {path}") from exc
    manifest = Manifest(
        schema_version=int(value.get("schema_version", 0)),
        logical_id=str(value.get("logical_id", "")),
        revision=str(value.get("revision", "")),
        output_id=str(value.get("output_id", "")),
        processing_spec=value.get("processing_spec", {}),
        processing_hash=str(value.get("processing_hash", "")),
        artifacts=tuple(value.get("artifacts", [])),
        raw_complete=bool(value.get("raw_complete", False)),
        created_at=str(value.get("created_at", "")),
        generation=value.get("generation"),
        supersedes=value.get("supersedes"),
    )
    manifest.validate()
    return manifest


def inspect_manifest(manifest_path: Path) -> str:
    try:
        manifest = load_manifest(manifest_path)
    except IntegrityError:
        return "incomplete"
    group = manifest_path.parent
    for item in manifest.artifacts:
        target = group / str(item["relative_uri"])
        if not target.is_file():
            return "incomplete"
        data = target.read_bytes()
        if len(data) != int(item["size_bytes"]) or hashlib.sha256(data).hexdigest() != item["sha256"]:
            return "incomplete"
    return "complete"
