"""Measure real process termination; cancellation of an asyncio Task alone is insufficient."""

import asyncio
import json
import multiprocessing as mp
import os
import signal
import struct
import subprocess
import sys
import time
from pathlib import Path

import pytest
from radiust.config import load_config
from radiust.context import SourceContext
from radiust.discovery import discover_all
from radiust.models import ProductInfo, SourceInfo
from radiust.sources.browser import BrowserAcquirer
from radiust.transport import HTTPTransport


def hanging_worker(pipe, target, config, max_age):
    try:
        time.sleep(60)
    finally:
        pipe.close()


class BlockingTransport(HTTPTransport):
    def _request_once_response(self, request):
        time.sleep(60)
        raise AssertionError("blocked request unexpectedly returned")


def worker_blocked_inside_http_transport(pipe, target, config, max_age):
    transport = BlockingTransport(
        allow_network=True, max_attempts=1,
        request_concurrency=1, host_concurrency=1,
    )

    async def request():
        await transport.get("https://provider.example/radar")

    asyncio.run(request())


def worker_blocked_inside_browser_navigation(pipe, target, config, max_age):
    class Page:
        def on(self, *_args):
            pass

        async def goto(self, *_args, **_kwargs):
            await asyncio.sleep(60)

    class BrowserContext:
        async def new_page(self):
            return Page()

        async def route(self, *_args):
            pass

        async def close(self):
            pass

    class Browser:
        async def new_context(self, **_kwargs):
            return BrowserContext()

        async def close(self):
            pass

    class Manager:
        async def __aenter__(self):
            self.chromium = self
            return self

        async def __aexit__(self, *_args):
            pass

        async def launch(self, **_kwargs):
            return Browser()

    context = SourceContext(
        config, target.source, temp_root=Path(config.values["runtime"]["temp_root"]),
    )

    async def browse():
        await BrowserAcquirer(Manager).fetch_page("https://provider.example/timeline", context)

    asyncio.run(browse())


def successful_first_then_hanging_worker(pipe, target, config, max_age):
    if target.product == "a":
        stamp = "2026-09-22T00:00:00Z"
        pipe.send({
            "source": target.source, "product": target.product, "station": target.station,
            "status": "success", "valid_time": stamp,
            "frame": {"source": target.source, "product": target.product,
                      "station": target.station, "valid_time": stamp, "base_time": None},
            "capabilities": None, "error": None,
        })
        pipe.close()
    else:
        hanging_worker(pipe, target, config, max_age)


def worker_with_subprocess(pipe, target, config, max_age):
    marker = Path(config.values["sources"]["fake"]["marker"])
    descendant = subprocess.Popen([
        sys.executable, "-c",
        "import pathlib, signal, sys, time\n"
        "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
        "pathlib.Path(sys.argv[2]).write_text('ready')\n"
        "deadline = time.monotonic() + 60\n"
        "while time.monotonic() < deadline:\n"
        "    pathlib.Path(sys.argv[3]).write_text(str(time.monotonic_ns()))\n"
        "    time.sleep(.02)\n"
        "pathlib.Path(sys.argv[1]).write_text('escaped')\n",
        str(marker), str(marker.with_suffix(".ready")), str(marker.with_suffix(".heartbeat")),
    ])
    marker.with_suffix(".pid").write_text(str(descendant.pid))
    try:
        time.sleep(60)
    finally:
        pipe.close()


def worker_with_owned_temp_resource(pipe, target, config, max_age):
    root = Path(config.values["runtime"]["temp_root"])
    root.mkdir(parents=True, exist_ok=True)
    (root / "owned").write_text("worker", encoding="utf-8")
    try:
        time.sleep(60)
    finally:
        pipe.close()


def worker_with_invalid_result(pipe, target, config, max_age):
    """A broken plugin must not impersonate a different catalog target."""
    pipe.send({"source": "not-the-target", "product": target.product, "station": target.station,
               "status": "success", "valid_time": "2026-09-22T00:00:00Z",
               "frame": {"source": "not-the-target", "product": target.product,
                         "station": target.station, "valid_time": "2026-09-22T00:00:00Z"}})
    pipe.close()


def worker_with_oversized_result(pipe, target, config, max_age):
    """A source/plugin must not be able to fill the parent's IPC pipe."""
    pipe.send({
        "source": target.source,
        "product": target.product,
        "station": target.station,
        "status": "upstream_failed",
        "error": {"code": "upstream_failed", "message": "x" * (2 * 1024 * 1024)},
    })
    pipe.close()


def worker_with_partial_ipc_message(pipe, target, config, max_age):
    """An injected plugin can write a header and never finish its IPC body."""
    os.write(pipe._pipe.fileno(), struct.pack("!i", 4096) + b"{")
    time.sleep(60)


def worker_with_late_success(pipe, target, config, max_age):
    time.sleep(0.5)
    stamp = "2026-09-22T00:00:00Z"
    pipe.send({
        "source": target.source, "product": target.product, "station": target.station,
        "status": "success", "valid_time": stamp,
        "frame": {"source": target.source, "product": target.product,
                  "station": target.station, "valid_time": stamp, "base_time": None},
        "error": None,
    })
    pipe.close()


def hanging_catalog():
    time.sleep(60)
    return ()


def crashing_catalog():
    raise RuntimeError("upstream catalog response: private-unlabelled-secret")


def catalog_with_partial_ipc_response():
    """Simulate a plugin corrupting the catalog worker's framed response."""
    import radiust.discovery_worker as ipc

    def partial_message(pipe, _result):
        os.write(pipe.fileno(), struct.pack("!i", 4096) + b"{")
        time.sleep(60)

    ipc.send_message = partial_message
    return (SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),)),)


@pytest.mark.parametrize(("catalog_fn", "expected_status"), [
    (hanging_catalog, "timeout"), (crashing_catalog, "upstream_failed"),
])
def test_catalog_expansion_is_bounded_and_failure_preserves_source_placeholders(catalog_fn, expected_status):
    config = load_config({"runtime": {"discovery_deadline": 0.4}}, environ={})
    start = time.monotonic()
    report = discover_all(config, catalog_fn=catalog_fn)
    assert time.monotonic() - start < 5
    assert report["counts"]["total"] > 0
    assert report["counts"][expected_status] == report["counts"]["total"]
    assert all(item["product"] is None and item["station"] is None for item in report["items"])
    assert "private-unlabelled-secret" not in repr(report)


def test_catalog_stage_interrupt_preserves_single_json_report_and_exit_priority(monkeypatch):
    import radiust.discovery as discovery

    def interrupt(*_args):
        raise KeyboardInterrupt

    monkeypatch.setattr(discovery, "_bounded_catalog", interrupt)
    report = discover_all(load_config(environ={}))
    assert report["interrupted"] is True
    assert report["counts"]["total"] > 0
    assert report["counts"]["not_started"] == report["counts"]["total"]
    assert discovery.discovery_exit_code(report) == 130


@pytest.mark.skipif(os.name != "posix", reason="malformed POSIX pipe frame")
def test_partial_catalog_ipc_body_returns_bounded_source_placeholders():
    config = load_config({"runtime": {"discovery_deadline": 1}}, environ={})
    before = {child.pid for child in mp.active_children()}
    started = time.monotonic()
    report = discover_all(config, catalog_fn=catalog_with_partial_ipc_response)
    assert time.monotonic() - started < 5
    assert report["counts"]["total"] > 0
    assert report["counts"]["timeout"] == report["counts"]["total"]
    assert all(item["product"] is None and item["station"] is None for item in report["items"])
    assert {child.pid for child in mp.active_children()} <= before


def test_invalid_worker_result_is_attributed_to_original_target():
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 2}}, environ={})
    report = discover_all(config, catalog=(info,), worker_fn=worker_with_invalid_result)
    assert report["counts"]["upstream_failed"] == 1
    assert report["items"][0]["source"] == "fake"
    assert report["items"][0]["frame"] is None


def test_oversized_worker_message_is_rejected_without_blocking_parent():
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 1}}, environ={})
    start = time.monotonic()
    report = discover_all(config, catalog=(info,), worker_fn=worker_with_oversized_result)
    assert time.monotonic() - start < 5
    assert report["counts"]["upstream_failed"] == 1
    assert "xxx" not in repr(report)


@pytest.mark.skipif(os.name != "posix", reason="malformed POSIX pipe frame")
def test_partial_worker_ipc_body_never_blocks_parent_past_deadline(tmp_path):
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    root = tmp_path / "isolated-worker"
    config = load_config({"runtime": {
        "allow_network": True, "discovery_deadline": 1, "temp_root": str(root),
    }}, environ={})
    before = {child.pid for child in mp.active_children()}
    started = time.monotonic()
    report = discover_all(config, catalog=(info,), worker_fn=worker_with_partial_ipc_message)
    assert time.monotonic() - started < 5
    assert report["counts"]["timeout"] == 1
    assert {child.pid for child in mp.active_children()} <= before
    assert not root.exists()


def test_completed_result_cannot_override_expired_deadline():
    """Even a valid result produced after the batch budget is not success."""
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 0.25}}, environ={})
    report = discover_all(config, catalog=(info,), worker_fn=worker_with_late_success)
    assert report["counts"]["timeout"] == 1
    assert report["counts"]["success"] == 0


def test_shared_batch_deadline_stops_hung_worker_within_five_seconds():
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 0.25}}, environ={})
    before = {child.pid for child in mp.active_children()}
    start = time.monotonic()
    report = discover_all(config, catalog=(info,), worker_fn=hanging_worker)
    elapsed = time.monotonic() - start
    assert elapsed < 5, elapsed
    assert report["counts"]["timeout"] == 1
    assert {child.pid for child in mp.active_children()} <= before


def test_shared_deadline_kills_worker_blocked_inside_http_transport():
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 0.25}}, environ={})
    before = {child.pid for child in mp.active_children()}
    started = time.monotonic()

    report = discover_all(config, catalog=(info,), worker_fn=worker_blocked_inside_http_transport)

    assert time.monotonic() - started < 5
    assert report["counts"]["timeout"] == 1
    assert {child.pid for child in mp.active_children()} <= before


def test_shared_deadline_kills_worker_blocked_inside_browser_navigation(tmp_path):
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    root = tmp_path / "browser-owned"
    config = load_config({
        "runtime": {"allow_network": True, "discovery_deadline": 0.25, "temp_root": str(root)},
    }, environ={})
    before = {child.pid for child in mp.active_children()}
    started = time.monotonic()

    report = discover_all(config, catalog=(info,), worker_fn=worker_blocked_inside_browser_navigation)

    assert time.monotonic() - started < 5
    assert report["counts"]["timeout"] == 1
    assert {child.pid for child in mp.active_children()} <= before
    assert not root.exists()


def test_shared_deadline_retains_completed_target_and_leaves_queued_target_unstarted():
    info = SourceInfo("fake", "fake", "1", (
        ProductInfo("a", variables=("v",)), ProductInfo("b", variables=("v",)),
        ProductInfo("c", variables=("v",)),
    ))
    config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 3}}, environ={})
    start = time.monotonic()
    report = discover_all(config, catalog=(info,), worker_fn=successful_first_then_hanging_worker)
    assert time.monotonic() - start < 8  # the three-second budget plus at most five seconds to reap
    assert report["counts"]["total"] == 3
    assert [(item["product"], item["status"]) for item in report["items"]] == [
        ("a", "success"), ("b", "timeout"), ("c", "not_started"),
    ]


def test_timeout_removes_parent_owned_worker_temp_root(tmp_path):
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 0.25, "temp_root": str(tmp_path / "batch")}}, environ={})
    report = discover_all(config, catalog=(info,), worker_fn=worker_with_owned_temp_resource)
    assert report["counts"]["timeout"] == 1
    assert not (tmp_path / "batch").exists()


def test_timeout_preserves_existing_caller_temp_root(tmp_path):
    existing = tmp_path / "caller-owned"
    existing.mkdir()
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 0.25,
                                      "temp_root": str(existing)}}, environ={})
    report = discover_all(config, catalog=(info,), worker_fn=worker_with_owned_temp_resource)
    assert report["counts"]["timeout"] == 1
    assert existing.is_dir()
    assert list(existing.iterdir()) == []


def test_interrupt_marks_running_target_cancelled_and_queued_not_started(monkeypatch):
    import radiust.discovery as discovery

    def interrupt(*_args):
        raise KeyboardInterrupt

    monkeypatch.setattr(discovery, "_run_isolated_target", interrupt)
    info = SourceInfo("fake", "fake", "1", (ProductInfo("a", variables=("v",)),
                                          ProductInfo("b", variables=("v",))))
    config = load_config({"runtime": {"allow_network": True}}, environ={})
    report = discover_all(config, catalog=(info,))
    assert report["interrupted"] is True
    assert report["counts"]["cancelled"] == 1
    assert report["counts"]["not_started"] == 1
    assert discovery.discovery_exit_code(report) == 130


@pytest.mark.skipif(os.name != "posix", reason="POSIX process groups required")
def test_timeout_terminates_worker_owned_subprocess(tmp_path):
    marker = tmp_path / "escaped"
    info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
    config = load_config({
        # Give the spawned Python child time to start under full-suite load;
        # otherwise the deadline can expire before this test reaches its
        # descendant-cleanup scenario at all.
        "runtime": {"allow_network": True, "discovery_deadline": 3},
        "sources": {"fake": {"marker": str(marker)}},
    }, environ={})
    result = discover_all(config, catalog=(info,), worker_fn=worker_with_subprocess)
    assert result["counts"]["timeout"] == 1
    assert marker.with_suffix(".pid").exists(), "worker never started its child"
    assert marker.with_suffix(".ready").exists(), "child did not install its SIGTERM handler"
    heartbeat = marker.with_suffix(".heartbeat")
    heartbeat_deadline = time.monotonic() + 1
    while not heartbeat.exists() and time.monotonic() < heartbeat_deadline:
        time.sleep(0.01)
    assert heartbeat.exists(), "child did not enter its heartbeat loop"
    stopped_at = heartbeat.read_text()
    time.sleep(0.12)
    assert heartbeat.read_text() == stopped_at, "worker descendant continued running after cancellation"
    assert not marker.exists(), "worker-owned subprocess reached its long-running continuation"


@pytest.mark.skipif(os.name != "posix", reason="real POSIX SIGINT required")
def test_real_sigint_reaps_running_worker_and_cleans_owned_temp_root(tmp_path):
    """Measure the parent process and its worker, not a mocked KeyboardInterrupt."""
    temp_root = tmp_path / "owned-by-discovery"
    script = """
import json
import sys
from radiust.config import load_config
from radiust.discovery import discover_all, discovery_exit_code
from radiust.models import ProductInfo, SourceInfo
from tests.integration.test_discovery_deadline import worker_with_owned_temp_resource

config = load_config({"runtime": {"allow_network": True, "discovery_deadline": 60,
                                  "temp_root": sys.argv[1]}}, environ={})
info = SourceInfo("fake", "fake", "1", (ProductInfo("p", variables=("v",)),))
report = discover_all(config, catalog=(info,), worker_fn=worker_with_owned_temp_resource)
print(json.dumps(report), flush=True)
sys.exit(discovery_exit_code(report))
"""
    proc = subprocess.Popen(
        [sys.executable, "-c", script, str(temp_root)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        start_new_session=True,
    )
    try:
        launch_deadline = time.monotonic() + 8
        while time.monotonic() < launch_deadline and proc.poll() is None:
            if any(temp_root.glob("worker-*/owned")):
                break
            time.sleep(0.02)
        else:
            raise AssertionError("isolated discovery worker did not reach its blocking phase")

        started = time.monotonic()
        proc.send_signal(signal.SIGINT)
        stdout, stderr = proc.communicate(timeout=5)
        assert time.monotonic() - started < 5
        assert proc.returncode == 130, (stdout, stderr)
        report = json.loads(stdout)
        assert report["interrupted"] is True
        assert report["counts"]["cancelled"] == 1
        assert report["counts"]["total"] == 1
        assert not temp_root.exists(), "discovery left behind its owned temporary root"
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.communicate(timeout=5)
