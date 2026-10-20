# simunation architecture (as implemented)

This document maps the plan (`.plan/simunation-poc-plan.md`) onto the code,
and lists every place where the implementation deviates from it and why.
Symbols follow the plan's Notation section (beacon slot `e`, zone slot `j`,
`B`, `S_z`, `D`, `K_f`, `R`, `L`, …).

## Layers

```
 web/ (TS + three.js)      umis-cli (native bins)
        │ wasm-bindgen              │ axum HTTP, clap
   umis-web (SimNode)          umis-node::Runtime  ◄── same loop on tokio and wasm
        └──────────────┬───────────┘
                umis-node::Runtime  ── select_biased!: swarm events, proof results, control, timers last
                       │                         │
                umis-net::Net (libp2p)    umis-protocol::Node  ── sans-IO: handle(now, Event) -> Vec<Action>
                                                 │
            umis-proof · umis-task · task-water3d · umis-core
```

`umis-protocol::Node` never touches IO. `Action`s are Publish, Validate
(gossipsub app-level validation), Request/Respond (`/simu/sync/1`, borsh),
Subscribe/Unsubscribe, Peering (rendezvous namespaces), WakeAt and Ui. The
same core runs in `umis-simnet` with a virtual clock and network.

## Core data (umis-core)

- **bmt** is a line-by-line port of bmt-js 3.0.2, including its quirks: JS
  `>>>` shift semantics (`ushr`), the leaf-chunk option-name mismatch
  (`maxPayloadLength` vs `maxPayloadSize`), and `getBmtIndexOfSegment`'s
  `% SEGMENT_SIZE` test (ported as is; the one case where bmt-js would loop
  forever stops instead). Zero-subtree hashes make a 10-segment record cost
  ~17 keccak calls. `tests/vectors/bmt-vectors.json` is produced by running
  the real `@nugaon/bmt-js@3.0.2` (`scripts/bmt-vectors.mjs`) over edge-case
  sizes up to 128²·4096+4096 bytes and over a sample of every `seg` record;
  the Rust test matches addresses, file inclusion proofs and every record
  field's segment proof byte for byte, natively and on wasm32.
- **seg**: `seg_record!` generates the struct, `encode`/`decode`
  (canonical: zero padding enforced), `address`, `prove_field`, and
  compile-time layout assertions. All 18 committed record types live in
  `records.rs`.
  *Deviation:* `ZoneBlockHeader` has an extra scalar `from_slot` (a block
  computes slots `from_slot ..= slot`): recovery blocks and blocks after a
  missed slot cover several zone slots and the header must commit to that.
- **time**: `TimeGrid` and `ZoneTiming` implement every formula of the
  Notation section (`k_z`, `m_z`, `P_z`, `n_z`, `j*`, commit ranges, fallback
  windows), with property tests.
- **genesis**: JSON; its hash is the bmt file address of its canonical JSON.

## Tasks (umis-task, task-water3d)

`ZoneTask` is the plan's trait plus `type Ctx` and `prepare(params, seed)`:
the genesis field (background current, stencil weights, forcing) is
precomputed once per zone and cached by `params_hash`. `DynZoneTask` /
`ZoneInstance` erase the types. `check_zone` lets a task check that proposal
params match the location and slot length.

water3d follows §3: the background current is the discrete curl of a vector
potential of random vortex tubes, jets and a drift, so it is exactly
discretely divergence-free (tested); dye sources and a tide table come from
`H(genesis.seed ‖ "water3d")` via ChaCha20; the update is one integer stencil
per value with non-negative weights summing to ≤ 2^s. Additions: a per-cell
**flux limiter** scales the advection weights down where the current would
violate CFL (keeps every weight ≥ 0 without a global pass), and the genesis
`focus` box concentrates features and dye sources in the genesis region so
the first zones show structure. `tests/golden-trace-8.txt` is checked by a
native test and by a wasm-bindgen test under node (native == wasm).

## Protocol (umis-protocol)

### Beacon chain

`BeaconState::apply_block` is the deterministic transition: housekeeping
(missed proof deadlines, expired proposals, unbonding, heartbeat removals),
txs (Join, Leave, Heartbeat, Grant, ProposeZone, FaultEvidence), zone
commitments, proof commitments, then cross-link resolution, rollback records
and settlement. `BeaconChain` keeps the block tree, fork choice (most blocks,
then lower proposer rank at the divergence, then lower hash), `DepthK(K_f)`
finality behind a `FinalityRule` trait, and branch-aware anchors
(`anchor(e)` = newest block with slot ≤ `e − D` on the branch being
validated).

**Commitment envelopes.** A `ZoneCommitEnvelope` in a beacon block carries
the covered zone blocks' signed headers, InputRefs and votes (no payloads).
Beacon validators therefore check the chain linkage, minter legitimacy under
each slot's anchor, InputRef causality/monotonicity, `inputs_root`, `deps`
and the vote tallies themselves, and later cross-link every InputRef against
the producer's committed blocks. *Deviation:* the plan lists only `deps`
in the body; the full InputRef lists are needed to detect orphaned inputs
that `deps` (newest per link) would hide.

**Commitments may cover several commit periods** (covered range =
`[last valid commitment's j*, j*(e))`). This is what lets a zone recover
after a rollback or a missed delegate, and why `ProofCommitment.leaf_count`
equals the covered slots rather than always `n_z`.

**Settlement** = proven ∧ included ≥ `D` slots ago ∧ all inputs resolved ∧
previous commitment settled ∧ every producer commitment it consumed from
settled, computed as a greatest fixed point so input cycles settle together.
Settled commitments are never invalidated. The settle delay `D` is there
because a mock "proof" is only a signature: faults are found by re-execution,
which needs a moment.

**Rollbacks are synchronized.** *Deviation from the plan's per-zone scope:*
any invalidation (fault, cross-link failure, missed proof) writes a rollback
record for *every* active zone with the same `e_record`: affected zones roll
back to their newest valid commitment before the contamination, the others
just drop their uncommitted tail. All zones recover at `e_record + D`, and
blocks minted under an anchor that predates the rollback record can neither
be committed nor consumed.
With bidirectional face links, per-zone rollbacks caused endless cascades in
SimNet (each zone's recovery window produced blocks its neighbours consumed);
the synchronized form ends every fault after one recovery. Before invalidating,
the beacon also invalidates consumer commitments that took inputs from a
producer's discarded range, to a fixed point.

**Faults.** InvalidTransition is verified by re-executing the block from the
parent state in the evidence; InvalidInputRef by the cheap rules (link,
causality, attachment, producer legitimacy under the producer's anchor,
monotonicity against the parent block in the evidence); Equivocation by two
signed headers. Each fault key is punished once; slash/reward/burn as in the
plan. The faulty block's hash is recorded as discarded; a commitment
containing it is invalidated (under mock proofs honest members reject the
block, so the producer zone normally keeps its honest fallback block and only
the consumers that used the bad outputs roll back).

**Zone creation** follows §1: registration checks (bounds, layer free and not
locked, linked-lock rule, adjacency via `links_between`, founders, spam
limits), voter snapshots, votes recorded in zone blocks and recounted by the
beacon from the committed blocks, Accepted/Rejected/Expired, activation at
`e_act` with links in both directions and the founders as members.
*Deviation:* founders switch zones at the decision block (effective through
anchors), not at `e_act`. Refused `ProposeZone` txs are dropped from every
pool on receipt (a refusal is final; the proposer resubmits).

### Zone chains

`ZoneEngine` keeps one zone's block tree. Fork choice for slot `j` uses the
anchor state of `j`: longest chain from the anchor's latest valid commitment,
ties to lower minter rank then lower hash; a block is valid only if every
block since that commitment used its slot's canonical anchor and was minted
under an anchor that already holds the zone's latest rollback record. Minting: rank `r ≤ R` mints at
`slot_start + r·W_z` if no valid block covers the slot; inputs are the
freshest causal producer outputs that are not discarded, not minted before a
producer rollback, and not in conflict with the producer's committed chain;
otherwise the last value is held. Blocks covering several slots (after a gap
or as a recovery block) hold every input.

*Deviation:* `ZoneBlock` gossip carries **input attachments** (the producer's
signed header, the port payload and its inclusion proof per InputRef) instead
of the state blob. Members re-execute every block (mock mode) and do not need
the parent state from the network; blocks are self-contained for validation,
fault evidence and sync.

Delegates build commitments against the head state *plus pooled commitments*
(`BeaconState::with_pending`): for zones with `S_z = B` the commitment for
`e` is made before the one for `e − 1` is on chain.

### Sync

Status polls; `BeaconBlocksBySlot` from the local finalized slot − 16;
`BeaconBlockByHash` for orphans; `GetCheckpoint` (state + tip block + held
inputs, verified against a valid commitment) then `ZoneBlocks` for joiners
and returning nodes. DepthK finality is not partition safe: after a
partition heals a node follows a strictly longer chain past its local
finality (counted as `finality_reverts`).

## Networking (umis-net, umis-node)

As in §5: QUIC + TCP/WS + WebRTC-direct listeners natively, relay and
rendezvous servers on the bootstrap, relay client + DCUtR on nodes,
`webrtc-websys` + `websocket-websys` + relay client in the browser,
gossipsub with keccak message ids and app-level validation, request-response
with a length-prefixed byte codec. Peering targets per rendezvous namespace
(`simu/<g8>/all`, `simu/<g8>/zone/<z>`) come from the protocol's Peering
action. *Not done:* autonat and NTP-lite clock offset estimation.

### Known issues

- **WebRTC under load.** With libp2p-webrtc 0.10.0-alpha (native) and
  libp2p-webrtc-websys 0.5.0 (browser), (a) a single write above 16 KiB
  fails, so the sync codec writes in 4 KiB pieces with a flush after each;
  (b) the browser resets a data channel as soon as more than 16 KiB of
  messages queue up while the page is busy (patched in `vendor/`, see
  `vendor/README.md`); (c) even then, long-lived gossipsub streams over
  WebRTC keep breaking until gossipsub's handler gives up after 5 substream
  attempts, which silently stops the browser's outbound gossip. Browsers
  therefore reach native nodes over **WebSocket by default**
  (`NetConfig::prefer_webrtc`, `?transport=webrtc` in the UI to switch);
  browser↔browser traffic goes through relay circuits over that connection.
  The M1 spike (`scripts/spike-e2e.mjs`) still exercises WebRTC-direct.
- wasm-bindgen 0.2.108 is pinned by libp2p-swarm (`wasm-bindgen-futures
  =0.4.58`).
- `closure invoked recursively or after being dropped` page errors come
  from libp2p-webrtc-websys data channel callbacks after a stream is dropped;
  they are harmless.

## Proofs (umis-proof, umis-stark, task-water3d::air)

The genesis selects the backend (`proof_backend`: `mock` or `plonky3`, plus
`stark_config_id`: `koalabear` = Plonky3's benchmark FRI preset, or
`koalabear-test` = 4 queries, insecure, for tests). Every node, native or
browser, builds it with `task_water3d::air::backend_for(genesis)`.

**Field: KoalaBear** (7a). The toy AIR (`umis-stark::toy`, a 1D stencil with a
LogUp memory bus, a range-check lookup and periodic public columns, 50
columns) was measured on both candidate fields (`examples/bench_toy.rs`,
`tests/wasm.rs` with `--include-ignored`):

| rows × 50 cols, benchmark FRI | Goldilocks native (24 threads) | KoalaBear native | Goldilocks wasm | KoalaBear wasm |
|---|---|---|---|---|
| 2^14: prove / verify / proof | 57 ms / 6 ms / 266 KB | 65 ms / 8 ms / 243 KB | 946 ms / 12 ms | 617 ms / 8 ms |
| 2^16: prove / verify / proof | 210 ms / 9 ms / 350 KB | 223 ms / 11 ms / 334 KB | 3778 ms / 15 ms | 2059 ms / 9 ms |

Natively the two are within ~10%; in wasm, contrary to the plan's
expectation (Goldilocks' simd128 path), KoalaBear proves 1.5–1.8× faster,
and its proofs are smaller. Browser workers are the slowest provers, so the
genesis pins KoalaBear; the water stencil fits its 31-bit range (values are
range-checked to |q| ≤ 2^15 and the weights sum to ≤ 2^s = 2^12, so every
stencil identity stays far below p/2 and holds over the integers). Plonky3 is pinned to `=0.8.0`; `parallel` is on natively,
off in wasm.

**The water3d AIR** (`task-water3d/src/air.rs`, feature `stark`) follows §4's
tall layout: one row per (substep, cell), height `k_sub·n_loc³`. 46 main
columns (`q`, six neighbours, `qn`, remainders, 8-bit limbs of `qn`, table
multiplicities), padded to at least `2^s` rows for the remainder table; 32
preprocessed columns committed once per zone (addresses,
selectors, the zone's genesis stencil weights and forcing shapes, the
range tables), 32 periodic columns the verifier evaluates from the public
states and halos (`X_in`, `X_out`, the six halo faces). Neighbour reads use a
LogUp memory bus; remainders and limbs are looked up in `2^s` and `2^8`
tables. Batch-STARK does not absorb periodic data into Fiat–Shamir, so the
public values carry a keccak digest of `params_hash ‖ slot ‖ X_in ‖ X_out ‖
halos` plus `tide(j)`. Tests: the witness equals `step()` bit for bit,
tampered cells, halos, outputs and remainders are rejected, and
`tests/stark.rs` / `tests/wasm.rs` pin the address of a test-preset proof
(no grinding, hence deterministic: native grinding is a parallel search that
returns any valid nonce), so the wasm prover is checked against the native
one byte for byte.

One slot, benchmark FRI preset, release builds (`examples/bench_air.rs`;
wasm: the ignored `stark_bench_in_wasm` test under node, single-threaded as
in a browser worker):

| `n_loc` | rows | prove, native 24 threads | native 2 threads | wasm | verify (native / wasm) | proof |
|---|---|---|---|---|---|---|
| 4 | 4096 | 32 ms | — | 176 ms | 7 / 7 ms | 196 KB |
| 8 | 4096 | 39 ms | 131 ms | 187 ms | 7 / 6 ms | 187 KB |
| 16 | 16384 | 91 ms | 397 ms | 682 ms | 11 / 11 ms | 261 KB |

**Proof flow.** One leaf per zone slot (`ProofLeaf` = the slot's
`TransitionStatement` + proof address); a `ProofCommitment` is the proof-BMT
root over the leaves of the slots its zone commitment covers. The slot's
*priority prover* (a member or full node other than the minter, ranked by
the anchor's seed) proves the
slot as soon as it accepts the block and gossips a `ProofShare`; the
commitment delegate collects the shares and, `B/8` into the next beacon
slot, proves missing ones itself. With `L ≥ 1` the proof commitment goes into
a later beacon block than its zone commitment; a commitment still optimistic
`L` slots after inclusion rolls back (`MissingProof`). Beacon validators
verify every leaf before accepting a proof commitment.
*Deviation:* no recursive aggregation. The beacon checks each leaf, and the
proof commitment ships `LeafData` (the state at every leaf boundary and each
slot's input payloads) so verifiers can evaluate the periodic columns.

**Proving off the event loop.** The sans-IO node emits `Action::Compute
(ProveJob)` and receives `Event::ComputeDone`. It keeps at most
`NodeConfig::max_jobs` jobs running (1 on native and browser nodes) and
queues the rest: blocks of pooled commitments first (they have a deadline),
then the zone's best chain, earliest slot first; jobs for blocks that are
neither pooled nor valid any more are dropped. Native nodes run jobs on a
dedicated rayon pool (`UMIS_PROVER_THREADS`, default 2; *deviation* from
`spawn_blocking`, see below). Browser nodes post them to Web Workers
(`SimNode.setProver`, `web/src/prover.ts`; each worker loads the same wasm
module and calls `proveJob`; up to half the cores, at most 4, shared by the
page's nodes; `?prover=inline` proves on the page thread).
`umis_proof::JobRunner` caches task instances and the STARK setup per zone.

**Verification cost.** A proof commitment's leaves are verified in parallel
natively (rayon), and both results and proof addresses (a BMT over ~200 KB)
are memoized in `Ctx`, since block building and fork choice re-apply bodies
many times.

**Load lessons.** On a one-machine devnet (4 nodes + bootstrap, 24 cores)
Plonky3 first caused periodic rollbacks. Every symptom looked like a protocol
fault, but each came from CPU or event-loop latency:

- an unbiased `select!` let timers starve network events, and a prover using
  every core slowed the other nodes, so rank-0 zone blocks arrived after the
  fallback window `W_z`. Fallback minters forked, a consumer took the losing
  block, and the cross-link check rolled every zone back. Fix:
  `select_biased!` (network first, timers last) and the 2-thread prover pool
  (instead of `spawn_blocking` onto all cores);
- forks and re-proving after rollbacks pushed provers past capacity, proof
  commitments missed `L`, and that caused the next rollback (`MissingProof`).
  Fix: the bounded, deadline-ordered job queue;
- block building applied its body about eight times; each pass hashed every
  proof twice and verified unseen leaves one by one, about 300 ms on the event
  loop. Fix: the memos and parallel verification (about 50 ms).

With these fixes, a full-size (`n_loc = 16`) Plonky3 devnet ran for 5 minutes with 0
zone forks and 0 rollbacks. The `forks`, `proved` and `jobs_dropped`
counters (`/status`, `/metrics`) and debug logs (`RUST_LOG=umis_node=debug`:
slow events, job wait and prove times) make this visible.

## SimNet (umis-simnet)

A virtual clock with seeded latencies, drops, partitions, offline nodes and
per-(sender, topic) delay rules, an idealized gossip mesh, routed
request-response, a shared signature cache and a thread-local rank cache.
Scenarios and the oracle are described in the README. Exit criterion of M4:
`cargo run --release -p umis-simnet -- --seeds 100 --scenario basic --slots 200`
(4 zones × 8 members + 2 full nodes, 200 beacon slots) passes with every zone
committing in every beacon slot.

Not covered by a scenario: the registering beacon block being reorged out
(hard to stage deterministically).
