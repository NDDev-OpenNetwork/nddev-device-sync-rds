#!/usr/bin/env python3
"""Prepare the exact browser relay queue overlay and native build inputs."""
import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def prepare(root):
    repo = Path(__file__).resolve().parents[1]
    spec = json.loads((repo / "providers/browser-transport.lock.json").read_text())
    sys.dont_write_bytecode = True
    imported = spec_from_file_location("build_inputs", repo / "scripts/prepare-screen-build.py")
    build = module_from_spec(imported)
    imported.loader.exec_module(build)
    build.prepare(root)
    original = build.package(root / "registry-inputs", spec)
    before = original / "src/ws_stream.rs"
    build.require(build.digest(before) == spec["before_source_sha256"], "browser relay source mismatch")
    patch = repo / spec["patch"]
    build.require(build.digest(patch) == spec["patch_sha256"], "browser relay patch mismatch")

    def bounded(contents):
        with tempfile.TemporaryDirectory(prefix="ws-overlay-", dir=root) as directory:
            path = Path(directory) / "src"
            path.mkdir()
            (path / "ws_stream.rs").write_text(contents)
            subprocess.run(["patch", "--batch", "--forward", "--no-backup-if-mismatch", "-p1", "-i", str(patch)], cwd=directory, check=True, stdout=subprocess.DEVNULL)
            return (path / "ws_stream.rs").read_text()

    build.derive(original, root / "build/ws_stream_wasm", bounded, spec["after_source_sha256"], "src/ws_stream.rs")
    receipt = dict(spec, build_sha256=hashlib.sha256((root / "screen-build-verification.json").read_bytes()).hexdigest())
    (root / "browser-build-verification.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(root / "build/ws_stream_wasm")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    prepare(parser.parse_args().destination.resolve())
