
import os
from pathlib import Path

import pytest
from click.testing import CliRunner
from radiust.cli.main import main


def _successful_isolated_preview_worker(pipe, query, config, root, _apply_legacy=False):
    from PIL import Image
    from radiust.discovery_worker import send_message

    assert set(config.values["sources"]) == {query.source}
    assert config.values["cache"]["enabled"] is False
    assert config.values["storage"]["access_key"] is None
    Image.new("RGBA", (2, 1), (7, 8, 9, 255)).save(Path(root) / "preview.png", format="PNG")
    send_message(pipe, {
        "kind": "preview", "file_identity": "sample.gif", "format": "GIF",
        "sha256": "a" * 64, "source": query.source, "product": "composite",
        "station": "sample", "valid_time": "2026-09-22T00:00:00Z",
        "frame_index": 0, "display_mode": "original", "rule_version": None,
        "reason": "original source pixels preserved", "width": 2, "height": 1,
    })


def _stalled_isolated_preview_worker(pipe, _query, _config, root, _apply_legacy=False):
    import os
    import signal
    import time
    from pathlib import Path

    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    Path(root, "started").write_text(str(os.getpid()), encoding="utf-8")
    while True:
        time.sleep(1)


def test_source_raw_preview_preserves_verified_bytes_and_raw_ownership(tmp_path):
    import hashlib
    from datetime import datetime, timezone

    from radiust.display.raw import preview_raw
    from radiust.models import Artifact, FrameRef
    from radiust.raw import RawFrame

    from tests.support.cli_experience import image_bytes

    payload = image_bytes()
    source = tmp_path / "unchanged.png"
    source.write_bytes(payload)
    ref = FrameRef("th", "composite", datetime(2026, 9, 22, tzinfo=timezone.utc), station="a")
    raw = RawFrame(ref, (Artifact("original.png", "data", "image/png", source,
                                  size_bytes=len(payload), sha256=hashlib.sha256(payload).hexdigest()),))
    try:
        preview = preview_raw(raw, limits={"max_artifact_bytes": len(payload), "max_pixels": 6,
                                           "max_temp_bytes": 48})
        assert (preview.source, preview.product, preview.station) == ("th", "composite", "a")
        assert preview.valid_time == "2026-09-22T00:00:00Z"
        assert preview.sha256 == hashlib.sha256(payload).hexdigest()
        preview.rgba[:] = 0
        assert source.read_bytes() == payload
        assert not raw.closed
        assert raw.bytes() == payload
    finally:
        raw.close()


def test_raw_preview_rejects_unverified_receipt(tmp_path):
    from datetime import datetime, timezone

    import pytest
    from radiust.display.raw import preview_raw
    from radiust.errors import IntegrityError
    from radiust.models import Artifact, FrameRef
    from radiust.raw import RawFrame

    from tests.support.cli_experience import image_bytes

    path = tmp_path / "image.png"
    path.write_bytes(image_bytes())
    ref = FrameRef("th", "composite", datetime(2026, 9, 22, tzinfo=timezone.utc))
    with pytest.raises(IntegrityError):
        preview_raw(RawFrame(ref, (Artifact("image.png", "data", "image/png", path,
                                            sha256="0" * 64),)))


def test_replayed_raw_manifest_preview_does_not_mutate_manifest_or_artifact(tmp_path):
    import hashlib
    import json
    from datetime import datetime, timezone

    from radiust.display.raw import preview_raw
    from radiust.models import Artifact, FrameRef
    from radiust.raw import RawFrame, raw_manifest
    from radiust.raw_replay import load_raw

    from tests.support.cli_experience import image_bytes

    payload = image_bytes()
    path = tmp_path / "source.png"
    path.write_bytes(payload)
    ref = FrameRef("th", "composite", datetime(2026, 9, 22, tzinfo=timezone.utc))
    raw = RawFrame(ref, (Artifact("source.png", "data", "image/png", path),))
    try:
        manifest = tmp_path / "raw-manifest.json"
        manifest.write_text(json.dumps(raw_manifest(raw)), encoding="utf-8")
    finally:
        raw.close()

    before_manifest = manifest.read_bytes()
    with load_raw(manifest) as replayed:
        preview = preview_raw(replayed)
        assert preview.sha256 == hashlib.sha256(payload).hexdigest()
        preview.rgba[:] = 0
        assert replayed.bytes() == payload
    assert path.read_bytes() == payload
    assert manifest.read_bytes() == before_manifest


def test_source_raw_preview_detects_path_changed_after_acquisition(tmp_path):
    from datetime import datetime, timezone

    import pytest
    from radiust.display.raw import preview_raw
    from radiust.errors import IntegrityError
    from radiust.models import Artifact, FrameRef
    from radiust.raw import RawFrame

    from tests.support.cli_experience import image_bytes

    path = tmp_path / "source.png"
    path.write_bytes(image_bytes())
    ref = FrameRef("th", "composite", datetime(2026, 9, 22, tzinfo=timezone.utc))
    with RawFrame(ref, (Artifact("source.png", "data", "image/png", path),)) as raw:
        path.write_bytes(image_bytes() + b"changed")
        with pytest.raises(IntegrityError, match="receipt mismatch"):
            preview_raw(raw)


def test_preview_enforces_pixel_and_working_memory_limits_before_decode():
    import pytest
    from radiust.display.raw import preview_bytes
    from radiust.errors import ResourceLimitError

    from tests.support.cli_experience import image_bytes

    payload = image_bytes()
    with pytest.raises(ResourceLimitError, match="pixel limit"):
        preview_bytes(payload, name="original.png", limits={"max_pixels": 1})
    with pytest.raises(ResourceLimitError, match="temporary byte limit"):
        preview_bytes(payload, name="original.png", limits={"max_temp_bytes": 7})
    with pytest.raises(ResourceLimitError, match="artifact byte limit"):
        preview_bytes(payload, name="original.png", limits={"max_artifact_bytes": len(payload) - 1})


def test_bad_image_is_bounded_and_does_not_change_existing_files(tmp_path):
    existing = tmp_path / "existing.raw"
    existing.write_bytes(b"keep")
    bad = tmp_path / "broken.png"
    bad.write_bytes(b"not a real PNG")
    result = CliRunner().invoke(main, ["cat", "--file", str(bad), "--renderer", "text"])
    assert result.exit_code != 0
    assert existing.read_bytes() == b"keep"
    assert bad.read_bytes() == b"not a real PNG"


def test_more_than_one_raw_artifact_does_not_pick_first(monkeypatch):
    import json
    from datetime import datetime, timezone

    from radiust.client import Client
    from radiust.config import load_config
    from radiust.display.isolated import _preview_worker
    from radiust.models import Artifact, FrameRef, Query
    from radiust.raw import RawFrame

    ref = FrameRef("th", "composite", datetime(2025, 1, 1, tzinfo=timezone.utc), station="cmp1")
    raw = RawFrame(ref, (
        Artifact("a.png", "data", "image/png", b"first"),
        Artifact("b.png", "data", "image/png", b"second"),
    ))

    class Acquire:
        def __enter__(self):
            return raw

        def __exit__(self, *_args):
            raw.close()

    monkeypatch.setattr(Client, "discover", lambda *_args, **_kwargs: [ref])
    monkeypatch.setattr(Client, "acquire", lambda *_args, **_kwargs: Acquire())

    class Pipe:
        def __init__(self):
            self.messages = []

        def send_bytes(self, payload):
            self.messages.append(json.loads(payload))

    pipe = Pipe()
    _preview_worker(pipe, Query("th", latest=True), load_config(environ={}), Path.cwd())
    assert "artifact" in pipe.messages[-1]["message"].lower()
    assert raw.closed


def test_raw_preview_rejects_acquired_frame_identity_change(monkeypatch, tmp_path):
    import json

    from radiust.client import Client
    from radiust.config import load_config
    from radiust.display.isolated import _preview_worker
    from radiust.models import Artifact, Query
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame, image_bytes

    selected = frame(station="a", hour=0)
    replaced = frame(station="a", hour=1)
    acquired = RawFrame(replaced, (Artifact("frame.png", "data", "image/png", image_bytes()),))

    class Acquire:
        def __enter__(self):
            return acquired

        def __exit__(self, *_args):
            acquired.close()

    monkeypatch.setattr(Client, "discover", lambda *_args, **_kwargs: [selected])
    monkeypatch.setattr(Client, "acquire", lambda *_args, **_kwargs: Acquire())

    class Pipe:
        def __init__(self):
            self.messages = []

        def send_bytes(self, payload):
            self.messages.append(json.loads(payload))

    pipe = Pipe()
    _preview_worker(pipe, Query("th", latest=True), load_config(environ={}), tmp_path)
    assert "identity" in pipe.messages[-1]["message"].lower()
    assert acquired.closed


def test_isolated_raw_worker_previews_acquired_bytes_without_scientific_paths(monkeypatch, tmp_path):
    import json

    from radiust.client import Client
    from radiust.config import load_config
    from radiust.display.isolated import _preview_worker
    from radiust.models import Artifact, Query
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame, image_bytes

    ref = frame(station="a")
    payload = image_bytes()
    raw = RawFrame(ref, (Artifact("frame.png", "data", "image/png", payload),))

    class Acquire:
        def __enter__(self):
            return raw

        def __exit__(self, *_args):
            raw.close()

    class Pipe:
        def __init__(self):
            self.messages = []

        def send_bytes(self, payload):
            self.messages.append(json.loads(payload))

    monkeypatch.setattr(Client, "discover", lambda *_args, **_kwargs: [ref])
    monkeypatch.setattr(Client, "acquire", lambda *_args, **_kwargs: Acquire())
    monkeypatch.setattr(Client, "fetch", lambda *_args, **_kwargs: pytest.fail("scientific fetch called"))
    monkeypatch.setattr(Client, "decode", lambda *_args, **_kwargs: pytest.fail("scientific decode called"))
    pipe = Pipe()

    _preview_worker(pipe, Query("th", latest=True), load_config(environ={}), tmp_path)

    assert pipe.messages[-1]["kind"] == "preview"
    assert pipe.messages[-1]["sha256"] == raw.receipts[0].sha256
    assert pipe.messages[-1]["source"] == "th" and pipe.messages[-1]["station"] == "a"
    assert raw.closed


def test_local_raw_preview_display_interrupt_returns_130_and_preserves_file(monkeypatch, tmp_path):
    from tests.support.cli_experience import image_bytes

    payload = image_bytes()
    source = tmp_path / "original.png"
    source.write_bytes(payload)

    def interrupted_display(*_args, **_kwargs):
        raise KeyboardInterrupt

    monkeypatch.setattr("radiust.cli.cat.show", interrupted_display)
    result = CliRunner().invoke(main, ["cat", "--file", str(source), "--renderer", "text"])
    assert result.exit_code == 130
    assert source.read_bytes() == payload


def test_source_raw_preview_display_interrupt_closes_acquired_raw(monkeypatch):
    from radiust.display.raw import preview_raw
    from radiust.models import Artifact
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame, image_bytes

    ref = frame()
    raw = RawFrame(ref, (Artifact("original.png", "data", "image/png", image_bytes()),))

    def source_preview(*_args, **_kwargs):
        try:
            return preview_raw(raw)
        finally:
            raw.close()

    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", source_preview)

    def interrupted_display(*_args, **_kwargs):
        raise KeyboardInterrupt

    monkeypatch.setattr("radiust.cli.cat.show", interrupted_display)
    result = CliRunner().invoke(main, ["cat", ref.source, "--raw", "--renderer", "text"])
    assert result.exit_code == 130
    assert raw.closed


def test_source_raw_preview_acquisition_interrupt_cleans_operation_temp_root(monkeypatch, tmp_path):
    from datetime import datetime, timezone

    import pytest
    from radiust import pipeline
    from radiust.config import load_config
    from radiust.display.isolated import _preview_worker
    from radiust.models import FrameRef, Query

    ref = FrameRef("th", "composite", datetime.now(timezone.utc))
    temp_root = tmp_path / "operation-temp"
    cache_root = tmp_path / "cache"
    output_root = tmp_path / "output"
    cache_root.mkdir()
    output_root.mkdir()
    cache_sentinel = cache_root / "existing.bin"
    output_sentinel = output_root / "existing.bin"
    cache_sentinel.write_bytes(b"cache-owned-by-user")
    output_sentinel.write_bytes(b"output-owned-by-user")
    config = load_config({
        "runtime": {"temp_root": str(temp_root)},
        "cache": {"dir": str(cache_root)},
        "storage": {"output": str(output_root)},
    }, environ={})

    class Source:
        async def discover(self, _query, _context):
            return [ref]

        async def download(self, _ref, context):
            (context.temp_root / "partial-artifact").write_bytes(b"partial")
            raise KeyboardInterrupt

    monkeypatch.setattr(pipeline, "get_source", lambda _source: Source())
    class Pipe:
        def __init__(self):
            self.messages = []

        def send_bytes(self, payload):
            self.messages.append(payload)

        def close(self):
            pass

    with pytest.raises(KeyboardInterrupt):
        _preview_worker(Pipe(), Query("th", latest=True), config, temp_root)
    assert not temp_root.exists() or not list(temp_root.iterdir())
    assert cache_sentinel.read_bytes() == b"cache-owned-by-user"
    assert output_sentinel.read_bytes() == b"output-owned-by-user"


def test_isolated_preview_restores_rgba_and_preserves_existing_temp_root(tmp_path):
    from radiust.config import load_config
    from radiust.display.isolated import preview_source_raw
    from radiust.models import Query

    configured_root = tmp_path / "caller-temp"
    configured_root.mkdir()
    sentinel = configured_root / "keep.bin"
    sentinel.write_bytes(b"caller-owned")
    config = load_config({
        "runtime": {"temp_root": str(configured_root)},
        "cache": {"enabled": True},
        "storage": {"access_key": "secret-to-exclude", "secret_key": "secret-to-exclude-too"},
        "sources": {"th": {"token": "source-private"}, "ph": {"token": "other-source"}},
    }, environ={})

    preview = preview_source_raw(
        Query("th", latest=True), config, worker=_successful_isolated_preview_worker,
    )

    assert preview.file_identity == "sample.gif"
    assert preview.format == "GIF" and preview.frame_index == 0
    assert preview.rgba.shape == (1, 2, 4)
    assert preview.rgba[0, 0].tolist() == [7, 8, 9, 255]
    assert sentinel.read_bytes() == b"caller-owned"
    assert sorted(path.name for path in configured_root.iterdir()) == ["keep.bin"]


@pytest.mark.skipif(os.name != "posix", reason="requires POSIX process groups and SIGINT")
def test_source_raw_sigint_kills_blocked_worker_and_cleans_only_owned_temp_root(tmp_path):
    import signal
    import subprocess
    import sys
    import time

    configured_root = tmp_path / "owned-temp"
    cache_root = tmp_path / "cache"
    output_root = tmp_path / "output"
    cache_root.mkdir()
    output_root.mkdir()
    cache_sentinel = cache_root / "keep.bin"
    output_sentinel = output_root / "keep.bin"
    cache_sentinel.write_bytes(b"cache-owned")
    output_sentinel.write_bytes(b"output-owned")
    config_path = tmp_path / "config.yml"
    config_path.write_text(
        "runtime:\n  temp_root: " + str(configured_root) +
        "\ncache:\n  dir: " + str(cache_root) +
        "\nstorage:\n  output: " + str(output_root) + "\n",
        encoding="utf-8",
    )
    script = tmp_path / "interrupt_raw_cli.py"
    script.write_text(
        """import sys
from pathlib import Path
sys.path.insert(0, sys.argv[2])
from click.testing import CliRunner
import radiust.cli.cat as cat
from radiust.cli.main import main
from radiust.display.isolated import preview_source_raw
from tests.integration.test_raw_preview_lifecycle import _stalled_isolated_preview_worker

cat.preview_source_raw = lambda query, config, progress=None: preview_source_raw(
    query, config, progress=progress, worker=_stalled_isolated_preview_worker)
def run():
    result = CliRunner().invoke(main, ['--conf', sys.argv[1], 'cat', 'th', '--raw', '--renderer', 'text'])
    sys.stderr.write(result.output)
    raise SystemExit(result.exit_code)

if __name__ == '__main__':
    run()
""",
        encoding="utf-8",
    )

    process = subprocess.Popen(
        [sys.executable, str(script), str(config_path), str(Path(__file__).resolve().parents[2])],
        cwd=Path(__file__).resolve().parents[2],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        deadline = time.monotonic() + 8
        marker = None
        while time.monotonic() < deadline and process.poll() is None:
            matches = list(configured_root.glob("worker-*/started"))
            if matches:
                marker = matches[0]
                break
            time.sleep(0.02)
        if marker is None:
            stdout, stderr = process.communicate(timeout=2)
            pytest.fail(f"raw preview worker did not start: exit={process.returncode}, stdout={stdout!r}, stderr={stderr!r}")
        worker_pid = int(marker.read_text(encoding="utf-8"))
        started = time.monotonic()
        process.send_signal(signal.SIGINT)
        stdout, stderr = process.communicate(timeout=5)
        assert process.returncode == 130, (stdout, stderr)
        assert time.monotonic() - started < 5
        assert not configured_root.exists()
        with pytest.raises(ProcessLookupError):
            os.kill(worker_pid, 0)
        assert cache_sentinel.read_bytes() == b"cache-owned"
        assert output_sentinel.read_bytes() == b"output-owned"
    finally:
        if process.poll() is None:
            process.kill()
            process.communicate(timeout=2)
