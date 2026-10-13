#!/usr/bin/env python3
"""Prepare checked Linux library inputs and emit consumer-root Cargo overrides."""
import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
from pathlib import Path
import shutil
import sys


LIBRARIES = {
    "rds-core": [],
    "rds-net": [],
    "rds-client": [],
    "rds-desktop": ["x11"],
    "rds-agent": ["desktop"],
}


def prepare(destination, config, archive=None):
    repo = Path(__file__).resolve().parents[1]
    destination, config = destination.resolve(), config.resolve()
    if destination.is_relative_to(repo) or config.is_relative_to(repo):
        raise ValueError("prepared inputs and generated config must stay outside the source repository")
    sys.dont_write_bytecode = True
    imported = spec_from_file_location("library_inputs", repo / "scripts/prepare-screen-build.py")
    build = module_from_spec(imported)
    imported.loader.exec_module(build)
    source = json.loads((repo / "providers/screen-source.lock.json").read_text())
    if archive:
        if archive.stat().st_size != source["archive_size"] or build.digest(archive) != source["archive_sha256"]:
            raise ValueError("explicit provider input archive mismatch")
        destination.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copyfile(archive, destination / f"rds-{source['source_commit']}.tar.gz")
    build.prepare(destination)
    upstream = "https://github.com/" + source["repository"]
    quote = lambda value: json.dumps(str(value))
    lines = ["# Generated checked library inputs; supply at the consumer Cargo root.",
             f"[patch.{quote(upstream)}]"]
    for name in LIBRARIES:
        lines.append(f"{name} = {{path={quote(destination / 'build/engine/crates' / name)}}}")
    lines.append("[patch.crates-io]")
    for name in ["iroh", "noq", "noq-proto"]:
        lines.append(f"{name} = {{path={quote(destination / 'build/engine/vendor' / name)}}}")
    lines.append(f"netlink-packet-core = {{path={quote(destination / 'build/netlink-packet-core')}}}")
    contents = "\n".join(lines) + "\n"
    if config.exists() and config.read_text() != contents:
        raise ValueError("refusing to overwrite a different consumer Cargo config")
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text(contents)
    receipt = {
        "source": source,
        "target": "x86_64-unknown-linux-gnu",
        "features": LIBRARIES,
        "config_sha256": hashlib.sha256(contents.encode()).hexdigest(),
        "build_receipt_sha256": hashlib.sha256((destination / "screen-build-verification.json").read_bytes()).hexdigest(),
        "scope": "source/library preparation only; no native identity/store/host activation",
    }
    (destination / "library-provider-verification.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(config)
    return receipt


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--archive", type=Path, help="optional exact published provider source archive")
    args = parser.parse_args()
    prepare(args.destination, args.config, args.archive)
