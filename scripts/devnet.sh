#!/usr/bin/env bash
# Local devnet: a bootstrap (relay + rendezvous + faucet + web UI) and N native
# full nodes (the genesis validators).
#
#   scripts/devnet.sh [N=4]          fresh genesis, run in the foreground (Ctrl-C stops all)
#   DETACH=1 scripts/devnet.sh 4     start in the background (stop: scripts/devnet.sh stop)
#   scripts/devnet.sh stop
#   CLI_FEATURES=cuda UMIS_GPU_PROVE=1 UMIS_GPU=3090 scripts/devnet.sh 4   nodes prove life2d on the GPU
#   MEMBERS=6 scripts/devnet.sh 4   also start 6 native nodes that join as zone members
#                                 (devnet validators are full nodes; zones get members only from joiners)
#
# Logs, keys and status endpoints live in .devnet/. `umis-cli status --all`
# compares the running nodes.
set -euo pipefail
cd "$(dirname "$0")/.."
DIR=.devnet
BIN=target/release/umis-cli
HTTP=${HTTP:-127.0.0.1:8080}

if [ "${1:-}" = "stop" ]; then
  if [ -f "$DIR/pids" ]; then
    xargs -r kill < "$DIR/pids" 2>/dev/null || true
    rm -f "$DIR/pids"
  fi
  exit 0
fi

N=${1:-4}
cargo build --release -p umis-cli ${CLI_FEATURES:+--features $CLI_FEATURES}
[ -f "$DIR/pids" ] && { xargs -r kill < "$DIR/pids" 2>/dev/null || true; }
rm -rf "$DIR"
mkdir -p "$DIR"
"$BIN" genesis --validators "$N" --start-in "${START_IN:-12}" --keys-dir "$DIR/keys" --out "$DIR/genesis.json" ${GENESIS_ARGS:-}

WEB=()
[ -d web/dist ] && WEB=(--serve-web web/dist)
"$BIN" bootstrap --genesis "$DIR/genesis.json" --data-dir "$DIR/bootstrap" --faucet-key "$DIR/keys/faucet.key" \
  --http "$HTTP" "${WEB[@]}" > "$DIR/bootstrap.log" 2>&1 &
echo $! >> "$DIR/pids"
for _ in $(seq 100); do curl -sf "http://$HTTP/bootstrap.json" > /dev/null && break; sleep 0.1; done

for i in $(seq 0 $((N - 1))); do
  "$BIN" node --genesis "$DIR/genesis.json" --key "$DIR/keys/node-$i.key" --data-dir "$DIR/node-$i" \
    --bootstrap-http "http://$HTTP" > "$DIR/node-$i.log" 2>&1 &
  echo $! >> "$DIR/pids"
done
for i in $(seq 0 $((${MEMBERS:-0} - 1))); do
  "$BIN" node --genesis "$DIR/genesis.json" --key "$DIR/keys/member-$i.key" --data-dir "$DIR/member-$i" \
    --bootstrap-http "http://$HTTP" --join member > "$DIR/member-$i.log" 2>&1 &
  echo $! >> "$DIR/pids"
done
echo "devnet: bootstrap http://$HTTP, $N nodes and ${MEMBERS:-0} joining members (logs in $DIR/)"

if [ -z "${DETACH:-}" ]; then
  trap 'xargs -r kill < "$DIR/pids" 2>/dev/null || true; rm -f "$DIR/pids"' EXIT INT TERM
  wait
fi
