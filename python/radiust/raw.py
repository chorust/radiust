"""Runtime-only raw acquisition envelope and ownership helpers."""

from __future__ import annotations

import hashlib
import shutil
from contextlib import suppress
from dataclasses import dataclass
from dataclasses import field as dc_field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .errors import IntegrityError
from .identity import artifact_bytes, safe_ref
from .models import Artifact, FrameRef, Receipt


@dataclass
class RawFrame:
    ref: FrameRef
    artifacts: tuple[Artifact, ...]
    metadata: dict[str, Any] = dc_field(default_factory=dict)
    receipts: tuple[Receipt, ...] = ()
    _closed: bool = False
    _temporary_paths: tuple[Path, ...] = ()
    _temporary_roots: tuple[Path, ...] = ()
    _cache_leases: tuple[Any, ...] = ()

    def __post_init__(self) -> None:
        if len({artifact.name for artifact in self.artifacts}) != len(self.artifacts):
            raise IntegrityError("raw artifact names must be unique")
        for artifact in self.artifacts:
            payload = artifact_bytes(artifact)
            if artifact.size_bytes is not None and artifact.size_bytes != len(payload):
                raise IntegrityError(f"raw artifact size mismatch: {artifact.name}")
            if artifact.sha256 is not None and artifact.sha256 != hashlib.sha256(payload).hexdigest():
                raise IntegrityError(f"raw artifact hash mismatch: {artifact.name}")
        if not self.receipts:
            now = datetime.now(timezone.utc)
            self.receipts = tuple(
                Receipt(
                    artifact.name,
                    len(artifact_bytes(artifact)),
                    hashlib.sha256(artifact_bytes(artifact)).hexdigest(),
                    now,
                    artifact.source_revision,
                )
                for artifact in self.artifacts
            )

    @property
    def closed(self) -> bool:
        return self._closed

    def _ensure_open(self) -> None:
        if self._closed:
            raise RuntimeError("RawFrame is closed")

    def _adopt_temp_root(self, root: Path | None) -> None:
        if root is not None:
            self._temporary_roots = (*self._temporary_roots, root)

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        for path in self._temporary_paths:
            with suppress(OSError):
                path.unlink(missing_ok=True)
        for root in self._temporary_roots:
            with suppress(OSError):
                shutil.rmtree(root)
        for lease in self._cache_leases:
            with suppress(Exception):
                lease.__exit__(None, None, None)

    async def aclose(self) -> None:
        self.close()

    def __enter__(self) -> RawFrame:
        self._ensure_open()
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.close()

    async def __aenter__(self) -> RawFrame:
        self._ensure_open()
        return self

    async def __aexit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        await self.aclose()

    def bytes(self, name: str | None = None) -> bytes:
        self._ensure_open()
        selected = self.artifacts[0] if name is None else next(a for a in self.artifacts if a.name == name)
        return artifact_bytes(selected)


def raw_manifest(raw: RawFrame) -> dict[str, Any]:
    raw._ensure_open()
    return {
        "schema_version": 1,
        "ref": safe_ref(raw.ref),
        "artifacts": [
            {
                "name": receipt.name,
                "role": next(artifact.role for artifact in raw.artifacts if artifact.name == receipt.name),
                "media_type": next(artifact.media_type for artifact in raw.artifacts if artifact.name == receipt.name),
                "size_bytes": receipt.size_bytes,
                "sha256": receipt.sha256,
                "source_revision": receipt.source_revision,
            }
            for receipt in raw.receipts
        ],
        "metadata": dict(raw.metadata),
        "raw_complete": True,
    }
