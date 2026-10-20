#!/usr/bin/env bash
# Builds the browser node (wasm) into web/src/pkg and the UI into web/dist.
#   scripts/build-web.sh [--wasm-only]
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --target wasm32-unknown-unknown -p umis-web
wasm-bindgen --target web --out-dir web/src/pkg target/wasm32-unknown-unknown/release/umis_web.wasm
if [ "${1:-}" != "--wasm-only" ]; then
  (cd web && { [ -d node_modules ] || npm ci; } && npm run build)
fi
