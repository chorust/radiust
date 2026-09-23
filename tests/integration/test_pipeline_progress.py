"""Progress callbacks describe completed work without requiring a terminal."""

from datetime import datetime, timezone

from radiust import Client, Query

QUERY = Query("my", at=datetime(2025, 12, 29, 6, 50, 1, tzinfo=timezone.utc))


def test_download_progress_reflects_real_single_frame_stages(tmp_path):
    events = []
    with Client(config={"cache": {"enabled": False}}) as client:
        report = client.download(
            QUERY, output=tmp_path, raw_only=True,
            progress=lambda stage, completed, total: events.append((stage, completed, total)),
        )
    assert report.counts["written"] == 1
    assert ("discover", 1, 1) in events
    assert ("acquire", 1, 1) in events
    assert ("commit", 1, 1) in events
    assert ("download", 1, 1) in events
    assert all(stage != "decode" for stage, _, _ in events)
    assert events.index(("acquire", 1, 1)) < events.index(("commit", 1, 1))


def test_scientific_fetch_emits_real_discover_acquire_decode_stages():
    events = []
    with Client(config={"cache": {"enabled": False}}) as client:
        field = client.fetch(QUERY, progress=lambda stage, completed, total: events.append((stage, completed, total)))
    assert field is not None
    assert [(stage, completed) for stage, completed, _ in events if completed == 1] == [
        ("discover", 1), ("acquire", 1), ("decode", 1),
    ]


def test_unchanged_sdk_calls_without_callbacks_emit_no_progress(tmp_path, capsys):
    with Client(config={"cache": {"enabled": False}}) as client:
        report = client.download(QUERY, output=tmp_path, raw_only=True)
    assert report.counts["written"] == 1
    captured = capsys.readouterr()
    assert captured.out == captured.err == ""


def test_batch_fetch_progress_has_real_resolve_and_completion_counts():
    events = []
    with Client(config={"cache": {"enabled": False}}) as client:
        result = client.fetch_many(
            QUERY, progress=lambda stage, completed, total: events.append((stage, completed, total)),
        )
    assert len(result.succeeded) == 1
    assert ("resolve", 1, None) in events
    assert ("fetch", 0, 1) in events
    assert ("fetch", 1, 1) in events


def test_cli_download_receives_real_pipeline_stages_before_report(monkeypatch, tmp_path):
    from click.testing import CliRunner
    from radiust.cli.main import main

    events = []
    closed = []

    class ProgressRecorder:
        def __init__(self, **_kwargs):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *_args):
            closed.append(True)

        def update(self, event):
            events.append((event.stage, event.completed, event.total))

    monkeypatch.setattr("radiust.cli.progress.Progress", ProgressRecorder)
    result = CliRunner().invoke(main, [
        "download", "my", "--at", "2025-12-29T06:50:01Z", "--raw-only",
        "--no-cache", "--output", str(tmp_path / "out"), "--json",
    ])
    assert result.exit_code == 0, result.output
    assert ("discover", 1, 1) in events
    assert ("acquire", 1, 1) in events
    assert ("commit", 1, 1) in events
    assert events[-1] == ("download", 1, 1)
    assert all(stage != "decode" for stage, _, _ in events)
    assert closed == [True]
    import json

    assert json.loads(result.output)["counts"]["written"] == 1
