#!/usr/bin/env bash
# Builds the wasm SpikeNode into web/spike/pkg.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --target wasm32-unknown-unknown -p umis-web
wasm-bindgen --target web --out-dir web/spike/pkg target/wasm32-unknown-unknown/release/umis_web.wasm
