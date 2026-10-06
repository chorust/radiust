"""Publish tested packages, allowing retries only for byte-identical artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path


def fetch(url: str) -> bytes | None:
    request = urllib.request.Request(url, headers={"User-Agent": "radiust-release", "Cache-Control": "no-cache"})
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.read()
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def crate_checksum(name: str, version: str) -> str | None:
    payload = fetch(f"https://index.crates.io/{name[:2]}/{name[2:4]}/{name}")
    if payload is None:
        return None
    for line in payload.splitlines():
        record = json.loads(line)
        if record["vers"] == version:
            if record["yanked"]:
                raise ValueError(f"{name} {version} has been yanked")
            return record["cksum"]
    return None


def require_same(name: str, expected: str, actual: str) -> None:
    if actual != expected:
        raise ValueError(f"{name} already exists with different contents; use a new tag/version")


def publish_crates(source: Path, artifacts: Path) -> None:
    version = json.loads((source / "release-info.json").read_text())["version"]
    # Recreate and compare both archives before the first irreversible upload.
    subprocess.run(["cargo", "package", "-p", "radiust-core", "-p", "radiust-cli", "--locked", "--no-verify"], cwd=source, check=True)
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"], cwd=source))
    package_dir = Path(metadata["target_directory"]) / "package"
    expected = {}
    for name in ("radiust-core", "radiust-cli"):
        filename = f"{name}-{version}.crate"
        expected[name] = hashlib.sha256((artifacts / filename).read_bytes()).hexdigest()
        require_same(filename, expected[name], hashlib.sha256((package_dir / filename).read_bytes()).hexdigest())
        existing = crate_checksum(name, version)
        if existing is not None:
            require_same(filename, expected[name], existing)
    for name in ("radiust-core", "radiust-cli"):
        if crate_checksum(name, version) is None:
            subprocess.run(["cargo", "publish", "-p", name, "--locked"], cwd=source, check=True)
        # Wait for registry visibility before publishing the dependent CLI.
        for attempt in range(18):
            checksum = crate_checksum(name, version)
            if checksum is not None:
                require_same(name, expected[name], checksum)
                print(f"{name} {version} is published and matches the tested archive", flush=True)
                break
            if attempt == 17:
                raise RuntimeError(f"{name} {version} did not become visible in the registry")
            time.sleep(10)


def pending_wheels(dist: Path, version: str) -> bool:
    # Avoid a cached 404 from a lookup made before the version was uploaded.
    payload = fetch(f"https://pypi.org/pypi/radiust/{version}/json?release-check={time.time_ns()}")
    published = {} if payload is None else {item["filename"]: item["digests"]["sha256"] for item in json.loads(payload)["urls"]}
    wheels = list(dist.glob("*.whl"))
    if not wheels:
        raise ValueError("no verified wheels supplied")
    # Check every existing artifact before moving any, so a collision fails closed.
    for wheel in wheels:
        if wheel.name in published:
            require_same(wheel.name, hashlib.sha256(wheel.read_bytes()).hexdigest(), published[wheel.name])
    for wheel in wheels:
        if wheel.name in published:
            already = dist.parent / "already-published"
            already.mkdir(exist_ok=True)
            wheel.rename(already / wheel.name)
    pending = any(dist.glob("*.whl"))
    if "GITHUB_OUTPUT" in os.environ:
        with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
            output.write(f"pending={'true' if pending else 'false'}\n")
    return pending


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    crates = commands.add_parser("crates")
    crates.add_argument("--source", required=True, type=Path)
    crates.add_argument("--artifacts", required=True, type=Path)
    pypi = commands.add_parser("pypi-check")
    pypi.add_argument("--dist", required=True, type=Path)
    pypi.add_argument("--version", required=True)
    args = parser.parse_args()
    if args.command == "crates":
        publish_crates(args.source.resolve(), args.artifacts.resolve())
    else:
        print(f"pending wheels: {pending_wheels(args.dist.resolve(), args.version)}")


if __name__ == "__main__":
    main()
