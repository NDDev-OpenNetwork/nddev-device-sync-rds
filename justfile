set shell := ["bash", "-euo", "pipefail", "-c"]

check:
    cargo fmt -- --check
    rustfmt --edition 2024 --check src/screen.rs qualification/screen.rs
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked

provider-check:
    cargo test --locked --test provider -- --ignored

screen-check:
    python3 scripts/check-screen.py --domain "$NDS_SCREEN_DOMAIN" --provider-root "$NDS_SCREEN_PROVIDER_ROOT" --legacy-provider "$NDS_SCREEN_LEGACY_PROVIDER" --output "$NDS_SCREEN_OUTPUT"
