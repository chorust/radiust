"""The aggregate discovery limiter is the single parent-owned quota."""

from __future__ import annotations

import asyncio
import concurrent.futures
import fcntl
import json
import multiprocessing as mp
import queue
import signal
import threading
import time
from pathlib import Path
from urllib.parse import urlparse

import pytest
from radiust.config import load_config
from radiust.discovery import _run_isolated_target, discover_all
from radiust.discovery_limits import DiscoveryLimiter
from radiust.models import DiscoveryTarget, ProductInfo, SourceInfo
from radiust.pipeline import Client
from radiust.transport import HTTPResponse, HTTPTransport


def _change_request_metrics(path: Path, source: str, host: str, delta: int) -> dict:
    with path.open("a+", encoding="utf-8") as stream:
        fcntl.flock(stream.fileno(), fcntl.LOCK_EX)
        stream.seek(0)
        try:
            state = json.load(stream)
        except json.JSONDecodeError:
            state = {"active": 0, "peak": 0, "by_source": {}, "peak_by_source": {}, "by_host": {}, "peak_by_host": {}}
        state["active"] += delta
        state["by_source"][source] = state["by_source"].get(source, 0) + delta
        state["by_host"][host] = state["by_host"].get(host, 0) + delta
        if delta > 0:
            state["peak"] = max(state["peak"], state["active"])
            state["peak_by_source"][source] = max(
                state["peak_by_source"].get(source, 0), state["by_source"][source]
            )
            state["peak_by_host"][host] = max(
                state["peak_by_host"].get(host, 0), state["by_host"][host]
            )
        stream.seek(0)
        stream.truncate()
        json.dump(state, stream)
        stream.flush()
        fcntl.flock(stream.fileno(), fcntl.LOCK_UN)
        return state


def _read_request_metrics(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as stream:
        fcntl.flock(stream.fileno(), fcntl.LOCK_SH)
        try:
            return json.load(stream)
        finally:
            fcntl.flock(stream.fileno(), fcntl.LOCK_UN)


class MeasuredHTTPTransport(HTTPTransport):
    def _request_once_response(self, request):
        headers = {key.lower(): value for key, value in request.header_items()}
        source = headers["x-radiust-source"]
        path = Path(headers["x-radiust-metrics"])
        expected_peak = int(headers["x-radiust-expected-peak"])
        host = urlparse(request.full_url).hostname or ""
        state = _change_request_metrics(path, source, host, 1)
        try:
            if expected_peak > 1:
                deadline = time.monotonic() + 2
                while state["active"] < expected_peak and time.monotonic() < deadline:
                    time.sleep(0.005)
                    state = _read_request_metrics(path)
            time.sleep(0.04)
            return HTTPResponse(b"ok", 200, request.full_url, {})
        finally:
            _change_request_metrics(path, source, host, -1)


def worker_reports_local_limits(pipe, target, config, max_age):
    runtime = config.values["runtime"]
    marker = Path(config.values["sources"][target.source]["marker"])
    marker.write_text(json.dumps({
        "frame": runtime["frame_concurrency"],
        "request": runtime["request_concurrency"],
        "host": runtime["host_concurrency"],
        "sources": sorted(config.values["sources"]),
        "storage": config.values["storage"],
    }), encoding="utf-8")
    pipe.send({
        "source": target.source, "product": target.product, "station": target.station,
        "status": "no_data", "valid_time": None, "frame": None, "capabilities": None, "error": None,
    })
    pipe.close()


def blocked_worker(pipe, target, config, max_age):
    time.sleep(60)


def cooperative_worker(pipe, target, config, max_age):
    marker = Path(config.values["sources"][target.source]["marker"])
    signal.signal(signal.SIGTERM, lambda *_args: marker.with_suffix(".term").write_text("term"))
    while not pipe.wait_cancelled(0.02):
        pass
    marker.with_suffix(".cooperative").write_text("cancelled")
    pipe.close()


def worker_makes_concurrent_requests(pipe, target, config, max_age):
    runtime = config.values["runtime"]
    endpoint = config.values["sources"][target.source]["probe_url"]
    transport = MeasuredHTTPTransport(
        allow_network=bool(runtime["allow_network"]),
        max_attempts=1,
        timeout=float(runtime["request_timeout"]),
        request_concurrency=int(runtime["request_concurrency"]),
        host_concurrency=int(runtime["host_concurrency"]),
    )

    async def request_burst():
        await asyncio.gather(*(
            transport.get(endpoint, headers={
                "X-Radiust-Source": target.source,
                "X-Radiust-Metrics": config.values["sources"][target.source]["metrics_file"],
                "X-Radiust-Expected-Peak": str(config.values["sources"][target.source]["expected_peak"]),
            })
            for _ in range(4)
        ))

    asyncio.run(request_burst())
    pipe.send({
        "source": target.source, "product": target.product, "station": target.station,
        "status": "no_data", "valid_time": None, "frame": None, "capabilities": None, "error": None,
    })
    pipe.close()


def test_limiter_caps_global_host_and_source_leases_and_reclaims_them():
    limiter = DiscoveryLimiter(global_limit=2, host_limit=1, source_limit=1)
    active = {"global": 0, "host": 0, "source": 0}
    peak = dict(active)
    lock = threading.Lock()
    barrier = threading.Barrier(4)

    def run(index: int) -> None:
        barrier.wait()
        with limiter.lease(f"source-{index % 2}", "same-host"):
            with lock:
                active["global"] += 1
                active["host"] += 1
                active["source"] += 1
                for key in peak:
                    peak[key] = max(peak[key], active[key])
            time.sleep(0.02)
            with lock:
                for key in active:
                    active[key] -= 1

    threads = [threading.Thread(target=run, args=(index,)) for index in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=2)
    assert all(not thread.is_alive() for thread in threads)
    assert peak["global"] <= 2
    assert peak["host"] <= 1
    assert peak["source"] <= 1
    assert limiter.snapshot() == {"global": 0, "hosts": {}, "sources": {}}


def test_discovery_network_restriction_skips_workers_and_socket_budget():
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    called = []

    def worker(*_args):
        called.append(True)
        raise AssertionError("network-restricted discovery must not start a worker")

    report = discover_all(
        load_config({"runtime": {"allow_network": False}}, environ={}),
        catalog=(info,),
        worker_fn=worker,
    )
    assert report["counts"]["network_restricted"] == 1
    assert called == []


def test_discovery_missing_credentials_skips_worker():
    info = SourceInfo("id", "Indonesia", "1", (ProductInfo("radar", variables=("rain_rate",)),))

    def worker(*_args):
        raise AssertionError("missing credentials must skip worker startup")

    config = load_config({"runtime": {"allow_network": True}}, environ={})
    report = discover_all(config, catalog=(info,), worker_fn=worker)

    assert report["counts"]["missing_credentials"] == 1


def test_limiter_rejects_non_positive_limits():
    with pytest.raises(ValueError):
        DiscoveryLimiter(global_limit=0, host_limit=1, source_limit=1)


def test_snapshot_reflects_active_leases_and_clears_after_exception():
    limiter = DiscoveryLimiter(global_limit=2, host_limit=2, source_limit=1)
    with pytest.raises(RuntimeError, match="stop"), limiter.lease("a", "host-a"):
        assert limiter.snapshot() == {"global": 1, "hosts": {"host-a": 1}, "sources": {"a": 1}}
        with limiter.lease("b", "host-b"):
            assert limiter.snapshot() == {"global": 2, "hosts": {"host-a": 1, "host-b": 1},
                                          "sources": {"a": 1, "b": 1}}
            raise RuntimeError("stop")
    assert limiter.snapshot() == {"global": 0, "hosts": {}, "sources": {}}


def test_spawned_discovery_worker_gets_one_local_slot_and_only_its_source(tmp_path):
    marker = tmp_path / "worker-config.json"
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({
        "runtime": {"allow_network": True, "frame_concurrency": 8, "request_concurrency": 12, "host_concurrency": 6},
        "sources": {"fake": {"marker": str(marker)}, "other": {"token": "must-not-cross"}},
        "storage": {"access_key": "synthetic-access", "secret_key": "synthetic-secret", "endpoint": "https://private.invalid"},
    }, environ={})

    report = discover_all(config, catalog=(info,), worker_fn=worker_reports_local_limits)
    child_config = json.loads(marker.read_text(encoding="utf-8"))

    assert report["counts"]["no_data"] == 1
    assert {key: child_config[key] for key in ("frame", "request", "host")} == {
        "frame": 1, "request": 1, "host": 1,
    }
    assert child_config["sources"] == ["fake"]
    assert child_config["storage"]["access_key"] is None
    assert child_config["storage"]["secret_key"] is None
    assert child_config["storage"]["endpoint"] is None


def test_worker_termination_reclaims_parent_global_host_and_source_lease(tmp_path):
    config = load_config({"runtime": {
        "allow_network": True, "discovery_deadline": 0.25, "temp_root": str(tmp_path / "owned"),
    }}, environ={})
    limiter = DiscoveryLimiter(global_limit=2, host_limit=2, source_limit=1)
    started = time.monotonic()

    result = _run_isolated_target(
        mp.get_context("spawn"),
        DiscoveryTarget("fake", "p", None),
        config,
        None,
        started + 0.25,
        blocked_worker,
        limiter,
    )

    assert time.monotonic() - started < 5
    assert result["status"] == "timeout"
    assert limiter.snapshot() == {"global": 0, "hosts": {}, "sources": {}}
    assert not (tmp_path / "owned").exists()


def test_worker_gets_cooperative_cancel_before_process_termination(tmp_path):
    marker = tmp_path / "worker-state"
    config = load_config({"runtime": {
        "allow_network": True, "discovery_deadline": 0.25,
        "temp_root": str(tmp_path / "owned"),
    }, "sources": {"fake": {"marker": str(marker)}}}, environ={})
    limiter = DiscoveryLimiter(global_limit=1, host_limit=1, source_limit=1)
    started = time.monotonic()

    result = _run_isolated_target(
        mp.get_context("spawn"), DiscoveryTarget("fake", "p", None), config,
        None, started + 0.25, cooperative_worker, limiter,
    )

    assert time.monotonic() - started < 5
    assert result["status"] == "timeout"
    assert marker.with_suffix(".cooperative").read_text() == "cancelled"
    assert not marker.with_suffix(".term").exists()
    assert limiter.snapshot() == {"global": 0, "hosts": {}, "sources": {}}
    assert not (tmp_path / "owned").exists()


def test_client_cancels_active_event_loop_task_from_parent_thread():
    client_queue = queue.Queue()
    started = threading.Event()
    finished = threading.Event()
    outcome = {}

    def run_client_operation():
        client = Client(config=load_config(environ={}))
        client_queue.put(client)

        async def blocked_operation():
            started.set()
            await asyncio.Event().wait()

        try:
            client._run(blocked_operation())
            outcome["result"] = "returned"
        except asyncio.CancelledError:
            outcome["result"] = "cancelled"
        finally:
            client.close()
            finished.set()

    thread = threading.Thread(target=run_client_operation)
    thread.start()
    client = client_queue.get(timeout=2)
    assert started.wait(timeout=2)
    client.cancel()
    thread.join(timeout=2)

    assert not thread.is_alive()
    assert finished.is_set()
    assert outcome["result"] == "cancelled"


@pytest.mark.parametrize(
    ("source_ids", "global_limit", "host_limit", "source_limit", "expected_peak"),
    [
        (("fake-a", "fake-b"), 1, 2, 1, 1),  # parent global lease
        (("fake-a", "fake-b"), 2, 1, 1, 1),  # shared host lease
        (("fake-a", "fake-a"), 2, 2, 1, 1),  # source lease
        (("fake-a", "fake-b"), 2, 2, 1, 2),  # allowed aggregate peak
    ],
)
def test_spawn_worker_request_peaks_obey_parent_leases(
    tmp_path, source_ids, global_limit, host_limit, source_limit, expected_peak,
):
    metrics_path = tmp_path / "request-metrics.json"
    metrics_path.write_text(json.dumps({
        "active": 0, "peak": 0, "by_source": {}, "peak_by_source": {},
        "by_host": {}, "peak_by_host": {},
    }), encoding="utf-8")
    endpoint = "https://shared.example.test/probe"
    runtime = {
        "allow_network": True, "request_timeout": 2, "discovery_deadline": 8,
    }
    source_configs = {
        source: {
            "probe_url": endpoint,
            "metrics_file": str(metrics_path),
            "expected_peak": expected_peak,
        }
        for source in set(source_ids)
    }
    config = load_config({"runtime": runtime, "sources": source_configs}, environ={})
    limiter = DiscoveryLimiter(
        global_limit=global_limit, host_limit=host_limit, source_limit=source_limit,
    )
    targets = [DiscoveryTarget(source, f"p{index}", None) for index, source in enumerate(source_ids)]
    deadline = time.monotonic() + 8
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        futures = [pool.submit(
            _run_isolated_target,
            mp.get_context("spawn"), target, config, None, deadline,
            worker_makes_concurrent_requests, limiter,
        ) for target in targets]
        results = [future.result(timeout=10) for future in futures]

    metrics = _read_request_metrics(metrics_path)
    assert [result["status"] for result in results] == ["no_data", "no_data"]
    assert metrics["peak"] == expected_peak
    assert all(peak <= 1 for peak in metrics["peak_by_source"].values())
    assert all(peak <= host_limit for peak in metrics["peak_by_host"].values())
    assert metrics["active"] == 0
    assert limiter.snapshot() == {"global": 0, "hosts": {}, "sources": {}}
