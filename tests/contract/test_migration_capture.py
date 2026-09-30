from __future__ import annotations

from scripts.validation.capture_migration_contracts import (
    FIXTURES,
    ROOT,
    _normalize,
)


def test_migration_capture_writes_python_baselines_separately_from_native_cli_goldens() -> None:
    assert FIXTURES == ROOT / "tests/fixtures/rust-migration/cli/python-baseline"


def test_migration_capture_redacts_secret_fields_and_unstructured_text(tmp_path) -> None:
    normalized = _normalize(
        {
            "run_id": "0123456789abcdef0123456789abcdef",
            "storage": {
                "access_key": "access-value",
                "secret_key": "secret-value",
                "credentials": {"user": "alice", "password": "password-value"},
            },
            "endpoint": "https://alice:password-value@example.test/api?token=query-value",
            "stderr": "Authorization: Bearer bearer-value password=inline-value",
            "path": str(tmp_path / "cache"),
        },
        tmp_path,
    )

    assert normalized["run_id"] == "<RUN_ID>"
    assert normalized["storage"]["access_key"] == "[REDACTED]"
    assert normalized["storage"]["secret_key"] == "[REDACTED]"
    assert normalized["storage"]["credentials"] == "[REDACTED]"
    assert normalized["endpoint"] == "https://alice:<REDACTED>@example.test/api?token=<REDACTED>"
    assert normalized["stderr"] == "Authorization: Bearer <REDACTED> password=<REDACTED>"
    assert normalized["path"] == "<TEMP>/cache"


def test_migration_capture_records_legacy_empty_stop_and_interrupt_boundaries(tmp_path) -> None:
    import json

    empty = json.loads((FIXTURES / "discover-single-empty.json").read_text())
    stop = json.loads((FIXTURES / "download-stop.json").read_text())
    interrupted = json.loads((FIXTURES / "download-interrupt.json").read_text())

    assert empty["exit_code"] == 0
    assert empty["stdout"]["items"] == []
    assert stop["exit_code"] == 2
    assert stop["stdout"]["error"]["code"] == "batch_error"
    assert stop["stdout"]["items"] == []
    assert interrupted["exit_code"] == 1
    assert interrupted["stdout"] == ""
    assert interrupted["stderr"] == "Aborted!"
