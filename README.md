# simunation

A peer-to-peer simulation network: one Rust client that runs natively and in
the browser (wasm) from the same code. Nodes form a **block lattice**:

- **zones** are mini-chains, each running a pluggable simulation task (the
  first one: a 3D water current) under leader-based minting, with its own
  slot length;
- every zone commits its work, with proofs, to a shared **beacon chain**,
  which defines global time, randomness, membership and stake;
- neighbouring zones stream boundary faces to each other directly over
  libp2p; inputs are used immediately and verified in parallel, and a failed
  check rolls the affected zones back and slashes the faulty producer;
- new zones are created at runtime at a free location (point + layer) after
  the adjacent zones vote on their own chains.

The design is in [`.plan/simunation-poc-plan.md`](.plan/simunation-poc-plan.md);
how the code realises it, and where it deviates, is in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Layout

| Path | What |
|---|---|
| `crates/umis-core` | keccak256, the bmt-js port (chunk/file BMT, inclusion proofs), the segment-aligned `seg` codec and all committed records, tick/slot arithmetic, genesis, ed25519 |
| `crates/umis-task` | `ZoneTask` trait + type erasure, task registry, ports/links, layers/locations/couplings, the `Lattice3d` topology, fixed point, the physics reference simulator |
| `crates/task-water3d` | the water-current task: genesis field (divergence-free background current, dye sources, tide), the integer stencil, faces; the devnet genesis; the Plonky3 AIR and `plonky3` backend (feature `stark`) |
| `crates/umis-proof` | `ProofBackend`, transition statements, proof leaves and proof-BMT assembly, proving jobs (`ProveJob`, `JobRunner`), the mock backend |
| `crates/umis-stark` | Plonky3 configurations (KoalaBear, Goldilocks) and FRI presets, the toy AIR and field benchmarks |
| `crates/umis-protocol` | the sans-IO node: beacon state transition, beacon chain (fork choice, finality, anchors), zone engine (validation, minting, fork choice, commitments, proofs), faults, rollback, slashing, zone creation, sync |
| `crates/umis-simnet` | deterministic multi-node harness, scenarios and the invariant oracle |
| `crates/umis-net` | libp2p adapter: transports per role and target, behaviours, codec, topics, rendezvous |
| `crates/umis-node` | the runtime driving a protocol node with the network, the clock and timers (tokio and wasm) |
| `crates/umis-cli` | `genesis`, `keygen`, `node`, `bootstrap`, `status`, `sim` |
| `crates/umis-web` | the browser node (`SimNode`, several per page) and the Web Worker prover (`proveJob`) |
| `web/` | Vite + TypeScript UI: node cards, lattice explorer, three.js 3D view, zone creation and proposals; Playwright specs |
| `scripts/` | `check.sh`, `devnet.sh`, `devnet-test.sh`, `e2e.sh`, `build-web.sh`, `demo.sh`, bmt-js vectors, spike drivers |
| `vendor/` | a patched `libp2p-webrtc-websys` (see `vendor/README.md`) |

## Quick start

Requirements: Rust stable ≥ 1.88 with `wasm32-unknown-unknown`,
`wasm-bindgen-cli 0.2.108` (`cargo install wasm-bindgen-cli --version 0.2.108`),
Node 24, and a Chromium for the browser tests.

```sh
scripts/check.sh                  # fmt, clippy, tests (native + wasm32), wasm determinism
scripts/demo.sh                   # devnet + web UI, open http://127.0.0.1:8080/?n=2
```

Native devnet only:

```sh
scripts/devnet.sh 4               # bootstrap + 4 native full nodes (Ctrl-C stops)
cargo run --release -p umis-cli -- status --all
scripts/devnet-test.sh            # kill/restart a node, a late joiner syncs and becomes a member
```

Browser:

```sh
scripts/build-web.sh              # wasm node + UI into web/dist
scripts/e2e.sh                    # devnet + Playwright (web/e2e/*.spec.ts)
```

In the page: **Add node** starts a browser node (its key is kept in
localStorage); with *auto join* it asks the bootstrap's faucet for test stake
and joins a zone. Each card shows the node's role this slot. The lattice
explorer colours commitments by status (optimistic, proven, settled, invalid)
and lists slashes and rollbacks; click a cell for the commitment and its
proof leaves. Click a ghost block in the 3D view to propose a new zone; the
proposals panel shows the per-zone tallies.

## Proofs

The genesis picks the proof backend. The devnet uses `plonky3`: every zone
slot is proven with the water3d STARK (KoalaBear, Plonky3 0.8), and the
beacon chain verifies every proof. `mock` signs statements and catches
faults by re-execution (SimNet's default):

```sh
scripts/devnet.sh 4                                             # plonky3, 16³ cells per zone
GENESIS_ARGS="--n-loc 8" scripts/devnet.sh 4                    # lighter zones
GENESIS_ARGS="--proof-backend mock" scripts/devnet.sh 4         # no STARKs
cargo run --release -p task-water3d --features stark --example bench_air -- 4 8 16
cargo run --release -p umis-simnet -- --scenario stark --seeds 3
```

Native nodes prove on `UMIS_PROVER_THREADS` threads (default 2); browser
nodes prove in Web Workers. See "Proofs" in `docs/ARCHITECTURE.md`.

## Simulated network

```sh
cargo test -p umis-simnet                                   # every scenario on one seed
cargo run --release -p umis-simnet -- --seeds 100 --scenario basic --slots 200
cargo run --release -p umis-simnet -- --seeds 20 --scenario all
```

Scenarios: `basic`, `churn`, `partition`, `adversary` (equivocation, forged
input), `faults` (invalid transition, orphaned input from a fallback race,
missed proof deadline, overlapping faults), `zone-create` (accept, refused
duplicate/linked proposals, a founder going offline), `zone-reject`,
`zone-expire`, `zone-layers` (a second layer coupled to water's top face),
`zone-sparse` (a sparse adjacent zone), `stark` (Plonky3 proofs, 40 slots).
Each run checks the oracle's
invariants (finality agreement, identical anchors and ranks, registry
agreement, replay of committed zone chains, commitment contiguity and
physical-time alignment, slashing exactly the injected faults, rollback reach,
vote thresholds, liveness gaps).

## Physics reference

```sh
cargo run --release -p umis-cli -- genesis --out /tmp/g.json --keys-dir /tmp/keys
cargo run --release -p umis-cli -- sim --genesis /tmp/g.json --beacon-slots 30
```
