"""Parent-owned concurrency leases for aggregate discovery.

Workers are short-lived and must not each receive a fresh copy of the global
quota.  The limiter therefore lives in the coordinator process and exposes a
small synchronous lease API that is also useful for deterministic tests.
"""

from __future__ import annotations

import threading
import time
from contextlib import AbstractContextManager
from typing import Any


class _Lease(AbstractContextManager["_Lease"]):
    def __init__(self, owner: DiscoveryLimiter, source: str, host: str, timeout: float | None) -> None:
        self._owner = owner
        self._source = source
        self._host = host
        self._timeout = timeout
        self._acquired: list[threading.Semaphore] = []
        self._recorded = False

    def __enter__(self) -> _Lease:
        deadline = None if self._timeout is None else time.monotonic() + self._timeout
        semaphores = (
            self._owner._global,
            self._owner._host(self._host),
            self._owner._source(self._source),
        )
        try:
            for semaphore in semaphores:
                remaining = None if deadline is None else max(0.0, deadline - time.monotonic())
                acquired = semaphore.acquire() if remaining is None else semaphore.acquire(timeout=remaining)
                if not acquired:
                    raise TimeoutError("discovery concurrency lease timed out")
                self._acquired.append(semaphore)
            self._owner._record(self._source, self._host, 1)
            self._recorded = True
        except BaseException:
            self._release()
            raise
        return self

    def _release(self) -> None:
        if self._recorded:
            self._owner._record(self._source, self._host, -1)
            self._recorded = False
        for semaphore in reversed(self._acquired):
            semaphore.release()
        self._acquired.clear()

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self._release()


class DiscoveryLimiter:
    """Bound global, host and source leases in a single coordinator."""

    def __init__(self, *, global_limit: int, host_limit: int, source_limit: int) -> None:
        for name, value in (("global_limit", global_limit), ("host_limit", host_limit), ("source_limit", source_limit)):
            if not isinstance(value, int) or value < 1:
                raise ValueError(f"{name} must be a positive integer")
        self._global = threading.BoundedSemaphore(global_limit)
        self._host_limit = host_limit
        self._source_limit = source_limit
        self._lock = threading.Lock()
        self._hosts: dict[str, threading.BoundedSemaphore] = {}
        self._sources: dict[str, threading.BoundedSemaphore] = {}
        self._active_global = 0
        self._active_hosts: dict[str, int] = {}
        self._active_sources: dict[str, int] = {}

    def _record(self, source: str, host: str, delta: int) -> None:
        with self._lock:
            self._active_global += delta
            for entries, key in ((self._active_hosts, host), (self._active_sources, source)):
                value = entries.get(key, 0) + delta
                if value:
                    entries[key] = value
                else:
                    entries.pop(key, None)

    def _host(self, host: str) -> threading.BoundedSemaphore:
        with self._lock:
            return self._hosts.setdefault(host, threading.BoundedSemaphore(self._host_limit))

    def _source(self, source: str) -> threading.BoundedSemaphore:
        with self._lock:
            return self._sources.setdefault(source, threading.BoundedSemaphore(self._source_limit))

    def lease(self, source: str, host: str, *, timeout: float | None = None) -> _Lease:
        if not isinstance(source, str) or not source or not isinstance(host, str) or not host:
            raise ValueError("source and host are required for a discovery lease")
        if timeout is not None and timeout < 0:
            raise ValueError("lease timeout cannot be negative")
        return _Lease(self, source, host, timeout)

    def snapshot(self) -> dict[str, Any]:
        # Track real active leases without inspecting semaphore internals.
        with self._lock:
            return {"global": self._active_global, "hosts": dict(self._active_hosts),
                    "sources": dict(self._active_sources)}


__all__ = ["DiscoveryLimiter"]
