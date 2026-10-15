#!/usr/bin/env python3
"""Build a real downstream Linux package in a new controlled cache.

An explicit source archive covers an unpublished NDS source tree. The public
commit mode also consumes NDS as a Git dependency after protected publication.
Neither mode activates an endpoint, native host or product viewer. The explicit
owned-screen profile compiles the exported SDK API; default remains separate.
"""
import argparse
import hashlib
from importlib.util import module_from_spec, spec_from_file_location
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib
import urllib.request


REPOSITORY = "NDDev-Technocracy-OpenNetwork-NDS/nddev-device-sync-rds"
TARGET = "x86_64-unknown-linux-gnu"


def run(args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    sources = parser.add_mutually_exclusive_group(required=True)
    sources.add_argument("--source-archive", type=Path)
    sources.add_argument("--public-commit")
    parser.add_argument("--archive-sha256", required=True)
    parser.add_argument("--provider-archive", type=Path, help="optional published source input, verified against the native source lock")
    parser.add_argument("--consumer-lock", type=Path, help="separate reviewed downstream lock; required for locked public-Git qualification")
    parser.add_argument("--owned-screen", action="store_true", help="qualify the exported Linux owned-screen SDK API; default remains the separate native-input/default-SDK gate")
    parser.add_argument("--resolve-lock", action="store_true", help="initial lock adoption; retain generated lock for review")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    if output.is_relative_to(repo) or output.exists():
        raise ValueError("output must be a new directory outside the source repository")
    if not re.fullmatch(r"[0-9a-f]{64}", args.archive_sha256):
        raise ValueError("an exact archive SHA-256 is required")
    if args.public_commit and not args.consumer_lock and not args.resolve_lock:
        raise ValueError("locked public-Git qualification requires an explicit downstream consumer lock")
    output.mkdir(mode=0o700, parents=True)
    archive = output / "nds-source.tar"
    if args.public_commit:
        if not re.fullmatch(r"[0-9a-f]{40}", args.public_commit):
            raise ValueError("an immutable public commit is required")
        with urllib.request.urlopen(f"https://codeload.github.com/{REPOSITORY}/tar.gz/{args.public_commit}", timeout=30) as response, archive.open("xb") as target:
            total = 0
            while chunk := response.read(65536):
                total += len(chunk)
                if total > 10_000_000:
                    raise ValueError("NDS source archive exceeds bound")
                target.write(chunk)
    else:
        if args.source_archive.stat().st_size > 10_000_000:
            raise ValueError("NDS source archive exceeds bound")
        shutil.copyfile(args.source_archive, archive)
    if digest(archive) != args.archive_sha256:
        raise ValueError("NDS source archive mismatch")
    source = output / "source"
    source.mkdir()
    with tarfile.open(archive) as bundle:
        if sum(member.size for member in bundle.getmembers()) > 20_000_000:
            raise ValueError("unpacked NDS source exceeds bound")
        bundle.extractall(source, filter="data")
    if not (source / "Cargo.toml").is_file():
        entries = list(source.iterdir())
        if len(entries) != 1 or not entries[0].is_dir():
            raise ValueError("invalid NDS archive root")
        source = entries[0]
    sys.dont_write_bytecode = True
    imported = spec_from_file_location("library_bootstrap", source / "scripts/prepare-library-provider.py")
    bootstrap = module_from_spec(imported)
    imported.loader.exec_module(bootstrap)
    provider = output / "provider"
    # External Cargo tools also resolve metadata. Keep the one root config in
    # this isolated Cargo home so they cannot silently resolve another graph.
    config = output / "cargo-home/config.toml"
    preparation = bootstrap.prepare(provider, config, args.provider_archive)
    toolchain = tomllib.loads((source / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    consumer = output / "consumer"
    (consumer / "src").mkdir(parents=True)
    quote = lambda value: json.dumps(str(value))
    sdk = f"git={quote('https://github.com/' + REPOSITORY)},rev={quote(args.public_commit)}" if args.public_commit else f"path={quote(source)}"
    sdk_manifest = tomllib.loads((source / "Cargo.toml").read_text())
    sdk_version = sdk_manifest["package"]["version"]
    sdk_features = ["owned-screen"] if args.owned_screen else []
    # Fetch/lock adoption uses this same unconditional SDK dependency feature;
    # Cargo fetch itself has no --features option.
    libraries = {name: features for name, features in bootstrap.LIBRARIES.items()
                 if not args.owned_screen or name != "rds-agent"}
    manifest = ["[package]", 'name="nds-rds-library-consumer"', 'version="0.0.0"',
                'edition="2024"', 'license="AGPL-3.0-only"']
    if args.owned_screen:
        manifest.extend(["[features]", 'owned-screen=["nddev-device-sync-rds/owned-screen"]'])
    manifest.extend(["[dependencies]",
                     f"nddev-device-sync-rds={{{sdk},version=\"={sdk_version}\",features={json.dumps(sdk_features)}}}"])
    if args.owned_screen:
        core_dependencies = sdk_manifest["target"]['cfg(target_os = "linux")']["dependencies"]
        for name, version in [("nddev-device-sync-domain", "=0.0.12"),
                              ("nddev-device-sync-application", "=0.0.1-alpha.21")]:
            dep = core_dependencies[name]
            if (dep["git"] != "https://github.com/NDDev-Technocracy-OpenNetwork-NDS/nddev-device-sync-core.git"
                    or dep["rev"] != "889f0f4a9b6416e92eaaac111471a9d700ef8468"
                    or dep["version"] != version):
                raise ValueError("owned API witness requires coordinated canonical Core inputs")
            manifest.append(name + "={version=" + quote(version) + ",git=" + quote(dep["git"]) + ",rev=" + quote(dep["rev"]) + "}")
        manifest.append('tokio-util={version="0.7",features=["rt"]}')
    native = "https://github.com/" + preparation["source"]["repository"]
    for name, features in libraries.items():
        manifest.append(f"{name}={{git={quote(native)},rev={quote(preparation['source']['source_commit'])},version=\"=0.1.0\",default-features=false,features={json.dumps(features)}}}")
    (consumer / "Cargo.toml").write_text("\n".join(manifest) + "\n")
    witness = source / ("qualification/owned-library.rs" if args.owned_screen else "qualification/library.rs")
    shutil.copyfile(witness, consumer / "src/lib.rs")
    # A public SDK commit cannot contain a lock that references itself. The
    # consumer owns its lock independently from the consumed SDK revision.
    lock = args.consumer_lock.resolve() if args.consumer_lock else (source / ("Cargo.lock" if args.owned_screen else "qualification/library.Cargo.lock") if not args.public_commit else None)
    input_lock_sha256 = digest(lock) if lock and lock.is_file() else None
    if args.consumer_lock and input_lock_sha256 is None:
        raise ValueError("the explicit consumer lock is unavailable")
    if input_lock_sha256:
        shutil.copyfile(lock, consumer / "Cargo.lock")
    if input_lock_sha256 is None and not args.resolve_lock:
        raise ValueError("a reviewed consumer lock is required")
    cargo_home = output / "cargo-home"
    target_dir = output / "target"
    env = dict(os.environ, CARGO_HOME=str(cargo_home), CARGO_TARGET_DIR=str(target_dir),
               GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1", GIT_TERMINAL_PROMPT="0",
               CARGO_NET_GIT_FETCH_WITH_CLI="true")
    for name in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_NET_OFFLINE", "RUSTC",
                 "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"]:
        env.pop(name, None)
    def cargo(arguments):
        # Cargo external subcommands must receive --config themselves.
        selected = ["--features", "owned-screen"] if args.owned_screen and arguments[0] in {"metadata", "clippy", "build", "check", "test"} else []
        return ["rustup", "run", toolchain, "cargo", arguments[0], "--config", str(config), *selected, *arguments[1:]]
    if args.resolve_lock:
        # A path package is absent from the new Git package-ID namespace.
        # Ordinary resolution retains the seed's other locked dependencies.
        run(cargo(["fetch", "--target", TARGET] if input_lock_sha256 else ["generate-lockfile"]), cwd=consumer, env=env)
    locked_sha256 = digest(consumer / "Cargo.lock")
    run(cargo(["fetch", "--locked", "--target", TARGET]), cwd=consumer, env=env)
    metadata = json.loads(subprocess.check_output(cargo(["metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", TARGET]), cwd=consumer, env=env))
    (output / "source-graph.json").write_text(json.dumps(metadata) + "\n")
    packages = {package["id"]: package for package in metadata["packages"]}
    def node_for(name, version=None):
        matches = [node for node in metadata["resolve"]["nodes"] if packages[node["id"]]["name"] == name
                   and (version is None or packages[node["id"]]["version"] == version)]
        if len(matches) != 1:
            raise ValueError(f"ambiguous/missing selected package: {name}")
        return matches[0]

    sdk_node = node_for("nddev-device-sync-rds")
    if set(sdk_node["features"]) - {"default"} != set(sdk_features):
        raise ValueError("the SDK does not have the selected qualification feature")
    sdk_package = packages[sdk_node["id"]]
    if args.owned_screen:
        for name, version in [("nddev-device-sync-domain", "0.0.12"),
                              ("nddev-device-sync-application", "0.0.1-alpha.21")]:
            package = packages[node_for(name, version)["id"]]
            if package["source"] != "git+https://github.com/NDDev-Technocracy-OpenNetwork-NDS/nddev-device-sync-core.git?rev=889f0f4a9b6416e92eaaac111471a9d700ef8468#889f0f4a9b6416e92eaaac111471a9d700ef8468":
                raise ValueError("owned API witness lost canonical Core source identity")
    sdk_source = sdk_package["source"]
    if args.public_commit:
        expected = f"git+https://github.com/{REPOSITORY}?rev={args.public_commit}#{args.public_commit}"
        if sdk_source != expected:
            raise ValueError("the SDK was not consumed from the exact public Git revision")
    elif Path(sdk_package["manifest_path"]) != source / "Cargo.toml":
        raise ValueError("the SDK archive source was replaced")
    for name, features in libraries.items():
        node = node_for(name)
        package = packages[node["id"]]
        effective = set(node["features"]) - {"default"}
        if (package["version"] != "0.1.0" or effective != set(features)
                or package["features"].get("default", [])
                or Path(package["manifest_path"]) != provider / "build/engine/crates" / name / "Cargo.toml"):
            raise ValueError(f"unexpected native package/features/source: {name}")
    for name in ["iroh", "noq", "noq-proto"]:
        if Path(packages[node_for(name)["id"]]["manifest_path"]) != provider / "build/engine/vendor" / name / "Cargo.toml":
            raise ValueError(f"lost consumer-root override: {name}")
    if Path(packages[node_for("netlink-packet-core", "0.8.2")["id"]]["manifest_path"]) != provider / "build/netlink-packet-core/Cargo.toml":
        raise ValueError("lost verified netlink overlay")
    run(["cargo", "deny", "--manifest-path", str(consumer / "Cargo.toml"), "--metadata-path", str(output / "source-graph.json"), "--config", str(source / "deny.toml"), "--target", TARGET, "check"], cwd=consumer, env=env)
    run(["cargo", "audit", "--deny", "warnings", "--file", str(consumer / "Cargo.lock")], cwd=consumer, env=env)
    if digest(consumer / "Cargo.lock") != locked_sha256:
        raise ValueError("dependency tools changed the locked source graph")
    run(cargo(["clippy", "--locked", "--offline", "--target", TARGET, "-j", "4", "--", "-D", "warnings"]), cwd=consumer, env=env)
    compiled = subprocess.run(cargo(["build", "--locked", "--offline", "--target", TARGET, "-j", "4", "--message-format=json"]), cwd=consumer, env=env, text=True, stdout=subprocess.PIPE)
    (output / "build.jsonl").write_text(compiled.stdout)
    artifacts = [json.loads(line) for line in compiled.stdout.splitlines() if line.startswith("{")]
    if compiled.returncode:
        for item in artifacts:
            if item.get("reason") == "compiler-message" and item["message"].get("rendered"):
                sys.stderr.write(item["message"]["rendered"])
        raise RuntimeError("downstream build failed; private diagnostics retained")
    if digest(consumer / "Cargo.lock") != locked_sha256:
        raise ValueError("compiler tools changed the locked source graph")
    features = [{"package_id": item["package_id"], "features": item["features"], "kind": item["target"]["kind"]} for item in artifacts if item.get("reason") == "compiler-artifact"]
    (output / "compiled-features.json").write_text(json.dumps(features, indent=2) + "\n")
    forbidden = {"egui", "wgpu", "winit", "ashpd", "pipewire", "gtk", "rds-cli", "rds-audio"}
    if any(packages[item["package_id"]]["name"] in forbidden for item in features):
        raise ValueError("unexpected GUI/portal/executable/audio package compiled")
    receipt = {"source_mode": "public_git" if args.public_commit else "local_archive",
               "public_commit": args.public_commit, "nds_archive_sha256": digest(archive),
               "sdk_source": sdk_source,
               "sdk_features": sdk_features,
               "qualification_profile": "owned-screen-api" if args.owned_screen else "native-inputs-default-sdk",
               "api_witness_sha256": digest(witness),
               "selected_native_libraries": libraries,
               "tooling_sha256": digest(Path(__file__)),
               "input_consumer_lock_sha256": input_lock_sha256,
               "lock_resolved": args.resolve_lock,
               "initial_cache": "new controlled Cargo home with only emitted config; empty registry/Git/build caches, no global cache copied",
               "provider": preparation, "target": TARGET,
               "source_graph_sha256": digest(output / "source-graph.json"),
               "compiled_features_sha256": digest(output / "compiled-features.json"),
               "consumer_lock_sha256": digest(consumer / "Cargo.lock"),
               "rustc": subprocess.check_output(["rustup", "run", toolchain, "rustc", "-vV"], env=env, text=True),
               "native_compiler": subprocess.check_output(["cc", "--version"], env=env, text=True).splitlines()[0],
               "scope": "downstream source/library/feature/license/build gate only; no identity/store/endpoint/host/Flutter runtime activation"}
    (output / "library-qualification.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(output / "library-qualification.json")


if __name__ == "__main__":
    main()
