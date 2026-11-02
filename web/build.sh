#!/usr/bin/env bash
# Build the emulator's WebAssembly into web/pkg. Needs the wasm32 target and
# the wasm-bindgen CLI at the version pinned in crates/bat-app-web/Cargo.toml.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p bat-app-web --release --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir web/pkg \
  target/wasm32-unknown-unknown/release/bat_app_web.wasm
echo "Built web/pkg. Serve web/ with: python3 -m http.server 8742 --directory web"
