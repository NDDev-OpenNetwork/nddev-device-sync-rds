#!/usr/bin/env python3
"""Verify an immutable upstream source tree in an explicit private directory.

This prepares a qualification dependency; it does not install a provider or
replace provider.lock.json. Build outputs must stay outside the verified tree.
"""
import argparse
import hashlib
import json
import pathlib
import subprocess
import tarfile
import urllib.request


def require(condition, message):
    if not condition:
        raise ValueError(message)


def prepare(destination):
    repo = pathlib.Path(__file__).resolve().parents[1]
    spec = json.loads((repo / "providers/screen-source.lock.json").read_text())
    destination = destination.resolve()
    destination.mkdir(mode=0o700, parents=True, exist_ok=True)
    repository, commit = spec["repository"], spec["source_commit"]
    archive = destination / f"rds-{commit}.tar.gz"
    if not archive.exists():
        partial = archive.with_suffix(".partial")
        try:
            with urllib.request.urlopen(
                f"https://codeload.github.com/{repository}/tar.gz/{commit}", timeout=30
            ) as source, partial.open("xb") as output:
                total = 0
                while chunk := source.read(65536):
                    total += len(chunk)
                    require(total <= spec["archive_size"], "source archive exceeds bound")
                    output.write(chunk)
            partial.rename(archive)
        finally:
            partial.unlink(missing_ok=True)
    require(archive.stat().st_size == spec["archive_size"], "archive size mismatch")
    require(hashlib.sha256(archive.read_bytes()).hexdigest() == spec["archive_sha256"],
            "archive digest mismatch")

    def api(endpoint):
        result = subprocess.run(
            ["gh", "api", f"repos/{repository}/{endpoint}"], check=True,
            capture_output=True, timeout=30,
        )
        return json.loads(result.stdout)

    signed = api(f"commits/{commit}")
    verification = signed["commit"]["verification"]
    require(signed["sha"] == commit and verification["verified"],
            "upstream commit signature is not verified by GitHub")
    require(verification["payload"].startswith(f"tree {spec['tree']}\n"),
            "signed commit tree mismatch")
    tree = api(f"git/trees/{spec['tree']}?recursive=1")
    require(tree["sha"] == spec["tree"] and not tree.get("truncated"),
            "incomplete upstream source tree")
    source = destination / "engine"
    if not source.exists():
        unpack = destination / "engine-unpack"
        unpack.mkdir(mode=0o700)
        with tarfile.open(archive) as bundle:
            require(sum(m.size for m in bundle.getmembers()) <= 100_000_000,
                    "unpacked source exceeds bound")
            bundle.extractall(unpack, filter="data")
        entries = list(unpack.iterdir())
        require(len(entries) == 1 and entries[0].is_dir(), "invalid source archive root")
        entries[0].rename(source)
        unpack.rmdir()
    blobs = 0
    for item in tree["tree"]:
        if item["type"] == "tree":
            continue
        require(item["type"] == "blob", "unsupported source object")
        path = source / item["path"]
        require(path.resolve().is_relative_to(source.resolve()), "unsafe source path")
        data = path.readlink().as_posix().encode() if path.is_symlink() else path.read_bytes()
        digest = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
        require(digest == item["sha"], f"source mismatch: {item['path']}")
        blobs += 1
    receipt = dict(spec, verified_git_blobs=blobs, github_verification=verification,
                   cargo_lock_sha256=hashlib.sha256((source / "Cargo.lock").read_bytes()).hexdigest())
    (destination / "screen-source-verification.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(source)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=pathlib.Path)
    prepare(parser.parse_args().destination)
