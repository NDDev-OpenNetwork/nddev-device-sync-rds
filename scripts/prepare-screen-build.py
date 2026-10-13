#!/usr/bin/env python3
"""Derive checked build inputs without mutating the verified upstream archive."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import urllib.request

from importlib.util import module_from_spec, spec_from_file_location


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def files(root):
    return {p.relative_to(root).as_posix(): p for p in root.rglob("*") if p.is_file() or p.is_symlink()}


def derive(original, destination, transform, expected_manifest):
    if not destination.exists():
        shutil.copytree(original, destination, symlinks=True)
        manifest = destination / "Cargo.toml"
        manifest.write_text(transform(manifest.read_text()))
    originals, derived = files(original), files(destination)
    require(originals.keys() == derived.keys(), "derived build has extra or missing source files")
    for relative, source in originals.items():
        target = derived[relative]
        require(target.resolve().is_relative_to(destination.resolve()), "unsafe derived source link")
        if relative == "Cargo.toml":
            require(digest(target) == expected_manifest, "derived manifest mismatch")
        elif source.is_symlink():
            require(target.is_symlink() and source.readlink() == target.readlink(), "derived source link mismatch")
        else:
            require(digest(source) == digest(target), f"derived source mismatch: {relative}")


def package(root, entry):
    archive = root / f"{entry['name']}-{entry['version']}.crate"
    if not archive.exists():
        partial = archive.with_suffix(".partial")
        try:
            with urllib.request.urlopen(f"https://static.crates.io/crates/{entry['name']}/{archive.name}", timeout=30) as source, partial.open("xb") as output:
                total = 0
                while chunk := source.read(65536):
                    total += len(chunk)
                    require(total <= entry["archive_size"], "registry archive exceeds bound")
                    output.write(chunk)
            partial.rename(archive)
        finally:
            partial.unlink(missing_ok=True)
    require(archive.stat().st_size == entry["archive_size"] and digest(archive) == entry["archive_sha256"], "registry archive mismatch")
    source = root / f"{entry['name']}-{entry['version']}"
    with tarfile.open(archive) as bundle:
        require(sum(m.size for m in bundle.getmembers()) <= 10_000_000, "registry source exceeds bound")
        if not source.exists():
            bundle.extractall(root, filter="data")
        expected = {}
        prefix = source.name + "/"
        for member in bundle.getmembers():
            require(member.name == source.name or member.name.startswith(prefix), "invalid registry source root")
            if member.isfile():
                expected[member.name[len(prefix):]] = bundle.extractfile(member).read()
            else:
                require(member.isdir(), "registry source contains an unsupported object")
        actual = files(source)
        require(actual.keys() == expected.keys(), "registry source has extra or missing files")
        for relative, data in expected.items():
            require(actual[relative].resolve().is_relative_to(source.resolve()) and actual[relative].read_bytes() == data, "registry source mismatch")
    return source


def prepare(root):
    root = root.resolve()
    repo = Path(__file__).resolve().parents[1]
    spec = json.loads((repo / "providers/screen-build.lock.json").read_text())
    source_spec = json.loads((repo / "providers/screen-source.lock.json").read_text())
    require(spec["upstream_commit"] == source_spec["source_commit"], "build source pin mismatch")
    module_spec = spec_from_file_location("provider", repo / "scripts/prepare-screen-provider.py")
    provider = module_from_spec(module_spec)
    sys.dont_write_bytecode = True
    module_spec.loader.exec_module(provider)
    provider.prepare(root)
    original = root / "engine"
    workspace = spec["workspace"]
    require(digest(repo / workspace["patch"]) == workspace["patch_sha256"], "workspace patch mismatch")
    require(digest(original / "Cargo.toml") == workspace["before_sha256"], "original manifest mismatch")

    def versions(contents):
        changed, count = re.subn(r'(rds-[a-z-]+\s*=\s*\{\s*path\s*=\s*"crates/rds-[a-z-]+")\s*\}', r'\1, version = "=0.1.0" }', contents)
        require(count == workspace["entries"], "workspace path dependency drift")
        return changed

    build = root / "build"
    build.mkdir(mode=0o700, exist_ok=True)
    derive(original, build / "engine", versions, workspace["after_sha256"])
    archives = root / "registry-inputs"
    archives.mkdir(mode=0o700, exist_ok=True)
    netlink = spec["netlink"]
    netlink_original = package(archives, netlink)
    require(digest(netlink_original / "Cargo.toml") == netlink["before_manifest_sha256"], "netlink manifest mismatch")
    require(digest(repo / netlink["patch"]) == netlink["patch_sha256"], "netlink patch mismatch")
    derive(netlink_original, build / "netlink-packet-core", lambda contents: contents.replace('[dependencies.paste]\nversion = "1"', '[dependencies.paste]\nversion = "=0.2.3"\npackage = "pastey"'), netlink["after_manifest_sha256"])
    package(archives, spec["macro"])
    rust = subprocess.check_output(["rustup", "run", spec["toolchain"], "rustc", "-vV"], text=True)
    require(f"release: {spec['toolchain']}\n" in rust, "qualification Rust toolchain mismatch")
    (root / "screen-build-verification.json").write_text(json.dumps(dict(spec, rustc=rust), indent=2) + "\n")
    print(build)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    prepare(parser.parse_args().destination)
