import hashlib
import json
from dataclasses import asdict
from io import BytesIO
from pathlib import Path

import pytest
from click.testing import CliRunner
from PIL import Image
from radiust.cli.main import main
from radiust.display.engine import DisplayBlockedError
from radiust.display.raw import preview_raw
from radiust.display.registry import LegacyDisplayRegistry, default_registry
from radiust.display.rules import rule_fingerprint

PNG = Path("tests/fixtures/sources/th_royalrain/raw/takhli.png")
GIF = Path("tests/fixtures/sources/th/raw/kkn240Loop.gif")
AU_PNG = Path("tests/fixtures/sources/au/raw/IDR021.T.202609180511.png")


def test_file_preview_rejects_oversized_input_before_decoding(tmp_path):
    from radiust.cli.cat import _read_preview_file

    path = tmp_path / "oversized.png"
    path.write_bytes(b"12345")
    try:
        _read_preview_file(path, 4)
    except Exception as exc:
        assert "byte limit" in str(exc)
    else:
        raise AssertionError("oversized input must be rejected")


def test_local_png_gif_text_is_explicit_raw_and_first_frame():
    for path in (PNG, GIF):
        result = CliRunner().invoke(main, ["cat", "--file", str(path), "--renderer", "text"])
        assert result.exit_code == 0, result.output
        assert "raw" in result.output
        assert "size=" in result.output
        assert "time=unknown" in result.output
    assert "first frame" in CliRunner().invoke(main, ["cat", "--file", str(GIF), "--renderer", "text"]).output


def test_source_raw_works_without_scientific_decoder(monkeypatch):
    from radiust.models import Artifact
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame

    ref = frame(station="cmp1")
    raw = RawFrame(ref, (Artifact("cmp1.gif", "data", "image/gif", Path("tests/fixtures/sources/th/raw/cmp1.gif").read_bytes()),))
    preview = preview_raw(raw)
    raw.close()
    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", lambda *_args, **_kwargs: preview)
    result = CliRunner().invoke(main, ["cat", "th", "--raw", "--station", "cmp1", "--renderer", "text"])
    assert result.exit_code == 0, result.output
    assert "source=th" in result.output
    assert "station=cmp1" in result.output
    assert "raw" in result.output
    assert "scientific" not in result.output.lower() or "not scientifically" in result.output.lower()


def test_au_raw_preview_keeps_original_provider_pixels():
    from radiust.models import Artifact
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame

    payload = AU_PNG.read_bytes()
    with Image.open(BytesIO(payload)) as image:
        original = image.convert("RGBA").tobytes()
    raw = RawFrame(
        frame(source="au", station="AU02"),
        (Artifact(AU_PNG.name, "data", "image/png", payload),),
    )
    with raw:
        preview = preview_raw(raw)
        assert preview.display_mode == "original"
        assert preview.rule_version is None
        assert preview.rgba.tobytes() == original


def test_raw_rejects_options_and_non_tty_before_acquisition(monkeypatch):
    calls = []
    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", lambda *args, **kwargs: calls.append("preview"))
    base = ["cat", "th", "--raw", "--station", "cmp1"]
    for extras in (["--renderer", "auto"], ["--renderer", "ansi"], ["--renderer", "text", "--variable", "dbz"]):
        result = CliRunner().invoke(main, base + extras)
        assert result.exit_code == 2, result.output
    assert not calls
    assert CliRunner().invoke(main, ["cat", "--file", str(PNG), "--raw", "--renderer", "text"]).exit_code == 2
    assert CliRunner().invoke(main, ["cat", "--file", str(PNG), "--decoded", "--renderer", "text"]).exit_code == 2
    assert CliRunner().invoke(main, ["cat", "th", "--raw", "--legacy-display", "--renderer", "text"]).exit_code == 2


def test_source_raw_ambiguous_preserves_station_choices(monkeypatch):
    from radiust.display.isolated import RawPreviewAmbiguousError

    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", lambda *_: (_ for _ in ()).throw(
        RawPreviewAmbiguousError([
            {"product": "composite", "station": "cmp1", "valid_time": "2026-09-22T00:00:00Z"},
            {"product": "composite", "station": "kkn240Loop", "valid_time": "2026-09-22T00:00:00Z"},
        ])))
    result = CliRunner().invoke(main, ["cat", "th", "--raw", "--renderer", "text"])
    assert result.exit_code != 0
    assert "station" in result.output.lower()


def test_source_raw_omitted_time_and_explicit_latest_use_same_query(monkeypatch):
    from tests.support.cli_experience import image_bytes

    queries = []

    def capture(query, _config, **_kwargs):
        queries.append(query)
        from radiust.display.raw import preview_bytes
        return preview_bytes(image_bytes(), name="frame.png", source=query.source)

    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", capture)
    runner = CliRunner()
    implicit = runner.invoke(main, ["cat", "th", "--raw", "--renderer", "text"])
    explicit = runner.invoke(main, ["cat", "th", "--raw", "--latest", "--renderer", "text"])
    assert implicit.exit_code == explicit.exit_code == 0, (implicit.output, explicit.output)
    assert len(queries) == 2
    assert queries[0] == queries[1]
    assert queries[0].latest and queries[0].at is None


@pytest.mark.parametrize("option", [
    ["--variable", "reflectivity"], ["--palette", "viridis"],
    ["--vmin", "0"], ["--vmax", "0"],
])
def test_raw_science_options_are_rejected_even_when_equal_to_defaults(monkeypatch, option):
    calls = []
    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", lambda *_args, **_kwargs: calls.append("preview"))
    response = CliRunner().invoke(main, ["cat", "th", "--raw", "--renderer", "text", *option])
    assert response.exit_code == 2
    assert "scientific" in response.output.lower()
    assert calls == []


def test_synthetic_gif_file_previews_first_frame_and_preserves_unknown_metadata(tmp_path):
    from radiust.display.raw import preview_bytes

    from tests.support.cli_experience import animated_gif_bytes

    payload = animated_gif_bytes()
    source = tmp_path / "unknown.gif"
    source.write_bytes(payload)
    value = preview_bytes(payload, name=source.name)
    assert value.frame_index == 0
    assert tuple(value.rgba[0, 0]) == (1, 2, 3, 255)
    assert value.source is value.product is value.station is value.valid_time is None
    output = CliRunner().invoke(main, ["cat", "--file", str(source), "--renderer", "text"])
    assert output.exit_code == 0, output.output
    assert "first frame (index=0)" in output.output
    assert "time=unknown" in output.output and "units=unknown" in output.output


def _synthetic_registry(root: Path):
    """No generated bytes or claimed evidence here belong to a real source."""
    rule = {
        "source": "th", "product": "composite", "path_id": "th/composite/cmp1",
        "rule_version": "synthetic-v1", "encoding_version": "旧项目-gray-dbz-v1",
        "legacy_reference": "synthetic test-only rule, never a Thailand historical baseline",
        "ordered_steps": [{"op": "palette", "entries": [{"rgb": [1, 2, 3], "dbz": 5}], "basis": "synthetic"},
                          {"op": "gray_encode", "basis": "synthetic"}],
        "input_constraints": {"formats": ["PNG"], "max_pixels": 16, "station": "cmp1"},
        "validation_status": "passed",
    }
    rule["config_hash"] = rule_fingerprint(rule)
    digest = hashlib.sha256(b"synthetic-fixture-only").hexdigest()
    evidence = {
        "path_id": rule["path_id"], "status": "passed", "rule_version": rule["rule_version"],
        "config_hash": rule["config_hash"], "input_hashes": [digest], "output_hash": digest,
        "baseline_identity": "synthetic-only", "baseline_hash": digest,
        "crop": [0, 0, 1, 1], "shape": [1, 1], "pixel_diff_count": 0,
        "alpha_comparison": "identical", "background_comparison": "identical",
        "missing_comparison": "identical", "intentional_differences": [],
        "review_conclusion": "synthetic test only", "sample_provenance": "synthetic unit test",
        "blocked_reasons": [], "scientific_status_unchanged": True,
    }
    index = {"schema_version": 1, "paths": [
        {"source": "th", "product": "composite", "path_id": rule["path_id"],
         "status": "passed", "rule_version": rule["rule_version"],
         "config_hash": rule["config_hash"], "rule_file": "rule.json", "evidence_file": "evidence.json"},
    ]}
    for name, value in (("rule.json", rule), ("evidence.json", evidence), ("index.json", index)):
        (root / name).write_text(json.dumps(value), encoding="utf-8")
    buffer = BytesIO()
    Image.new("RGBA", (1, 1), (1, 2, 3, 255)).save(buffer, format="PNG")
    return LegacyDisplayRegistry(root), buffer.getvalue()


def test_builtin_display_registry_enables_only_exact_old_code_replay_rules():
    registry = default_registry()
    passed = {path_id for path_id, item in registry._entries.items() if item["status"] == "passed"}
    assert passed == {
        "au/composite", "ca/rain", "es/composite", "fr/composite", "id/composite",
        "kr/composite", "my/composite/east", "my/composite/peninsular", "nz/rain", "pt/composite", "sg/composite",
        "th/composite/kkn240Loop", "th_royalrain/cappi", "tw/observation", "vn/cmax",
    }
    assert registry._entries["th/composite/cmp1"]["status"] == "blocked"


def test_raw_preview_preserves_source_pixels_and_legacy_display_is_opt_in(tmp_path):
    from radiust.models import Artifact
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame

    registry, payload = _synthetic_registry(tmp_path)
    with RawFrame(frame(station="cmp1"), (Artifact("a.png", "data", "image/png", payload),)) as raw:
        original = raw.bytes()
        preview = preview_raw(raw, registry=registry)
        assert preview.display_mode == "original" and preview.rule_version is None
        assert preview.rgba[0, 0].tolist() == [1, 2, 3, 255]
        legacy = preview_raw(raw, registry=registry, apply_legacy=True)
        assert legacy.display_mode == "legacy" and legacy.rule_version == "synthetic-v1"
        assert legacy.rgba[0, 0].tolist() == [16, 16, 16, 255]
        assert preview.sha256 == hashlib.sha256(original).hexdigest()
        assert "pixels preserved" in (preview.reason or "")
        assert raw.bytes() == original


def test_synthetic_legacy_cli_rule_and_file_mode_boundaries(tmp_path, monkeypatch):
    from radiust.models import Artifact
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame

    registry, payload = _synthetic_registry(tmp_path)
    monkeypatch.setattr("radiust.display.raw.default_registry", lambda: registry)
    def source_preview(*_args, **kwargs):
        raw = RawFrame(frame(station="cmp1"), (Artifact("a.png", "data", "image/png", payload),))
        try:
            return preview_raw(raw, apply_legacy=bool(kwargs.get("apply_legacy", False)))
        finally:
            raw.close()
    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", source_preview)
    response = CliRunner().invoke(main, ["cat", "th", "--station", "cmp1", "--renderer", "text"])
    assert response.exit_code == 0, response.output
    assert "display=original" in response.output and "rule=unknown" in response.output
    legacy = CliRunner().invoke(main, ["cat", "th", "--legacy-display", "--station", "cmp1", "--renderer", "text"])
    assert legacy.exit_code == 0, legacy.output
    assert "display=legacy" in legacy.output and "rule=synthetic-v1" in legacy.output
    assert "units=unknown" in legacy.output and "not a scientifically validated field" in legacy.output
    file = tmp_path / "independent.png"
    file.write_bytes(payload)
    original = CliRunner().invoke(main, ["cat", "--file", str(file), "--renderer", "text"])
    assert original.exit_code == 0 and "display=original" in original.output
    assert file.read_bytes() == payload


def test_matched_rule_execution_or_evidence_failure_never_falls_back_to_original(tmp_path, monkeypatch):
    from radiust.models import Artifact
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame

    registry, payload = _synthetic_registry(tmp_path)
    monkeypatch.setattr("radiust.display.raw.default_registry", lambda: registry)
    Image.new("RGBA", (1, 1), (9, 9, 9, 255)).save(tmp_path / "unknown.png", format="PNG")
    unknown = RawFrame(frame(station="cmp1"), (Artifact("a.png", "data", "image/png", (tmp_path / "unknown.png").read_bytes()),))
    def source_preview(*_args, **_kwargs):
        return preview_raw(unknown, apply_legacy=True)
    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", source_preview)
    result = CliRunner().invoke(main, ["cat", "th", "--legacy-display", "--station", "cmp1", "--renderer", "text"])
    assert result.exit_code != 0 and "unmapped" in result.output
    assert "display=original" not in result.output
    rule_path = tmp_path / "rule.json"
    rule = json.loads(rule_path.read_text())
    rule["ordered_steps"][0]["entries"][0]["dbz"] = 10
    rule_path.write_text(json.dumps(rule), encoding="utf-8")
    with (RawFrame(frame(station="cmp1"), (Artifact("a.png", "data", "image/png", payload),)) as raw,
          pytest.raises(DisplayBlockedError, match="evidence or identity")):
        preview_raw(raw, registry=registry, apply_legacy=True)


def test_blocked_station_display_does_not_inherit_other_station_rule(tmp_path):
    from radiust.models import Artifact
    from radiust.raw import RawFrame

    from tests.support.cli_experience import frame

    registry, payload = _synthetic_registry(tmp_path)
    index = json.loads((tmp_path / "index.json").read_text())
    index["paths"].insert(0, {"source": "th", "product": "composite",
                              "path_id": "th/composite/a", "status": "blocked"})
    (tmp_path / "index.json").write_text(json.dumps(index), encoding="utf-8")
    registry = LegacyDisplayRegistry(tmp_path)
    with RawFrame(frame(station="a"), (Artifact("a.png", "data", "image/png", payload),)) as raw:
        preview = preview_raw(raw, registry=registry, apply_legacy=True)
        assert preview.display_mode == "original"
        assert "blocked" in (preview.reason or "")


def test_synthetic_verified_multi_artifact_source_cat_uses_full_frame(tmp_path, monkeypatch):
    from tests.sources.test_tile_adapter_replay import _synthetic_display_tiles

    raw, rule, evidence = _synthetic_display_tiles()
    for name, value in (("rule.json", asdict(rule)), ("evidence.json", asdict(evidence))):
        (tmp_path / name).write_text(json.dumps(value), encoding="utf-8")
    index = {"schema_version": 1, "paths": [{
        "source": rule.source, "product": rule.product, "path_id": rule.path_id,
        "status": "passed", "rule_version": rule.rule_version, "config_hash": rule.config_hash,
        "rule_file": "rule.json", "evidence_file": "evidence.json",
    }]}
    (tmp_path / "index.json").write_text(json.dumps(index), encoding="utf-8")
    registry = LegacyDisplayRegistry(tmp_path)
    monkeypatch.setattr("radiust.display.raw.default_registry", lambda: registry)
    before = [raw.bytes(item.name) for item in raw.artifacts]
    def source_preview(*_args, **_kwargs):
        try:
            return preview_raw(raw, apply_legacy=True)
        finally:
            raw.close()
    monkeypatch.setattr("radiust.cli.cat.preview_source_raw", source_preview)
    response = CliRunner().invoke(main, ["cat", "synthetic", "--legacy-display", "--station", "global",
                                         "--renderer", "text"])
    assert response.exit_code == 0, response.output
    assert "size=4x4" in response.output and "display=legacy" in response.output
    assert "rule=synthetic-v1" in response.output
    assert "units=unknown" in response.output
    assert raw.closed  # CLI owns its acquisition context.
    assert [item.payload for item in raw.artifacts] == before
