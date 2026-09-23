"""Short-lived isolated source discoverer; sends a safe frame projection only."""

from __future__ import annotations

import asyncio
import json
import threading
from datetime import datetime, timezone
from typing import Any

from .cli.safety import safe_text
from .config import EffectiveConfig
from .context import Cancellation
from .discovery import DiscoveryTarget, error_item
from .errors import AuthenticationError, NoDataError, StaleFrameError, UnsupportedQueryError
from .models import Query

# A worker result is a small identity/error record.  Keeping the IPC payload
# bounded prevents a broken plugin from filling the coordinator pipe or
# forcing it to unpickle an arbitrary provider response.
MAX_IPC_BYTES = 1 * 1024 * 1024


def send_message(pipe: Any, value: Any) -> None:
    payload = json.dumps(value, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")
    if len(payload) > MAX_IPC_BYTES:
        raise ValueError("discovery worker message exceeds the IPC limit")
    pipe.send_bytes(payload)


def recv_message(pipe: Any) -> Any:
    payload = pipe.recv_bytes(MAX_IPC_BYTES)
    if len(payload) > MAX_IPC_BYTES:
        raise ValueError("discovery worker message exceeds the IPC limit")
    return json.loads(payload.decode("utf-8"))


class BoundedPipe:
    """Connection facade handed to adapters so all result writes are bounded."""

    def __init__(self, pipe: Any, cancellation_event: Any = None) -> None:
        self._pipe = pipe
        self._cancellation_event = cancellation_event

    def send(self, value: Any) -> None:
        send_message(self._pipe, value)

    def close(self) -> None:
        self._pipe.close()

    def wait_cancelled(self, timeout: float | None = None) -> bool:
        if self._cancellation_event is None:
            return False
        return bool(self._cancellation_event.wait(timeout))


def worker_main(pipe: Any, target: DiscoveryTarget, config: EffectiveConfig, max_age: float | None) -> None:
    from .client import Client

    result: dict[str, Any]
    cancellation = Cancellation()
    client = Client(config=config, _cancellation=cancellation)
    watcher_stop = threading.Event()

    def watch_parent_cancellation() -> None:
        while not watcher_stop.is_set():
            if pipe.wait_cancelled(0.025):
                client.cancel()
                return

    watcher = threading.Thread(
        target=watch_parent_cancellation, name="radiust-discovery-cancel", daemon=True,
    )
    watcher.start()
    try:
        query = Query(target.source, product=target.product, stations=(target.station,) if target.station else (), latest=True)
        with client:
            refs = client.discover(query)
        if not refs:
            result = error_item(target, "no_data", "source returned no matching frame")
        else:
            now = datetime.now(timezone.utc)
            if max_age is not None and all(
                ref.valid_time <= now and (now - ref.valid_time).total_seconds() > max_age
                for ref in refs
            ):
                result = error_item(target, "stale", "latest frame is older than --max-age")
            elif len(refs) > 1:
                result = error_item(target, "ambiguous", f"{len(refs)} candidates for source/product/station; select a single source")
            else:
                ref = refs[0]
                result = {
                    "source": target.source, "product": target.product, "station": target.station,
                    "status": "success", "valid_time": ref.valid_time.isoformat().replace("+00:00", "Z"),
                    "frame": {"source": ref.source, "product": ref.product, "station": ref.station, "valid_time": ref.valid_time.isoformat().replace("+00:00", "Z"), "base_time": ref.base_time.isoformat().replace("+00:00", "Z") if ref.base_time else None},
                    "capabilities": {"scientific_decode": None, "reason": "scientific validation was not checked by discovery"}, "error": None,
                }
    except (NoDataError, StaleFrameError, AuthenticationError, UnsupportedQueryError) as exc:
        status = "no_data" if isinstance(exc, NoDataError) else "stale" if isinstance(exc, StaleFrameError) else "missing_credentials" if isinstance(exc, AuthenticationError) else "upstream_failed"
        result = error_item(target, status, safe_text(exc), retryable=False)
    except asyncio.CancelledError:
        result = error_item(target, "cancelled", "discovery was cancelled", retryable=False)
    except Exception:
        # Unknown exceptions can contain tokens, response bodies and URLs.
        result = error_item(target, "upstream_failed", "source discovery failed; inspect provider logs safely", retryable=True)
    finally:
        watcher_stop.set()
        watcher.join(timeout=0.05)
    try:
        pipe.send(result)
    finally:
        pipe.close()


__all__ = ["BoundedPipe", "MAX_IPC_BYTES", "recv_message", "send_message", "worker_main"]
