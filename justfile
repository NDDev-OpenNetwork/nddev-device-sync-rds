set shell := ["bash", "-euo", "pipefail", "-c"]

check:
    cargo fmt -- --check
    rustfmt --edition 2024 --check src/screen.rs qualification/screen.rs qualification/browser/transport.rs qualification/browser/fixture.rs qualification/browser/receiver_probe.rs
    rustfmt --edition 2024 --check qualification/library.rs
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked

provider-check:
    cargo test --locked --test provider -- --ignored

screen-check:
    python3 scripts/check-screen.py --domain "$NDS_SCREEN_DOMAIN" --provider-root "$NDS_SCREEN_PROVIDER_ROOT" --legacy-provider "$NDS_SCREEN_LEGACY_PROVIDER" --output "$NDS_SCREEN_OUTPUT"

browser-check:
    python3 scripts/check-browser.py --domain "$NDS_SCREEN_DOMAIN" --provider-root "$NDS_SCREEN_PROVIDER_ROOT" --output "$NDS_BROWSER_OUTPUT" --wasm-bindgen "$NDS_BROWSER_WASM_BINDGEN" --playwright-module "$NDS_BROWSER_PLAYWRIGHT" --browser "$NDS_BROWSER_EXECUTABLE"

library-prepare destination config:
    python3 scripts/prepare-library-provider.py --destination "{{destination}}" --config "{{config}}"
