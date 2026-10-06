"""Install packaged crates through an isolated directory registry, before upload."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

import tomllib


def run(command: list[str], cwd: Path, *, capture: bool = False) -> str:
    result = subprocess.run(command, cwd=cwd, check=True, text=True, stdout=subprocess.PIPE if capture else None)
    return result.stdout if capture else ""


def verify(source: Path, output: Path, *, offline: bool = False, debug: bool = False) -> dict:
    source, output = source.resolve(), output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    info = json.loads((source / "release-info.json").read_text())
    version = info["version"]
    network = ["--offline"] if offline else []
    run(["cargo", "package", "-p", "radiust-core", "-p", "radiust-cli", "--locked", "--no-verify", *network], source)
    metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked", *network], source, capture=True))
    package_dir = Path(metadata["target_directory"]) / "package"
    with tempfile.TemporaryDirectory(prefix="radiust-crate-install-") as temporary:
        isolated = Path(temporary)
        vendor = isolated / "vendor"
        run(["cargo", "vendor", "--locked", "--versioned-dirs", *network, str(vendor)], source)
        extracted = isolated / "packages"
        extracted.mkdir()
        checksums = {}
        for name in ("radiust-core", "radiust-cli"):
            archive = package_dir / f"{name}-{version}.crate"
            checksums[archive.name] = hashlib.sha256(archive.read_bytes()).hexdigest()
            with tarfile.open(archive) as tar:
                tar.extractall(extracted, filter="data")
            package = extracted / f"{name}-{version}"
            manifest = tomllib.loads((package / "Cargo.toml").read_text())
            assert manifest["package"]["version"] == version
            assert (package / "LICENSE").read_bytes() == (source / "LICENSE").read_bytes()
            if name == "radiust-core":
                for resource, digest in info["embedded_resources"].items():
                    assert hashlib.sha256((package / resource).read_bytes()).hexdigest() == digest, resource
            else:
                assert manifest["dependencies"]["radiust-core"]["version"] == f"={version}"
                assert "path" not in manifest["dependencies"]["radiust-core"]
            target = vendor / package.name
            shutil.copytree(package, target)
            files = {
                str(path.relative_to(target)): hashlib.sha256(path.read_bytes()).hexdigest()
                for path in target.rglob("*") if path.is_file()
            }
            (target / ".cargo-checksum.json").write_text(json.dumps({"files": files, "package": checksums[archive.name]}))
            shutil.copyfile(archive, output / archive.name)
        config = isolated / "registry.toml"
        config.write_text('[source.crates-io]\nreplace-with = "release-vendor"\n\n[source.release-vendor]\ndirectory = ' + json.dumps(str(vendor)) + "\n")
        cargo = ["cargo", "--config", str(config)]
        # The registry contains the exact .crate contents and package checksum;
        # no workspace path overrides or Python resource directories are present.
        consumer = isolated / "consumer"
        (consumer / "src").mkdir(parents=True)
        (consumer / "Cargo.toml").write_text(
            '[package]\nname = "release-consumer"\nversion = "0.0.0"\nedition = "2024"\n'
            f'[dependencies]\nradiust-core = "={version}"\n'
        )
        (consumer / "src/main.rs").write_text(
            'fn main() {\n'
            f'    assert_eq!(radiust_core::VERSION, "{version}");\n'
            '    let catalog = radiust_core::source::catalog::SourceCatalog::builtin().unwrap();\n'
            '    assert!(!catalog.sources.is_empty());\n'
            '    assert!(catalog.sources.iter().any(|source| source.id == "id"));\n'
            '}\n'
        )
        run([*cargo, "generate-lockfile", "--offline"], consumer)
        run([*cargo, "run", "--locked", "--offline", *([] if debug else ["--release"])], consumer)
        install = isolated / "install"
        run([*cargo, "install", "radiust-cli", "--version", version, "--root", str(install), "--locked", "--offline", *(["--debug"] if debug else [])], isolated)
        binary = install / "bin/radiust"
        assert run([str(binary), "--version"], isolated, capture=True).strip() == f"radiust {version}"
        report = json.loads(run([str(binary), "list", "sources", "--json"], isolated, capture=True))
        assert report["error"] is None
        for name in ("id", "id_sidarma"):
            report = json.loads(run([str(binary), "list", "stations", name, "--json"], isolated, capture=True))
            assert report["error"] is None
        shutil.copyfile(binary, output / "radiust")
    report = {"version": version, "commit": info["commit"], "checkout_dirty": info.get("checkout_dirty"), "crates": checksums, "core_consumer": "passed", "cli_install": "passed", "provider_network": False, "profile": "debug" if debug else "release"}
    (output / "crate-install.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--debug", action="store_true")
    args = parser.parse_args()
    # Never inherit a workspace's relative target directory into a consumer.
    if "CARGO_TARGET_DIR" in os.environ:
        os.environ["CARGO_TARGET_DIR"] = str(Path(os.environ["CARGO_TARGET_DIR"]).resolve())
    print(json.dumps(verify(args.source, args.output, offline=args.offline, debug=args.debug), indent=2))


if __name__ == "__main__":
    main()
