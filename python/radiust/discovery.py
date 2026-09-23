"""Catalog-complete, bounded aggregate discovery for the CLI."""

from __future__ import annotations

import json
import multiprocessing as mp
import os
import shutil
import signal
import tempfile
import threading
import time
from collections.abc import Callable
from contextlib import suppress
from importlib import resources
from pathlib import Path
from typing import Any

from .config import EffectiveConfig
from .discovery_limits import DiscoveryLimiter
from .models import DISCOVERY_STATUSES, DiscoveryItem, DiscoveryReport, DiscoveryTarget, SourceInfo
from .registry import sources

STATUSES = DISCOVERY_STATUSES

# These adapters explicitly require the named source configuration fields.
# Availability labels alone must not be treated as proof of missing credentials.
REQUIRED_CREDENTIALS = {
    "id": "token", "ph": "timeline_token", "id_sidarma": "api_key", "wunderground": "api_key",
}

_MAX_CATALOG_TARGETS = 20_000


def _recv_until_deadline(parent: Any, deadline: float) -> Any:
    """Bound the *whole* IPC read, not just readiness of the frame header.

    Connection.poll() becomes ready as soon as a writer supplies the length
    header. An untrusted child can then stall forever halfway through the
    payload; an unconditional recv_bytes() would defeat the shared deadline.
    The worker is terminated in the caller's finally block on timeout, which
    also closes the pipe and releases any waiting reader.
    """
    from .discovery_worker import recv_message

    if time.monotonic() >= deadline:
        raise TimeoutError("discovery IPC exceeded the batch budget")
    done = threading.Event()
    received: list[Any] = []
    failures: list[Exception] = []

    def receive() -> None:
        try:
            received.append(recv_message(parent))
        except Exception as exc:
            failures.append(exc)
        finally:
            done.set()

    threading.Thread(target=receive, name="radiust-discovery-ipc", daemon=True).start()
    while not done.is_set():
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("discovery IPC exceeded the batch budget")
        done.wait(min(0.04, remaining))
    if time.monotonic() >= deadline:
        raise TimeoutError("discovery IPC exceeded the batch budget")
    if failures:
        raise failures[0]
    return received[0]


def _catalog_worker(pipe: Any, catalog_fn: Callable[[], tuple[SourceInfo, ...]]) -> None:
    """Keep entry-point imports, source enumeration and target expansion killable."""
    if os.name == "posix":
        os.setsid()
    try:
        catalog = tuple(catalog_fn())
        if len(catalog) > _MAX_CATALOG_TARGETS:
            raise ValueError("catalog source limit exceeded")
        targets = expand_targets(catalog, limit=_MAX_CATALOG_TARGETS)
        from .discovery_worker import send_message

        send_message(pipe, {
            "targets": [(target.source, target.product, target.station) for target in targets],
            "availability": {info.id: info.availability for info in catalog},
        })
    except Exception:
        # Untrusted plugin exceptions may include credentials or response bodies.
        from .discovery_worker import send_message

        with suppress(Exception):
            send_message(pipe, {"error": "catalog expansion failed"})
    finally:
        pipe.close()


def _catalog_placeholders(status: str, message: str, *, max_age: float | None, interrupted: bool = False) -> dict[str, Any]:
    """Retain known built-in sources when a plugin prevents catalog expansion."""
    try:
        catalog_path = resources.files("radiust.resources").joinpath("catalog.json")
        ids = {item["id"] for item in json.loads(catalog_path.read_text(encoding="utf-8"))["sources"]}
    except (OSError, ValueError, KeyError, TypeError):
        ids = set()
    if not ids:
        ids = {"catalog"}
    return discovery_report(
        [error_item(DiscoveryTarget(source, None, None), status, message, retryable=True)
         for source in sorted(ids)],
        max_age=max_age, interrupted=interrupted,
    )


def _bounded_catalog(
    ctx: Any, deadline: float, catalog_fn: Callable[[], tuple[SourceInfo, ...]],
) -> tuple[list[DiscoveryTarget], dict[str, str]] | None:
    parent, child = ctx.Pipe(duplex=False)
    process = ctx.Process(target=_catalog_worker, args=(child, catalog_fn), daemon=True)
    try:
        process.start()
    except Exception:
        parent.close()
        child.close()
        return None
    child.close()
    try:
        while process.is_alive() and not parent.poll(0.04):
            if time.monotonic() >= deadline:
                break
        if time.monotonic() >= deadline or not parent.poll():
            return None
        try:
            result = _recv_until_deadline(parent, deadline)
            if "error" in result:
                return None
            targets = [DiscoveryTarget(*parts) for parts in result["targets"]]
            if len(targets) > _MAX_CATALOG_TARGETS or len(targets) != len(set(targets)):
                return None
            availability = result["availability"]
            if not all(t.source in availability for t in targets):
                return None
            return targets, availability
        except (EOFError, OSError, KeyError, ValueError, TypeError, AttributeError, TimeoutError):
            return None
    finally:
        _stop_worker(process, parent)


def _isolated_worker(
    pipe: Any,
    target: DiscoveryTarget,
    config: EffectiveConfig,
    max_age: float | None,
    worker: Any,
    cancellation_event: Any,
) -> None:
    # Isolate worker-created browser/HTTP subprocesses so that the parent can
    # terminate the entire owned process group when the budget expires.
    if os.name == "posix":
        os.setsid()
    from .discovery_worker import BoundedPipe

    worker(BoundedPipe(pipe, cancellation_event), target, config, max_age)


def expand_targets(catalog: tuple[SourceInfo, ...], *, limit: int | None = None) -> list[DiscoveryTarget]:
    if limit is not None and limit < 1:
        raise ValueError("catalog target limit must be positive")
    targets: set[DiscoveryTarget] = set()

    def add(target: DiscoveryTarget) -> None:
        if target in targets:
            return
        if limit is not None and len(targets) >= limit:
            raise ValueError("catalog target limit exceeded")
        targets.add(target)

    for info in catalog:
        if not info.products:
            add(DiscoveryTarget(info.id, None, None))
        for product in info.products:
            stations = [s.id for s in info.stations if not s.product_ids or product.id in s.product_ids]
            if not stations:
                add(DiscoveryTarget(info.id, product.id, None))
            else:
                for station in stations:
                    add(DiscoveryTarget(info.id, product.id, station))
    return sorted(targets, key=lambda t: (t.source, t.product or "", t.station or ""))


def error_item(target: DiscoveryTarget, status: str, message: str, *, retryable: bool = False) -> dict[str, Any]:
    return {
        "source": target.source, "product": target.product, "station": target.station,
        "status": status, "valid_time": None, "frame": None, "capabilities": None,
        "error": {"code": status, "message": message, "stage": "discover", "retryable": retryable},
    }


def discovery_report(items: list[dict[str, Any]], *, interrupted: bool = False, max_age: float | None = None) -> dict[str, Any]:
    return DiscoveryReport(
        tuple(DiscoveryItem.from_mapping(item) for item in items),
        interrupted=interrupted, max_age=max_age,
    ).as_dict()


def discovery_exit_code(report: dict[str, Any]) -> int:
    if report["interrupted"]:
        return 130
    counts = report["counts"]
    if not counts["total"]:
        return 3
    if counts["success"] == counts["total"]:
        return 0
    if counts["success"]:
        return 4
    if counts["no_data"] + counts["stale"] == counts["total"]:
        return 3
    return 5


def _worker_temp_root(config: EffectiveConfig) -> tuple[Path, bool]:
    configured = config.values["runtime"].get("temp_root")
    if configured:
        parent = Path(str(configured))
        owns_parent = False
        try:
            parent.mkdir(parents=True)
            owns_parent = True
        except FileExistsError:
            if not parent.is_dir():
                raise
        return Path(tempfile.mkdtemp(prefix="worker-", dir=parent)), owns_parent
    return Path(tempfile.mkdtemp(prefix="radiust-discovery-")), False


def _run_isolated_target(
    ctx: Any,
    target: DiscoveryTarget,
    config: EffectiveConfig,
    max_age: float | None,
    deadline: float,
    worker: Any,
    limiter: DiscoveryLimiter,
) -> dict[str, Any]:
    """Run one source behind a bounded IPC and parent-owned lease."""
    remaining = max(0.0, deadline - time.monotonic())
    # Discovery has no child-to-parent per-request broker. Use one shared
    # conservative host bucket for whole-worker leases; HTTPTransport applies
    # real per-host limits inside that serialized worker. This keeps unknown
    # and browser-owned network paths within the same parent quota.
    lease = limiter.lease(target.source, "_discovery_worker", timeout=remaining)
    try:
        lease.__enter__()
    except TimeoutError:
        return error_item(target, "timeout", "discovery concurrency lease exceeded the batch budget", retryable=True)

    parent, child = ctx.Pipe(duplex=False)
    cancellation_event = ctx.Event()
    temp_root: Path | None = None
    owns_configured_parent = False
    process: mp.Process | None = None
    try:
        temp_root, owns_configured_parent = _worker_temp_root(config)
        source_config = config.values.get("sources", {}).get(target.source, {})
        local_values = dict(config.values)
        local_values["sources"] = {target.source: source_config}
        local_values["runtime"] = {
            **config.values["runtime"],
            "temp_root": str(temp_root),
            # Until request leases can be brokered over IPC, each child gets
            # one request slot. Parent worker leases then bound aggregate
            # requests even when an adapter internally gathers many URLs.
            "frame_concurrency": 1,
            "request_concurrency": 1,
            "host_concurrency": 1,
        }
        # Child receives no storage credentials or remote output endpoint.
        local_values["storage"] = {
            **config.values["storage"], "access_key": None, "secret_key": None, "endpoint": None,
        }
        local_config = EffectiveConfig(local_values, config.origins)
        process = ctx.Process(
            target=_isolated_worker,
            args=(child, target, local_config, max_age, worker, cancellation_event),
            daemon=True,
        )
        try:
            process.start()
        except Exception:
            process = None
            return error_item(target, "upstream_failed", "discovery worker could not start", retryable=True)
        child.close()
        while process.is_alive() and not parent.poll(0.04):
            if time.monotonic() >= deadline:
                break
        if parent.poll():
            try:
                received = _recv_until_deadline(parent, deadline)
                item = DiscoveryItem.from_mapping(received)
                if item.target != target:
                    raise ValueError("worker result does not match its requested target")
                return item.as_dict()
            except TimeoutError:
                return error_item(target, "timeout", "discovery worker IPC exceeded the batch budget", retryable=True)
            except (EOFError, OSError):
                return error_item(target, "upstream_failed", "discovery worker exited without a result", retryable=True)
            except (TypeError, ValueError, KeyError, AttributeError, UnicodeError, json.JSONDecodeError):
                # A plugin's malformed or misattributed report must not
                # overwrite another target or abort the entire batch.
                return error_item(target, "upstream_failed", "discovery worker returned an invalid result", retryable=True)
        return error_item(
            target,
            "timeout" if time.monotonic() >= deadline else "upstream_failed",
            "discovery worker exceeded budget or exited without a result",
            retryable=True,
        )
    finally:
        if process is not None:
            _stop_worker(process, parent, cancellation_event)
        else:
            parent.close()
            child.close()
        if temp_root is not None:
            shutil.rmtree(temp_root, ignore_errors=True)
            configured = config.values["runtime"].get("temp_root")
            if configured and owns_configured_parent:
                parent = Path(str(configured))
                with suppress(OSError):
                    if parent.exists() and not any(parent.iterdir()):
                        parent.rmdir()
        lease.__exit__(None, None, None)


def discover_all(
    config: EffectiveConfig,
    *,
    max_age: float | None = None,
    catalog: tuple[SourceInfo, ...] | None = None,
    catalog_fn: Callable[[], tuple[SourceInfo, ...]] | None = None,
    worker_fn: Any = None,
    progress: Callable[[str, int, int], None] | None = None,
) -> dict[str, Any]:
    """A dedicated child boundary ensures a blocked source cannot hold the CLI open."""
    from .discovery_worker import worker_main

    deadline = time.monotonic() + float(config.values["runtime"]["discovery_deadline"])
    ctx = mp.get_context("spawn")
    if catalog is None:
        try:
            expanded = _bounded_catalog(ctx, deadline, catalog_fn or sources)
        except KeyboardInterrupt:
            report = _catalog_placeholders(
                "not_started", "discovery interrupted before catalog expansion completed",
                max_age=max_age, interrupted=True,
            )
            return report
        if expanded is None:
            timeout = time.monotonic() >= deadline
            report = _catalog_placeholders(
                "timeout" if timeout else "upstream_failed",
                "catalog expansion exceeded the batch budget" if timeout else "catalog expansion failed",
                max_age=max_age,
            )
            if progress is not None:
                progress("discover", report["counts"]["total"], report["counts"]["total"])
            return report
        targets, availability = expanded
    else:
        try:
            targets = expand_targets(catalog, limit=_MAX_CATALOG_TARGETS)
        except ValueError:
            return discovery_report(
                [error_item(DiscoveryTarget(info.id, None, None), "upstream_failed", "catalog target limit exceeded", retryable=True)
                 for info in catalog],
                max_age=max_age,
            )
        availability = {info.id: info.availability for info in catalog}
    items: list[dict[str, Any]] = []
    interrupted = False
    index = 0
    active_target: DiscoveryTarget | None = None
    runtime = config.values["runtime"]
    limiter = DiscoveryLimiter(
        global_limit=min(int(runtime["frame_concurrency"]), int(runtime["request_concurrency"]), int(runtime["host_concurrency"])),
        host_limit=int(runtime["host_concurrency"]),
        source_limit=1,
    )

    def completed() -> None:
        if progress is not None:
            progress("discover", len(items), len(targets))

    try:
        while index < len(targets):
            target = targets[index]
            if time.monotonic() >= deadline:
                break
            if availability[target.source] == "retired":
                items.append(error_item(target, "retired", "source retired"))
                index += 1
                completed()
                continue
            if availability[target.source] in {"missing_dependency", "upstream_unavailable"}:
                items.append(error_item(target, "upstream_failed", "source is unavailable in the current installation", retryable=True))
                index += 1
                completed()
                continue
            required = REQUIRED_CREDENTIALS.get(target.source)
            source_config = config.values.get("sources", {}).get(target.source, {})
            if required and not source_config.get(required):
                items.append(error_item(target, "missing_credentials", f"source requires sources.{target.source}.{required}"))
                index += 1
                completed()
                continue
            # Existing network permission is authoritative; do not probe the public
            # provider, import credentials from another checkout or infer that a
            # fixture in tests represents live availability.
            if not config.values["runtime"]["allow_network"]:
                items.append(error_item(target, "network_restricted", "public network access is disabled"))
                index += 1
                completed()
                continue
            active_target = target
            items.append(_run_isolated_target(ctx, target, config, max_age, deadline, worker_fn or worker_main, limiter))
            active_target = None
            index += 1
            completed()
    except KeyboardInterrupt:
        interrupted = True
        if active_target is not None:
            items.append(error_item(active_target, "cancelled", "discovery interrupted"))
            index += 1
            completed()

    for target in targets[index:]:
        items.append(error_item(target, "not_started", "discovery was not started before the batch ended"))
        completed()
    return discovery_report(items, interrupted=interrupted, max_age=max_age)


def _stop_worker(process: mp.Process, parent: Any, cancellation_event: Any = None) -> None:
    try:
        if cancellation_event is not None and process.is_alive():
            cancellation_event.set()
            # Give the worker a brief chance to cancel its active task and
            # release source/browser resources before process-group teardown.
            process.join(timeout=0.5)
        group: int | None = None
        if os.name == "posix" and process.pid is not None:
            # A finished direct worker may leave a subprocess alive. A group
            # exists only after _isolated_worker has called setsid().
            try:
                group = os.getpgid(process.pid)
            except ProcessLookupError:
                group = process.pid
            if group == process.pid and group != os.getpgrp():
                with suppress(ProcessLookupError):
                    os.killpg(group, signal.SIGTERM)
            else:
                group = None
        if process.is_alive():
            process.terminate()
        process.join(timeout=0.5)
        if group is not None:
            # A detached helper that ignores TERM must not outlive its worker.
            with suppress(ProcessLookupError):
                os.killpg(group, signal.SIGKILL)
        if process.is_alive():
            process.kill()
            process.join(timeout=0.5)
    finally:
        parent.close()
