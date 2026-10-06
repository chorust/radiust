"""Prepare a self-contained release tree without modifying the tagged checkout."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CRATES = ("radiust-core", "radiust-cli", "radiust-python")
TAG = re.compile(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)\Z")


def release_version(tag: str) -> str:
    if not TAG.fullmatch(tag):
        raise ValueError("release tag must be vMAJOR.MINOR.PATCH (no leading zeroes)")
    return tag[1:]


def sync_resources(root: Path, *, check: bool = False) -> dict[str, str]:
    core = root / "crates/radiust-core"
    expected: dict[Path, Path] = {}
    for source in (core / "src").rglob("*.rs"):
        for relative in re.findall(r'include_str!\(\s*"([^"]*resources/builtin/[^"]+)"', source.read_text()):
            target = (source.parent / relative).resolve()
            resource = target.relative_to(core / "resources/builtin")
            expected[target] = root / "python/radiust/resources" / resource
    if not expected:
        raise ValueError("no embedded core resources found")
    hashes = {}
    for target, canonical in sorted(expected.items()):
        payload = canonical.read_bytes()
        json.loads(payload)  # Reject invalid metadata before it enters a release.
        if check:
            if not target.is_file() or target.read_bytes() != payload:
                raise ValueError(f"stale embedded resource: {target.relative_to(root)}")
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(payload)
        hashes[str(target.relative_to(core))] = hashlib.sha256(payload).hexdigest()
    extras = set((core / "resources/builtin").rglob("*.json")) - set(expected)
    if check and extras:
        raise ValueError(f"unused embedded resources: {sorted(map(str, extras))}")
    for extra in extras:
        extra.unlink()
    for crate in CRATES[:2]:
        target = root / "crates" / crate / "LICENSE"
        if check:
            if target.read_bytes() != (root / "LICENSE").read_bytes():
                raise ValueError(f"stale license: {crate}")
        else:
            shutil.copyfile(root / "LICENSE", target)
    return hashes


def replace_once(path: Path, pattern: str, replacement: str) -> None:
    updated, count = re.subn(pattern, replacement, path.read_text(), count=1, flags=re.MULTILINE)
    if count != 1:
        raise ValueError(f"cannot update release version in {path}")
    path.write_text(updated)


def set_versions(root: Path, version: str) -> None:
    for crate in CRATES:
        manifest = root / "crates" / crate / "Cargo.toml"
        replace_once(manifest, r'^version = "[^"]+"$', f'version = "{version}"')
        for dependency in CRATES:
            pattern = rf'^{dependency} = \{{ path = "([^"]+)"(?:, version = "[^"]+")? \}}$'
            text = re.sub(pattern, rf'{dependency} = {{ path = "\1", version = "={version}" }}', manifest.read_text(), flags=re.MULTILINE)
            manifest.write_text(text)
    replace_once(root / "pyproject.toml", r'^version = "[^"]+"$', f'version = "{version}"')
    replace_once(root / "python/radiust/__init__.py", r'^__version__ = "[^"]+"$', f'__version__ = "{version}"')
    for lock, names in (("Cargo.lock", CRATES), ("uv.lock", ("radiust",))):
        for name in names:
            replace_once(root / lock, rf'^(name = "{name}"\n)version = "[^"]+"$', rf'\g<1>version = "{version}"')


def prepare(root: Path, output: Path, tag: str) -> dict:
    version = release_version(tag)
    root, output = root.resolve(), output.resolve()
    if output == root or root in output.parents:
        raise ValueError("release output must be outside the source checkout")
    output.mkdir(parents=True, exist_ok=False)
    for name in ("Cargo.toml", "Cargo.lock", "pyproject.toml", "uv.lock", "README.md", "README.en.md", "LICENSE"):
        shutil.copyfile(root / name, output / name)
    for crate in CRATES:
        source = root / "crates" / crate
        target = output / "crates" / crate
        target.mkdir(parents=True)
        shutil.copyfile(source / "Cargo.toml", target / "Cargo.toml")
        for name in ("README.md", "LICENSE"):
            if (source / name).is_file():
                shutil.copyfile(source / name, target / name)
        shutil.copytree(source / "src", target / "src")
        if (source / "resources").is_dir():
            shutil.copytree(source / "resources", target / "resources")
    shutil.copytree(root / "python", output / "python", ignore=shutil.ignore_patterns("__pycache__", "*.so", "*.pyc", "*.pyd"))
    resources = sync_resources(output)
    set_versions(output, version)
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True).strip())
    report = {"tag": tag, "version": version, "commit": commit, "checkout_dirty": dirty, "embedded_resources": resources}
    (output / "release-info.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--tag")
    mode.add_argument("--check-resources", action="store_true")
    mode.add_argument("--sync-resources", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.tag:
        if args.output is None:
            parser.error("--tag requires --output")
        report = prepare(args.root, args.output, args.tag)
        print(json.dumps(report, indent=2))
    else:
        hashes = sync_resources(args.root.resolve(), check=args.check_resources)
        print(f"verified {len(hashes)} embedded resources" if args.check_resources else f"synced {len(hashes)} embedded resources")


if __name__ == "__main__":
    main()
