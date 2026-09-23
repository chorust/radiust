"""Killable source discovery and acquisition for the raw preview CLI."""

from __future__ import annotations

import multiprocessing as mp
import os
import shutil
import time
from collections.abc import Callable
from contextlib import suppress
from pathlib import Path
from typing import Any

from PIL import Image

from ..config import EffectiveConfig
from ..discovery import _recv_until_deadline, _stop_worker, _worker_temp_root
from ..discovery_worker import send_message
from ..models import Query
from .models import RawPreview


class RawPreviewError(RuntimeError):
    """The isolated raw preview worker could not complete its request."""


class RawPreviewAmbiguousError(RawPreviewError):
    def __init__(self, candidates: list[dict[str, Any]]) -> None:
        self.candidates = candidates
        super().__init__(f"raw preview requires a unique frame; {len(candidates)} candidates")


def _isolated_preview_worker(
    pipe: Any, query: Query, config: EffectiveConfig, root: str, apply_legacy: bool = False,
) -> None:
    if os.name == "posix":
        os.setsid()
    try:
        _preview_worker(pipe, query, config, Path(root), apply_legacy=apply_legacy)
    finally:
        pipe.close()


def _preview_worker(
    pipe: Any, query: Query, config: EffectiveConfig, root: Path, *, apply_legacy: bool = False,
) -> None:
    from ..cli.safety import safe_text
    from ..client import Client
    from ..display.raw import preview_raw

    try:
        with Client(config=config) as client:
            refs = client.discover(query)
            send_message(pipe, {"kind": "progress", "stage": "discover", "completed": len(refs), "total": len(refs)})
            if not refs:
                send_message(pipe, {"kind": "error", "message": "source returned no matching frame"})
                return
            if len(refs) > 1:
                candidates = [
                    {
                        "product": ref.product,
                        "station": ref.station,
                        "valid_time": ref.valid_time.isoformat().replace("+00:00", "Z"),
                    }
                    for ref in refs[:12]
                ]
                send_message(pipe, {"kind": "ambiguous", "candidates": candidates, "total": len(refs)})
                return

            ref = refs[0]
            send_message(pipe, {"kind": "progress", "stage": "acquire", "completed": 0, "total": 1})
            with client.acquire(ref) as raw:
                if raw.ref != ref:
                    raise ValueError("acquired raw frame identity differs from selected discovery frame")
                preview = preview_raw(
                    raw, limits=client.config.values["runtime"], apply_legacy=apply_legacy,
                )
                preview_path = root / "preview.png"
                Image.fromarray(preview.rgba, mode="RGBA").save(preview_path, format="PNG")
                if preview_path.stat().st_size > int(client.config.values["runtime"]["max_temp_bytes"]):
                    raise ValueError("raw preview exceeds temporary byte limit")
                message = {
                    "kind": "preview",
                    "file_identity": preview.file_identity,
                    "format": preview.format,
                    "sha256": preview.sha256,
                    "source": preview.source,
                    "product": preview.product,
                    "station": preview.station,
                    "valid_time": preview.valid_time,
                    "frame_index": preview.frame_index,
                    "display_mode": preview.display_mode,
                    "rule_version": preview.rule_version,
                    "reason": preview.reason,
                    "width": preview.width,
                    "height": preview.height,
                }
        send_message(pipe, message)
    except KeyboardInterrupt:
        raise
    except Exception as exc:
        with suppress(Exception):
            send_message(pipe, {"kind": "error", "message": safe_text(exc)})


def preview_source_raw(
    query: Query,
    config: EffectiveConfig,
    *,
    progress: Callable[[str, int, int | None], None] | None = None,
    worker: Callable[[Any, Query, EffectiveConfig, str, bool], None] | None = None,
    apply_legacy: bool = False,
) -> RawPreview:
    """Return a raw preview while keeping blocked source I/O killable by the CLI."""
    ctx = mp.get_context("spawn")
    runtime = config.values["runtime"]
    deadline = time.monotonic() + float(runtime["frame_deadline"])
    root: Path | None = None
    owns_configured_parent = False
    parent = child = None
    process: mp.Process | None = None
    try:
        root, owns_configured_parent = _worker_temp_root(config)
        values = dict(config.values)
        values["sources"] = {query.source: config.values.get("sources", {}).get(query.source, {})}
        values["runtime"] = {**runtime, "temp_root": str(root)}
        values["cache"] = {**config.values["cache"], "enabled": False}
        values["storage"] = {
            **config.values["storage"], "access_key": None, "secret_key": None, "endpoint": None,
        }
        worker_config = EffectiveConfig(values, config.origins)
        parent, child = ctx.Pipe(duplex=False)
        process = ctx.Process(
            target=_isolated_preview_worker if worker is None else _isolated_preview_worker_with,
            args=(child, query, worker_config, str(root), apply_legacy) if worker is None
            else (child, worker, query, worker_config, str(root), apply_legacy),
            daemon=True,
        )
        try:
            process.start()
        except Exception as exc:
            process = None
            raise RawPreviewError("raw preview worker could not start") from exc
        child.close()
        if progress is not None:
            progress("discover", 0, None)

        result: dict[str, Any] | None = None
        while result is None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise RawPreviewError("raw preview exceeded its frame deadline")
            if parent.poll(min(0.04, remaining)):
                try:
                    message = _recv_until_deadline(parent, deadline)
                except (EOFError, OSError, ValueError, TypeError) as exc:
                    raise RawPreviewError("raw preview worker exited without a valid result") from exc
                if not isinstance(message, dict):
                    raise RawPreviewError("raw preview worker returned an invalid result")
                if message.get("kind") == "progress":
                    if progress is not None:
                        progress(message["stage"], int(message["completed"]), message.get("total"))
                    continue
                result = message
                break
            if not process.is_alive():
                raise RawPreviewError("raw preview worker exited without a result")

        if result["kind"] == "error":
            raise RawPreviewError(str(result.get("message") or "raw preview failed"))
        if result["kind"] == "ambiguous":
            raise RawPreviewAmbiguousError(result.get("candidates", []))
        if result["kind"] != "preview":
            raise RawPreviewError("raw preview worker returned an invalid result")
        _stop_worker(process, parent)
        process = None
        preview = _load_preview(root, result, runtime)
        if progress is not None:
            progress("acquire", 1, 1)
        return preview
    except TimeoutError as exc:
        raise RawPreviewError("raw preview exceeded its frame deadline") from exc
    finally:
        if process is not None:
            _stop_worker(process, parent)
        else:
            if parent is not None:
                parent.close()
            if child is not None:
                with suppress(OSError):
                    child.close()
        if root is not None:
            shutil.rmtree(root, ignore_errors=True)
            configured = runtime.get("temp_root")
            if configured and owns_configured_parent:
                configured_parent = Path(str(configured))
                with suppress(OSError):
                    if configured_parent.exists() and not any(configured_parent.iterdir()):
                        configured_parent.rmdir()


def _isolated_preview_worker_with(
    pipe: Any,
    worker: Callable[[Any, Query, EffectiveConfig, str, bool], None],
    query: Query,
    config: EffectiveConfig,
    root: str,
    apply_legacy: bool = False,
) -> None:
    if os.name == "posix":
        os.setsid()
    try:
        worker(pipe, query, config, root, apply_legacy)
    finally:
        pipe.close()


def _load_preview(root: Path, result: dict[str, Any], runtime: dict[str, Any]) -> RawPreview:
    import re

    import numpy as np

    path = root / "preview.png"
    if not path.is_file() or path.stat().st_size > int(runtime["max_temp_bytes"]):
        raise RawPreviewError("raw preview worker did not produce a bounded image")
    if not re.fullmatch(r"[0-9a-f]{64}", str(result.get("sha256", ""))):
        raise RawPreviewError("raw preview worker returned an invalid image hash")
    try:
        with Image.open(path) as image:
            if image.format != "PNG":
                raise ValueError("worker output is not PNG")
            width, height = image.size
            pixels = width * height
            if pixels > int(runtime["max_pixels"]):
                raise ValueError("worker output exceeds pixel limit")
            if pixels * 8 > int(runtime["max_temp_bytes"]):
                raise ValueError("worker output exceeds temporary byte limit")
            if width != result.get("width") or height != result.get("height"):
                raise ValueError("worker output dimensions do not match its receipt")
            rgba = np.asarray(image.convert("RGBA"), dtype=np.uint8).copy()
    except (OSError, ValueError) as exc:
        raise RawPreviewError("raw preview worker produced an unreadable image") from exc
    return RawPreview(
        rgba=rgba,
        file_identity=str(result["file_identity"]),
        format=str(result["format"]),
        sha256=str(result["sha256"]),
        source=result.get("source"),
        product=result.get("product"),
        station=result.get("station"),
        valid_time=result.get("valid_time"),
        frame_index=int(result["frame_index"]),
        display_mode=str(result["display_mode"]),
        rule_version=result.get("rule_version"),
        reason=result.get("reason"),
    )


__all__ = ["RawPreviewAmbiguousError", "RawPreviewError", "preview_source_raw"]
