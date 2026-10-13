#!/usr/bin/env python3
"""Qualify the existing encrypted Iroh/RDS channel and browser presentation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib


def run(args, **kwargs):
    subprocess.run(args, check=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--domain", type=Path, required=True)
    parser.add_argument("--provider-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--wasm-bindgen", type=Path, required=True)
    parser.add_argument("--playwright-module", type=Path, required=True)
    parser.add_argument("--browser", type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    if output.is_relative_to(repo):
        raise ValueError("browser evidence must stay outside the repository")
    output.mkdir(mode=0o700, parents=True, exist_ok=True)
    provider = args.provider_root.resolve()
    run(["python3", str(repo / "scripts/prepare-browser-build.py"), str(provider)])
    engine = provider / "build/engine"
    quote = lambda value: json.dumps(str(value))
    domain = args.domain.resolve()
    package = tomllib.loads((domain / "Cargo.toml").read_text())["package"]
    common = ["[package]", 'version="0.0.0"', 'edition="2024"', 'license="AGPL-3.0-only"', "[dependencies]",
              f'nddev-device-sync-domain={{package={quote(package["name"])},version={quote("=" + package["version"])},path={quote(domain)}}}']
    patches = ["[patch.crates-io]"] + [f'{name}={{path={quote(engine / "vendor" / name)}}}' for name in ["iroh", "noq", "noq-proto"]]
    client = output / "client"
    fixture = output / "fixture"
    for project in [client, fixture]:
        (project / "src").mkdir(parents=True, exist_ok=True)
    manifest = common.copy()
    manifest.insert(1, 'name="nds-browser-screen-proof"')
    manifest.insert(5, '[lib]\ncrate-type=["cdylib","rlib"]')
    manifest += [f'iroh={{version="=1.3.0",path={quote(engine / "vendor/iroh")},features=["unstable-custom-transports"]}}',
                 f'rds-core={{version="=0.1.0",path={quote(engine / "crates/rds-core")}}}',
                 'wasm-bindgen="=0.2.129"', 'wasm-bindgen-futures="=0.4.79"', 'js-sys="=0.3.106"',
                 'tokio={version="1",default-features=false,features=["io-util","sync"]}',
                 'postcard={version="1",features=["use-std"]}', 'serde={version="1",features=["derive"]}',
                 'serde-wasm-bindgen="0.6"', 'n0-future="0.3"', 'futures-util="0.3"', 'rand="0.10"', 'ws_stream_wasm="=0.7.5"']
    manifest += patches + [f'ws_stream_wasm={{path={quote(provider / "build/ws_stream_wasm")}}}']
    (client / "Cargo.toml").write_text("\n".join(manifest) + "\n")
    (client / "src/lib.rs").write_text(
        'pub use iroh::endpoint::{ClosedStream, SendStream};\n'
        f'#[path = {quote(engine / "crates/rds-net/src/wire.rs")}]\npub mod wire;\n'
        f'#[path = {quote(repo / "qualification/browser/transport.rs")}]\npub mod transport;\n'
        f'#[path = {quote(repo / "qualification/browser/receiver_probe.rs")}]\npub mod receiver_probe;\n')
    manifest = common.copy()
    manifest.insert(1, 'name="nds-browser-screen-fixture"')
    for name, features in [("rds-client", []), ("rds-core", []), ("rds-desktop", ["x11"]), ("rds-agent", ["desktop", "transport-noq"]), ("rds-net", ["transport-noq"])]:
        manifest.append(f'{name}={{version="=0.1.0",path={quote(engine / "crates" / name)},features={json.dumps(features)}}}')
    manifest += ['tokio={version="1",features=["full"]}', 'tokio-util={version="0.7",features=["rt"]}',
                 'ed25519-dalek="3"', 'rand="0.10"', 'tempfile="3"', 'x11rb="0.14"',
                 'axum={version="0.8",features=["json","ws"]}', 'serde_json="1"', 'iroh-relay={version="=1.3.0",features=["server"]}']
    manifest += patches + [f'netlink-packet-core={{path={quote(provider / "build/netlink-packet-core")}}}']
    (fixture / "Cargo.toml").write_text("\n".join(manifest) + "\n")
    (fixture / "src/main.rs").write_text(f'include!({quote(repo / "qualification/browser/fixture.rs")});\n')
    for project, filename in [(client, "client.Cargo.lock"), (fixture, "fixture.Cargo.lock")]:
        shutil.copyfile(repo / "qualification/browser" / filename, project / "Cargo.lock")
    env = dict(os.environ, RUSTUP_TOOLCHAIN="1.99.0")
    client_env = dict(env, CARGO_TARGET_DIR=str(output / "wasm-target"))
    native_env = dict(env, CARGO_TARGET_DIR=str(output / "native-target"))
    if os.environ.get("NDS_BROWSER_WASM_TARGET"):
        client_env["CARGO_TARGET_DIR"] = os.environ["NDS_BROWSER_WASM_TARGET"]
    if os.environ.get("NDS_BROWSER_NATIVE_TARGET"):
        native_env["CARGO_TARGET_DIR"] = os.environ["NDS_BROWSER_NATIVE_TARGET"]
    for project, target, build_env in [(client, "wasm32-unknown-unknown", client_env), (fixture, "x86_64-unknown-linux-gnu", native_env)]:
        metadata = subprocess.check_output(["rustup", "run", "1.99.0", "cargo", "metadata", "--manifest-path", str(project / "Cargo.toml"), "--locked", "--format-version", "1", "--filter-platform", target], env=build_env)
        (project / "compiled-graph.json").write_bytes(metadata)
        run(["cargo", "deny", "--manifest-path", str(project / "Cargo.toml"), "--config", str(repo / "deny.toml"), "--target", target, "check"], env=build_env)
        run(["rustup", "run", "1.99.0", "cargo", "clippy", "--manifest-path", str(project / "Cargo.toml"), "--locked", "--target", target, "--", "-D", "warnings"], env=build_env)
        compiled = subprocess.run(["rustup", "run", "1.99.0", "cargo", "build", "--manifest-path", str(project / "Cargo.toml"), "--locked", "--target", target, "--message-format=json", "-j", "4"], env=build_env, text=True, stdout=subprocess.PIPE)
        (project / "build.jsonl").write_text(compiled.stdout)
        records = [json.loads(line) for line in compiled.stdout.splitlines() if line.startswith("{")]
        if compiled.returncode:
            for item in records:
                if item.get("reason") == "compiler-message" and item["message"].get("rendered"):
                    sys.stderr.write(item["message"]["rendered"])
            raise RuntimeError("browser qualification build failed; private diagnostics retained")
        features = [{"package_id": item["package_id"], "features": item["features"], "kind": item["target"]["kind"]}
                    for item in records if item.get("reason") == "compiler-artifact"]
        (project / "compiled-features.json").write_text(json.dumps(features, indent=2) + "\n")
    assets = output / "assets"
    (assets / "pkg").mkdir(parents=True, exist_ok=True)
    run([str(args.wasm_bindgen), str(Path(client_env["CARGO_TARGET_DIR"]) / "wasm32-unknown-unknown/debug/nds_browser_screen_proof.wasm"), "--target", "web", "--out-dir", str(assets / "pkg")])
    shutil.copyfile(repo / "qualification/browser/index.html", assets / "index.html")
    executable = Path(native_env["CARGO_TARGET_DIR"]) / "x86_64-unknown-linux-gnu/debug/nds-browser-screen-fixture"
    run(["python3", str(repo / "scripts/owned-x11.py"), "--", "node", str(repo / "qualification/browser/check.cjs"), str(executable), str(assets), str(output), str(args.playwright_module.resolve()), str(args.browser.resolve())], env=env)
    # Validate actual browser compositor output, independently of canvas reads.
    from PIL import Image
    pixels = {}
    for name, red in [("red.png", True), ("blue.png", False)]:
        pixel = Image.open(output / name).convert("RGB").getpixel((108, 108))
        if not ((pixel[0] > 180 and pixel[2] < 70) if red else (pixel[2] > 180 and pixel[0] < 70)):
            raise ValueError("browser compositor pixels did not match the real X11 source")
        pixels[name] = pixel
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    receipt = {"source": json.loads((repo / "providers/screen-source.lock.json").read_text()),
               "browser_overlay": json.loads((repo / "providers/browser-transport.lock.json").read_text()),
               "toolchain": "1.99.0", "transport": "Iroh WASM endpoint via own relay, existing rds/0 and peer-authenticated encrypted channel", "server_media_decryption": False,
               "outer_relay": "isolated loopback HTTP/WS fixture; production HTTPS/WSS unqualified",
               "proof": "real transport endpoint signature and one-use fixture challenge; NDS device proof/store integration unqualified",
               "pixels": pixels, "browser": json.loads((output / "browser-results.json").read_text())["browser"],
               "scope": "Linux Chrome isolated native capture -> encrypted relay -> browser decoder/canvas; NDS authority/production integration unqualified",
               "domain_package": package["name"], "domain_manifest_sha256": digest(domain / "Cargo.toml"),
               "domain_screen_sha256": digest(domain / "src/screen.rs"),
               "source_sha256": {p.name: digest(p) for p in sorted((repo / "qualification/browser").iterdir()) if p.is_file()},
               "compiled": {name: {file: digest(project / file) for file in ["Cargo.lock", "compiled-graph.json", "compiled-features.json"]} for name, project in [("client", client), ("fixture", fixture)]}}
    (output / "browser-qualification.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
