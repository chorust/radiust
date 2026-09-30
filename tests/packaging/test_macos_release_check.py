from __future__ import annotations

import json
from pathlib import Path

import pytest

from scripts.validation import check_macos_release


def test_arguments_require_wheel_and_explicit_architecture(tmp_path: Path, capsys) -> None:
    wheel = tmp_path / "radiust.whl"
    with pytest.raises(SystemExit) as error:
        check_macos_release.parse_args([str(wheel)])
    assert error.value.code == 2
    assert "--expected-architecture" in capsys.readouterr().err

    with pytest.raises(SystemExit) as error:
        check_macos_release.parse_args([])
    assert error.value.code == 2


def test_arguments_accept_supported_architecture_and_report_path(tmp_path: Path) -> None:
    wheel = tmp_path / "radiust.whl"
    report = tmp_path / "release.json"
    args = check_macos_release.parse_args(
        [
            str(wheel),
            "--expected-architecture",
            "arm64",
            "--expected-macos-version",
            "11.0",
            "--report",
            str(report),
        ]
    )
    assert args.wheel == wheel
    assert args.expected_architecture == "arm64"
    assert args.expected_macos_version == "11.0"
    assert args.report == report


def test_arguments_can_enable_isolated_format_readback(tmp_path: Path) -> None:
    args = check_macos_release.parse_args(
        [
            str(tmp_path / "radiust.whl"),
            "--expected-architecture",
            "arm64",
            "--with-format-readback",
        ]
    )
    assert args.with_format_readback is True


def test_arguments_reject_unknown_architecture(tmp_path: Path, capsys) -> None:
    with pytest.raises(SystemExit) as error:
        check_macos_release.parse_args(
            [str(tmp_path / "radiust.whl"), "--expected-architecture", "universal2"]
        )
    assert error.value.code == 2
    assert "invalid choice" in capsys.readouterr().err


@pytest.mark.parametrize(
    ("system", "machine", "expected", "error"),
    [
        ("Linux", "aarch64", "arm64", "macOS is required"),
        ("Darwin", "x86_64", "arm64", "does not match"),
        ("Darwin", "arm64", "arm64", None),
        ("Darwin", "aarch64", "arm64", None),
    ],
)
def test_host_validation_is_independent_of_current_platform(
    system: str, machine: str, expected: str, error: str | None
) -> None:
    result = check_macos_release.host_error(system, machine, expected)
    if error is None:
        assert result is None
    else:
        assert result is not None
        assert error in result


def test_clean_environment_drops_developer_proj_and_gdal_overrides(
    monkeypatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("PROJ_LIB", "/opt/homebrew/share/proj")
    monkeypatch.setenv("GDAL_DATA", "/usr/local/opt/gdal/share/gdal")

    environment = check_macos_release._clean_environment(tmp_path)

    assert "PROJ_LIB" not in environment
    assert "GDAL_DATA" not in environment


def test_wheel_macos_deployment_version_is_parsed_from_platform_tags() -> None:
    tags = [
        "cp312-cp312-macosx_11_0_arm64",
        "cp312-abi3-macosx_12_0_arm64",
        "py3-none-any",
    ]
    assert check_macos_release._wheel_macos_versions(tags) == {(11, 0), (12, 0)}


def test_non_macos_audit_fails_closed_before_reading_wheel(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(check_macos_release.platform, "system", lambda: "Linux")
    monkeypatch.setattr(check_macos_release.platform, "machine", lambda: "aarch64")
    args = check_macos_release.parse_args(
        [str(tmp_path / "missing.whl"), "--expected-architecture", "arm64"]
    )

    report = check_macos_release.run_audit(args)

    assert report["status"] == "failed"
    assert report["checks"]["host"]["passed"] is False
    assert report["errors"][0]["check"] == "host"
    assert report["checks"].get("wheel") is None


def test_report_serialization_is_stable_and_does_not_claim_format_readback(tmp_path: Path) -> None:
    report = check_macos_release.new_report(
        Path("radiust.whl"), "arm64", "Darwin", "arm64", "11.0"
    )
    check_macos_release.record_check(report, "host", True, details={"machine": "arm64"})
    check_macos_release.record_check(
        report, "wheel", False, error="wheel architecture did not match"
    )
    check_macos_release.finish_report(report)

    serialized = check_macos_release.serialize_report(report)
    assert serialized == check_macos_release.serialize_report(report)
    assert serialized.endswith("\n")
    decoded = json.loads(serialized)
    assert decoded["status"] == "failed"
    assert decoded["scope"]["format_readback"] == "not_performed"
    assert decoded["errors"] == [
        {"check": "wheel", "message": "wheel architecture did not match"}
    ]

    output = tmp_path / "nested" / "audit.json"
    check_macos_release.write_report(output, report)
    assert output.read_text(encoding="utf-8") == serialized
