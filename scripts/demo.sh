#!/usr/bin/env bash
# Demo: builds the browser node and UI, starts a 4-node devnet whose bootstrap
# serves the UI, prints a status line every 8 s. Open the printed URL (add
# `?n=2` to start two browser nodes right away). Ctrl-C stops everything.
set -euo pipefail
cd "$(dirname "$0")/.."
scripts/build-web.sh
DETACH=1 scripts/devnet.sh "${1:-4}"
trap 'scripts/devnet.sh stop' EXIT INT TERM
echo
echo "  UI:      http://127.0.0.1:8080/?n=2"
echo "  status:  cargo run --release -p umis-cli -- status --all"
echo
while sleep 8; do
  curl -s http://127.0.0.1:8080/status | python3 -c '
import sys, json
s = json.load(sys.stdin)
z = " ".join(f"z{v[\"zone\"]}:{(v[\"last_commit\"] or [None])[0]}" for v in s["zones"])
print(f"beacon {s[\"head_slot\"]} final {s[\"finalized_slot\"]} | last commits {z} | slashes {s[\"slashes\"]} rollbacks {s[\"rollbacks\"]} | proposals {len(s[\"proposals\"])}")
' || true
done
