# umis roadmap (from 2026-11-21)

Status of the proof system against the project goals, and every remaining step in order. Each step has a goal,
the design, acceptance criteria and its dependencies. Session handoff (how to resume, run, verify): `HANDOFF.md`.
Original design: `simunation-poc-plan.md` (its milestones M0–M8 are done; this file continues from there).

## 1. Goals

From the POC plan and the owner's decisions since:

| # | Goal | Source |
|---|---|---|
| G1 | Every zone transition is proven by a STARK that every node (native, browser) can verify | plan §4 |
| G2 | **≥ 100 proven bits** for every proof the network relies on (decided 2026-11-19; 128 not needed) | owner |
| G3 | Proving keeps up with simulation; life2d (the test task) proves at about the native step's cost on a GPU | life2d effort |
| G4 | The beacon chain verifies **one aggregate proof per beacon slot** instead of every leaf proof, without needing full states (succinct verification) | plan §9 future work, REPORT phase 3 |
| G5 | No cheap liveness attacks around proofs (junk shares / proof commitments) | REPORT §4.2 |
| G6 | Native and browser nodes run the same code; browsers can at least prove leaves | plan context |
| G7 | Every soundness-relevant check has a negative test that runs the verifier in release mode; security is computed with p3-security, never `queries × log_blowup + PoW` | CLAUDE.md |

Owner rules that steer choices: prefer the newer (Johnson/DKT26) bound only where it is faster or otherwise
pays off, else the conservative unique-decoding bound (2026-11-20). Benchmarks back to back (shared machine).
Commit only when asked (they asked for every step so far).

## 2. Where we are (commit `b1dc8b2`, branch `life2d-gpu-demo`)

| Goal | State |
|---|---|
| G1 | Done for water3d (batch STARK + LogUp) and life2d (uni/batch STARK, GPU-resident prover). Beacon verifies every leaf (memoized, grouped for batches) |
| G2 | **Leaves done**: `FriPreset::P100` (EF4, blowup 4, 128 queries, grinding at query/OOD/batch/lookup) ≥ 101 bits UDR, asserted per task over the whole accepted shape envelope. **Aggregation done in the prototype**: `umis-agg` over KoalaBear⁵, every layer 102–103 bits (J101 Johnson default, U101 UDR) |
| G3 | Life2d 256×256 k=1 on a 3090 Ti: 0.77 ms/slot with P100 (3 provers × batch 16), native step 0.65 ms. Remaining costs are fixed by the proof format (transcript absorb ~2.7 ms/batch) and the PCIe link (row download ~1.2 ms/batch); GPU FRI measured at ≤ 3 % gain, not built |
| G4 | **Prototype only** (`crates/umis-agg`): leaf → layer 1 → 2-to-1, statement-checked, 1.6–1.8 s and ~400 KB per 2-to-1 step (J101). Not wired into the protocol; leaves must be EF5 (CPU only); per-claim circuit (periodic claim data) |
| G5 | Done: proof commitments checked on receipt (head + pending projection, any L), candidates per signer, eviction, rebuild after inclusion; untrusted shares never block self-proving; SimNet `junk-proofs`, `junk-proofs-l0`, `life-stark-junk` |
| G6 | Browser nodes prove/verify P100 leaves in wasm (single-threaded; not measured for life at P100). EF5 aggregation leaves in wasm: unmeasured |
| G7 | Followed for every change so far (mutation-checked negative tests) |

Decisions already made: 100 bits (not 128); L = 0 allowed with STARKs; Plonky3 pinned `=0.9.0-rc.2`;
Plonky3-recursion git dependency pinned to `3a69ba6` (approved); aggregation over EF5; Johnson preset J101 for
aggregation (faster than UDR there); GPU FRI not built (measured).

Still to decide (owner): canonical state roots (keccak BMT vs Poseidon2, §4 P2), browser role (leaf prover only, or
verifier too), nonlinear physics (raises constraint degree), Plonky3 version policy beyond the current pins.

## 3. Plan

Order: **A** is the main line (aggregation into the protocol). **B** are small hygiene items, do them between A
steps. **C** are optional. Effort is in working sessions of this assistant (one session ≈ one long conversation).

### A. Aggregation into the protocol

**A1. ~~Measure where layer-1 time goes~~ — done 2026-11-21** (`docs/zk/aggregation.md` "Where the time goes": build ≈ ⅓ of each step; per-claim circuits cascade to every 2-to-1 owner and to the checker, so A2 is required before A4). Split `layer1` into owner construction (circuit build,
`TrustedPreparedLayer::new`) and `prove`, for 64×64 and 256×256 at J101, and the same for `aggregate`. Also measure
an EF5 leaf on the CPU vs the EF4 GPU leaf, and the layer-1 circuit size (rows per table). Output: a table in
`docs/zk/aggregation.md`. Decides how urgent A2 is: if the build is a small share, per-claim circuits are tolerable
for a first protocol version.

**A2. Make the life leaf circuit claim-independent (1–2 sessions).** Today the periodic columns carry claim data
(INV-LIFE-FS-2), so the recursive verifier circuit changes with every slot. Target: periodic columns that depend
only on the shape, claim bound through public values.
- Mask: replace `mask[o][class]` (built from the `absent` flags) by shape-only band indicators per neighbour
  direction times 8 public `absent` values (degree unchanged: the mask enters `f` additively). Verifier computes
  the public `absent` from `decide` as today.
- Generation 0 and `X_out`: bind the committed cells to public digests instead of periodic limbs. Options, in order of
  preference; the step starts with a 1-hour design check of each against Plonky3 rc.2 and P3R:
  1. **Poseidon2 sponge AIR in the same batch** (separate AIR, LogUp bus carrying the packed limbs from `LifeAir` to
     the sponge; public values = Poseidon2 digests of `E` and `X_out`). This is the design of guide §3.4–3.5
     (`docs/zk/notes/plonky3-implementation-guide.md`), already compile-sketched for water. Leaves become batch
     STARKs with lookups: layer 1 then needs P3R's low-level `verify_batch_circuit` path (guide §4.2), not the
     trusted uni-STARK owner.
  2. Poseidon2 permutation columns inside `LifeAir` (no bus; about 2 permutations per trace row at n = 256, i.e.
     several hundred extra columns): simpler recursion, wider trace. Measure before choosing.
  - The verifier computes both digests natively from the boards (cheap); the keccak claim digest goes away.
- Keep the EF4 P100 path working unchanged (it can keep the periodic layout) unless the new layout is not slower.
- Acceptance: one prepared layer-1 owner per zone shape proves many slots (`prove` with different claims, one
  `new`); negative tests: a claim with another board / output / absent mask is rejected (release mode); security
  re-asserted (`p100_is_secure_on_the_whole_envelope` and the umis-agg shape test with the new widths); spec
  updated (`docs/zk/life2d-air-spec.md`, new INV-LIFE-* ids for the digests and the mask).
- Depends on: A1 (to size the benefit). Touches `crates/task-life2d/src/air.rs`, `air/tests.rs`, `shape.rs`,
  GPU prover (`air/gpu.rs`, `umis-gpu` JIT handles new constraints automatically; trace generation kernels may need
  the sponge columns), `umis-agg`.

**A3. Statement schema and per-commitment aggregation (1 session).** Define what an aggregate proves, so the beacon
can check it without states:
- Leaf statement: `[zone, from_slot, params_hash, d_prev, d_next, d_inputs..., absent]` as field elements, where the
  digests are the A2 Poseidon2 digests (or the beacon's state-root encoding, see P2 below).
- Per zone commitment: chain the slots' layer-1 proofs with `TrustedPreparedAggregation::new_state_transition`
  (statement `[init, final, count]`, upstream checks `left.final == right.init`), so one proof covers a commitment.
- Per beacon slot: plain 2-to-1 across zones; export the list of zone statements (or a Poseidon2 root of them).
- Acceptance: an aggregate over a 4-slot commitment verifies against the commitment's first and last state digests;
  a gap or reordering is rejected.

**A4. Protocol integration (2–3 sessions).** Behind a genesis flag (`aggregation: true`), keep the per-leaf path as
the fallback:
- Record `AggregateProof { beacon_slot, zones_root, statement_root, proof }`; an aggregation deadline `L_agg` after
  `L`; aggregator role = native full nodes (ranked like provers).
- `BeaconState::apply_aggregate`: verify one aggregate per beacon slot against the zone commitments it covers (their
  state digests), then mark them Proven; leaves no longer need `LeafData` states on the beacon chain.
- Cross-zone binding: every consumed halo digest must equal its producer's face digest at that slot. First version:
  checked natively by the beacon from the exported statements; later inside the top circuit.
- Shares/proof commitments become leaf-only traffic between provers and aggregators.
- Acceptance: SimNet scenario `life-agg` (honest) and `life-agg-junk` (junk aggregate, missing aggregate → fallback or
  rollback as designed); a devnet run with the bootstrap aggregating.
- Depends on: A2, A3, and decision P2 (state roots).

**A5. Leaves on the GPU in EF5 (1–2 sessions, optional until A4 runs).** The GPU kernels use `Ext4` arithmetic
(quotient, out-of-domain values, FRI inputs). Port to `Ext5` (`QuinticTrinomialExtensionField`, X⁵ + X² − 1) behind a
type parameter; keep Keccak trees out (aggregation needs Poseidon2 Merkle trees on the device: a new Poseidon2 leaf
and layer kernel, `umis-gpu/src/pow.rs` already has the permutation). Acceptance: GPU EF5 leaves verify with the CPU
verifier and recurse. Until then aggregation leaves prove on the CPU (16.8–23 ms per slot measured).

**A6. Water aggregation (later).** Water leaves are batch STARKs with LogUp: P3R's low-level batch verifier path
(guide §4.2) and an EF5 water config. Only after life works end to end.

### B. Hygiene (small, any time)

- **B1** Report the P3R bug upstream: `RecursiveAir::declares_interactions` omits `num_periodic_columns`
  (`recursion/src/traits/air.rs:236` at `3a69ba6`); our workaround is in `LifeAir::eval`. Draft the issue text for
  the owner (do not post without asking).
- ~~**B2**~~ Done 2026-11-21: `stark`, `life-stark-keccak`, `life-stark-junk`, `life-stark-l0` run in
  `crates/umis-simnet/tests/scenarios.rs` (about 11 s together in debug).
- ~~**B3**~~ Done 2026-11-21: shares go on `zone/<z>/proof` only; a share on `zone/<z>/out` is ignored.
- ~~**B4**~~ Done 2026-11-21: `Ctx::verify_proof` memoizes failures keyed by statement, proof address **and** a keyed
  fingerprint of the public data (a copied honest proof with wrong states must not condemn the right states); test
  `ctx::tests::failed_verifications_are_memoized_per_public_data`. Grouped batch verification is not memoized.
- **B5** Browser prover: stop posting the secret key to workers (`crates/umis-web/src/prover.rs:24`); pass only what the
  job needs (async backends do not sign).
- **B6** Measure P100 in wasm for life2d (browser leaf cost) and EF5 leaves in wasm; record in `docs/life2d.md`.
- **B7** Refresh the demo (`scripts/demo/`) with P100 and republish the player artifact (the published one predates
  P100).
- **B8** Move share verification off the event loop (`ZoneEngine::on_share` verifies synchronously).

### C. Optional / research

- **C1** GPU FRI (plan in `docs/gpu.md`): only if a faster PCIe link or much larger batches change the measured ≤ 3 %.
- **C2** Batching across zones (one batch STARK for several zones' commitments).
- **C3** Water on the GPU (needs a device batch prover with LogUp).
- **C4** WHIR for inner aggregation layers (P3R supports it; may shrink layers further).

## 4. Pending owner decisions (ask when the step needs them)

- **P1** Browser role: leaf prover only, or verifier of every proof? (decides B6 depth, wasm SIMD/threads work).
- **P2** Canonical state roots: keep keccak BMT (Swarm-compatible) and add a Poseidon2 digest next to it (dual roots),
  or make Poseidon2 canonical. Needed by A3/A4. Recommendation: dual roots (keccak for storage/DA, Poseidon2 for
  proofs), bound by the minter's signature and fault evidence.
- **P3** Nonlinear physics: raises the constraint degree (P100's blowup 4 allows degree ≤ 5).
- **P4** Version policy: stay on the pinned rc.2 + P3R `3a69ba6` until a reason to move (security fix, needed feature).

## 5. Schedule

| order | step | sessions | needs |
|---|---|---|---|
| 1 | ~~B2, B3, B4~~ (done 2026-11-21) | 0.5 | — |
| 2 | ~~A1 measure~~ (done 2026-11-21) | 0.5 | — |
| 3 | A2 claim-independent leaf circuit | 1–2 | A1 |
| 4 | B1 draft upstream issue, B5 | 0.25 | — |
| 5 | A3 statement schema, per-commitment chaining | 1 | A2, P2 |
| 6 | A4 protocol integration + SimNet | 2–3 | A3 |
| 7 | B6, B7 browser numbers, demo refresh | 0.5 | — |
| 8 | A5 EF5 GPU leaves | 1–2 | A4 running |
| 9 | A6 water aggregation | 2 | A4 |

Every step ends with `scripts/check.sh` green, SimNet sweep (`umis-simnet --scenario all --seeds 5 --slots 60`) green
where protocol code changed, docs and this file updated, and a commit (with the attribution lines).
