#!/usr/bin/env bash
# Native devnet end to end: 4 nodes agree, a node is killed and restarted, a
# late joiner syncs from a checkpoint and becomes a zone member.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=target/release/umis-cli
DIR=.devnet
DETACH=1 scripts/devnet.sh 4
trap 'scripts/devnet.sh stop' EXIT
status() { "$BIN" status --all; }
head_of() { curl -s "$(cat "$DIR/node-$1/http.addr")/status" | python3 -c 'import sys,json; print(json.load(sys.stdin)["head_slot"])'; }

echo "== waiting for the lattice (40 s)"; sleep 40
status
echo "== kill node-2"
pid=$(sed -n 4p "$DIR/pids"); kill "$pid"
h0=$(head_of 0); sleep 20; h1=$(head_of 0)
echo "node-0 head $h0 -> $h1 with node-2 down"
[ "$h1" -gt "$h0" ] || { echo "lattice stalled"; exit 1; }
echo "== restart node-2"
"$BIN" node --genesis "$DIR/genesis.json" --key "$DIR/keys/node-2.key" --data-dir "$DIR/node-2" \
  --bootstrap-http http://127.0.0.1:8080 >> "$DIR/node-2.log" 2>&1 &
echo $! >> "$DIR/pids"
echo "== late joiner (member)"
"$BIN" node --genesis "$DIR/genesis.json" --key "$DIR/keys/joiner.key" --data-dir "$DIR/joiner" \
  --bootstrap-http http://127.0.0.1:8080 --join member > "$DIR/joiner.log" 2>&1 &
echo $! >> "$DIR/pids"
sleep 45
status
zone=$(curl -s "$(cat "$DIR/joiner/http.addr")/status" | python3 -c 'import sys,json; s=json.load(sys.stdin); print(s["zone"], s["minted"])')
echo "joiner zone/minted: $zone"
[ "${zone%% *}" != "None" ] || { echo "joiner did not join"; exit 1; }
echo "DEVNET OK"
