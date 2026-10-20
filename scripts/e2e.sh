#!/usr/bin/env bash
# Browser end to end: builds the wasm node and the UI, starts a 4-node devnet
# whose bootstrap serves web/dist, and runs the Playwright specs in web/e2e
# (system chromium by default; CHROME=/path/to/chrome to override).
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/build-web.sh
cargo build --release -p umis-cli
DETACH=1 scripts/devnet.sh 4
trap 'scripts/devnet.sh stop' EXIT
for _ in $(seq 100); do curl -sf http://127.0.0.1:8080/bootstrap.json > /dev/null && break; sleep 0.1; done
(cd web && npx playwright test "$@")
