from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import subprocess
import time
import urllib.error
from pathlib import Path
from zipfile import ZipFile

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[2]


def load_script(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / f"scripts/release/{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


prepare = load_script("prepare")
publish = load_script("publish")
verify_wheel = load_script("verify_wheel")


@pytest.mark.parametrize("tag", ["main", "v1.2", "v01.2.3", "v1.2.3-rc.1", "v1.2.3\n"])
def test_invalid_tags_do_not_create_release_tree(tmp_path, tag):
    output = tmp_path / "release"
    with pytest.raises(ValueError, match="release tag"):
        prepare.prepare(ROOT, output, tag)
    assert not output.exists()


def test_prepared_tag_versions_and_resources_are_independent_of_checkout(tmp_path):
    original = (ROOT / "pyproject.toml").read_bytes()
    output = tmp_path / "release"
    info = prepare.prepare(ROOT, output, "v0.2.3")
    assert info["version"] == "0.2.3"
    assert not (output / "tests/fixtures").exists()
    assert (ROOT / "pyproject.toml").read_bytes() == original
    assert '__version__ = "0.2.3"' in (output / "python/radiust/__init__.py").read_text()
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked", "--offline"],
        cwd=output, capture_output=True, text=True, check=True,
    )
    packages = json.loads(result.stdout)["packages"]
    assert {package["version"] for package in packages} == {"0.2.3"}
    for package in packages:
        for dependency in package["dependencies"]:
            if dependency["name"].startswith("radiust-"):
                assert dependency["req"] == "=0.2.3"
    for resource, digest in info["embedded_resources"].items():
        assert hashlib.sha256((output / "crates/radiust-core" / resource).read_bytes()).hexdigest() == digest
    prepare.sync_resources(output, check=True)
    # A changed canonical resource must fail the drift check and be refreshed.
    canonical = output / "python/radiust/resources/catalog.json"
    canonical.write_bytes(canonical.read_bytes() + b"\n")
    with pytest.raises(ValueError, match="stale embedded resource"):
        prepare.sync_resources(output, check=True)
    prepare.sync_resources(output)
    prepare.sync_resources(output, check=True)


def test_wheel_rejects_validation_imagery_before_install(tmp_path):
    wheel = tmp_path / "sample.whl"
    with ZipFile(wheel, "w") as archive:
        archive.writestr("radiust.data/data/tests/fixtures/sources/my/raw/peninsular.png", b"image")
    with pytest.raises(ValueError, match="validation-only fixtures"):
        verify_wheel.verify(wheel, "must-not-run", "0.1.1")


def test_pypi_retry_skips_only_identical_artifacts(tmp_path, monkeypatch):
    dist = tmp_path / "dist"
    dist.mkdir()
    old = dist / "old.whl"
    old.write_bytes(b"verified wheel")
    new = dist / "new.whl"
    new.write_bytes(b"another verified wheel")
    payload = {"urls": [{"filename": old.name, "digests": {"sha256": hashlib.sha256(old.read_bytes()).hexdigest()}}]}
    monkeypatch.setattr(publish, "fetch", lambda url: json.dumps(payload).encode())
    monkeypatch.delenv("GITHUB_OUTPUT", raising=False)
    assert publish.pending_wheels(dist, "0.2.3") is True
    assert new.is_file()
    assert (tmp_path / "already-published/old.whl").is_file()


def test_macos_build_and_repair_use_tag_commit_timestamp():
    workflow = yaml.safe_load((ROOT / ".github/workflows/release.yml").read_text())
    steps = workflow["jobs"]["wheels"]["steps"]
    script = next(step["run"] for step in steps if step.get("name") == "Build and repair macOS wheel")
    epoch = 'export SOURCE_DATE_EPOCH="$(git show -s --format=%ct HEAD)"'
    assert script.index(epoch) < script.index("python -m maturin build") < script.index("delocate-wheel")


def test_delocate_repacking_is_reproducible_for_partial_pypi_retry(tmp_path, monkeypatch):
    tools = pytest.importorskip("delocate.tools")
    wheeltools = pytest.importorskip("delocate.wheeltools")
    epoch = int(subprocess.check_output(["git", "show", "-s", "--format=%ct", "HEAD"], cwd=ROOT, text=True))
    monkeypatch.delenv("SOURCE_DATE_EPOCH", raising=False)
    monkeypatch.delenv("GITHUB_OUTPUT", raising=False)
    raw = tmp_path / "raw.whl"
    with ZipFile(raw, "w") as archive:
        archive.writestr("sample/__init__.py", b"# release fixture\n")
        archive.writestr("sample-0.2.3.dist-info/RECORD", b"")

    def repack(output, repair_time):
        tree = output.parent / "unpacked"
        tools.zip2dir(raw, tree)
        wheeltools.rewrite_record(tree)
        # Repair rewrites metadata and creates files at the current wall time.
        for path in [tree, *tree.rglob("*")]:
            os.utime(path, (repair_time, repair_time))
        tools.dir2zip(tree, output)
        return hashlib.sha256(output.read_bytes()).hexdigest()

    first = tmp_path / "first"
    first.mkdir()
    dist = tmp_path / "dist"
    dist.mkdir()
    name = "sample-0.2.3-cp312-cp312-macosx_11_0_arm64.whl"
    old, retry = first / name, dist / name
    # Confirm this fixture reproduces the timestamp-only hash mismatch.
    assert repack(old, epoch + 60) != repack(retry, epoch + 120)
    monkeypatch.setenv("SOURCE_DATE_EPOCH", str(epoch))
    digest = repack(old, epoch + 60)
    assert repack(retry, epoch + 120) == digest
    with ZipFile(retry) as archive:
        expected_time = time.gmtime(epoch)[:5] + (time.gmtime(epoch).tm_sec // 2 * 2,)
        assert {entry.date_time for entry in archive.infolist()} == {expected_time}

    pending = dist / "pending.whl"
    pending.write_bytes(b"not yet published")
    payload = {"urls": [{"filename": name, "digests": {"sha256": digest}}]}
    monkeypatch.setattr(publish, "fetch", lambda url: json.dumps(payload).encode())
    assert publish.pending_wheels(dist, "0.2.3") is True
    assert pending.is_file()
    assert not retry.exists()
    assert (tmp_path / "already-published" / name).is_file()


def test_pypi_collision_fails_before_moving_any_artifact(tmp_path, monkeypatch):
    dist = tmp_path / "dist"
    dist.mkdir()
    wheel = dist / "collision.whl"
    wheel.write_bytes(b"new contents")
    payload = {"urls": [{"filename": wheel.name, "digests": {"sha256": "different hash"}}]}
    monkeypatch.setattr(publish, "fetch", lambda url: json.dumps(payload).encode())
    with pytest.raises(ValueError, match="different contents"):
        publish.pending_wheels(dist, "0.2.3")
    assert wheel.is_file()
    assert not (tmp_path / "already-published").exists()


def test_registry_access_errors_cannot_be_mistaken_for_an_unpublished_package(monkeypatch):
    def denied(*args, **kwargs):
        raise urllib.error.HTTPError("https://index.crates.io", 403, "denied", {}, None)

    monkeypatch.setattr(publish.urllib.request, "urlopen", denied)
    with pytest.raises(urllib.error.HTTPError):
        publish.crate_checksum("radiust-core", "0.2.3")


def test_yanked_crate_cannot_be_silently_accepted_on_retry(monkeypatch):
    record = {"vers": "0.2.3", "cksum": "hash", "yanked": True}
    monkeypatch.setattr(publish, "fetch", lambda url: json.dumps(record).encode())
    with pytest.raises(ValueError, match="yanked"):
        publish.crate_checksum("radiust-core", "0.2.3")
