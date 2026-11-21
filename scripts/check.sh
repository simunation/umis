#!/usr/bin/env bash
# Full local/CI gate: every milestone must end green on this script.
#   scripts/check.sh            fmt + clippy + tests (native). The network is native-only
#                               since 2026-11-22 (ROADMAP §1): this is the whole gate.
#   WITH_WASM=1 ...             also the frozen browser build: wasm32 check and clippy,
#                               wasm tests, web/ type check (legacy, not required)
#   SKIP_WASM_TEST=1 ...        with WITH_WASM: skip the wasm-bindgen node test run
#   SKIP_WEB=1 ...              with WITH_WASM: skip the web/ type check
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"

# The frozen browser build (WITH_WASM=1 only).
WASM_CRATES=(umis-core umis-task task-water3d task-life2d umis-tasks umis-proof umis-stark umis-protocol umis-net umis-node umis-web)
# `tests/wasm.rs` tests run under node via wasm-bindgen-test-runner: determinism
# (golden trace, golden STARK proof) and the wasm prover.
WASM_TESTS=("umis-core" "task-water3d --features stark" "umis-stark")

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }

step "cargo fmt --check"
cargo fmt --all -- --check

step "cargo clippy (native, all targets)"
cargo clippy --workspace --all-targets -- -D warnings

step "cargo test (native)"
cargo test --workspace --quiet

if [ -z "${WITH_WASM:-}" ]; then
  step "all checks passed (native; WITH_WASM=1 adds the frozen browser build)"
  exit 0
fi

step "cargo check (wasm32-unknown-unknown)"
pkgs=()
for c in "${WASM_CRATES[@]}"; do pkgs+=(-p "$c"); done
cargo check --target wasm32-unknown-unknown "${pkgs[@]}"

step "cargo clippy (wasm32-unknown-unknown)"
cargo clippy --target wasm32-unknown-unknown "${pkgs[@]}" -- -D warnings

step "duplicate wasm-bindgen versions (wasm32)"
dups=$(cargo tree --target wasm32-unknown-unknown -p umis-web -d -e normal 2>/dev/null \
  | grep -E '^(wasm-bindgen|js-sys|web-sys) v' | sort -u || true)
if [ -n "$dups" ]; then
  echo "duplicate wasm-bindgen family crates in the wasm build:"
  echo "$dups"
  exit 1
fi
echo "none"

if [ -z "${SKIP_WASM_TEST:-}" ]; then
  if command -v wasm-bindgen-test-runner >/dev/null 2>&1; then
    step "wasm tests (node)"
    for c in "${WASM_TESTS[@]}"; do
      # shellcheck disable=SC2086
      cargo test --target wasm32-unknown-unknown -p $c --test wasm
    done
  else
    echo "wasm-bindgen-test-runner not installed; skipping wasm tests"
  fi
fi

if [ -z "${SKIP_WEB:-}" ] && [ -f web/package.json ] && [ -d web/node_modules ]; then
  step "web: type check"
  (cd web && npx tsc --noEmit)
fi

step "all checks passed"
