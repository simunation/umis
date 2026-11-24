# Recursive aggregation (`umis-agg`)

`crates/umis-agg` (native only) aggregates life2d leaf proofs with Plonky3-recursion (P3R), pinned to commit
`3a69ba61f713c8400b14eb3d8f0e52caa13512c7` (unaudited upstream; approved by the owner on 2026-11-20). Plonky3 is on
`=0.9.0-rc.2`, the version P3R pins. Status: working prototype, not wired into the protocol.

## Design

- **Field: KoalaBear⁵** (`QuinticTrinomialExtensionField`, about 2^155) for every aggregation proof. P3R's
  in-circuit verifier cannot check proof-of-work at the out-of-domain, batch or lookup steps (INV-PARAM-5). Over
  KoalaBear⁴ the batch-combination term (`F − log2|LDE| − log2 B`) then capped the recursive proofs at 93–96 bits
  (first spike, 2026-11-20). EF5 adds about 31 bits to every field-size term. P3R's cross-config APIs need the same
  challenge field on both sides, so leaves are EF5 proofs too.
- **Config:** upstream's built-in `KoalaBearD5Poseidon2BinaryConfig` (Poseidon2 Merkle trees, binary, 8-element
  digests) at `AggParams`; backend `FriRecursionBackend::new_d5(KOALA_BEAR_D1_W16)`.
- **Owners:** `TrustedPreparedLayer` (layer 1: verify one leaf in-circuit) and `TrustedPreparedAggregation` (2-to-1).
  Their `CircuitVerifier`s check each recursive proof against its expected statement: the leaf's public values (the
  claim digest), and for a 2-to-1 proof the left statement followed by the right one.
- **Parameters** (`AggParams`, all without OOD/batch/lookup grinding, 16-bit query and per-round commit grinding,
  fold arity 8):

  | preset | regime | blowup | queries | leaf / layer 1 / 2-to-1 proven bits |
  |---|---|---|---|---|
  | `J101` (default) | Johnson (DKT26 line count, `fri-soundness-foundations.md` §3.6) | 8 | 60 (58 until 2026-11-22) | 102.9 / 102.4 / 102.0 (58 queries) |
  | `U101` | unique decoding only | 4 | 128 | 102.7 / 102.8 / 102.8 |

  Over EF4 the Johnson regime gave nothing (the batch term bound in both); over EF5 it needs about half the queries,
  and recursive verifier circuits scale with the query count. The owner's rule (2026-11-20): take the newer bound
  where it is faster.

## Measured (16 cores, load about 6)

| zone | params | leaf | layer 1 | 2-to-1 | verify |
|---|---|---|---|---|---|
| 64×64 | J101 | 16.8 ms, 66 KB | 167 ms, 347 KB | 1.62 s, 404 KB | 9–11 ms |
| 256×256 | J101 | 23.2 ms, 136 KB | 324 ms, 347 KB | 1.79 s, 401 KB | 10–12 ms |
| 64×64 | U101 | 9.1 ms, 122 KB | 270 ms, 669 KB | 2.52 s, 835 KB | 17 ms |
| 256×256 | U101 | 20.9 ms, 260 KB | 447 ms, 705 KB | 3.07 s, 831 KB | 18–22 ms |

The earlier EF4 spike (insecure at about 93 bits) took 1.3 s and 620 KB per 2-to-1 step. Recursion cost does not
grow with the zone. Leaves are proved on the CPU: the GPU prover is EF4-only.

## Where the time goes (A1, 2026-11-21, J101, load 13–15)

Each recursive step is two parts: building the prepared owner (the verifier circuit for its children, P3R
`TrustedPrepared*::new`) and proving with it (`prove(&self, …)`, reusable).

| zone | step | build | prove | table log heights (6 tables) |
|---|---|---|---|---|
| 64×64 | layer 1 | 93–100 ms | 181–182 ms | 11, 6, 13, 12, 10, 0 |
| 256×256 | layer 1 | 165–189 ms | 338–367 ms | 14, 7, 14, 13, 11, 0 |
| any | 2-to-1 | 733–959 ms | 1.43–1.60 s | 9, 8, 16, 16, 13, 0 |

- The build takes about a third of every step.
- **The per-claim circuits cascade.** An aggregation owner fixes its children's verifiers. Every layer-1 circuit
  differs per claim, because of the periodic columns (INV-LIFE-FS-2). So every 2-to-1 owner must be rebuilt too,
  and nothing can be built once and reused.
- **The checker cannot use a fixed verifier.** It would have to rebuild the whole tree of circuits from the claims
  to check one aggregate (about 1 s per 2-to-1 node). A2 (claim-independent leaf) is therefore **required** before
  protocol integration (A4), not just an optimization. With reusable owners a 2-to-1 step drops to its prove time
  (about 1.6× faster).
- Leaves: under the same load, an EF5 J101 leaf on the CPU (including its native verify) took 20/36 ms
  (64×64 / 256×256), against 5.3/18.6 ms for an EF4 P100 proof (`bench_life_air`). The GPU EF4 single-proof
  latency is 3.7 ms at 256×256 (`docs/gpu.md`). Leaves are not the bottleneck: one 2-to-1 step costs as much as
  about 60 leaves.

## Claim-independent leaves (A2, 2026-11-21)

`task_life2d::air::digest::LifeDigestAir` (spec §4c, INV-LIFE-DIG-*) replaces the per-claim periodic data with a
Poseidon2 sponge in the trace. Its statement is the digest plus the 8 `absent` flags. `umis_agg::digest_leaf`
proves it; `Layer1Owner::new(p, representative)` and `AggOwner::new(p, left, right)` are built **once per zone
shape** and then `prove` any slot (`DIGEST=1` in `agg_bench`). J101, load 10–15, two runs:

| zone | leaf (width, size) | layer 1: build once, then prove | 2-to-1: build once, then prove | bits leaf / L1 / 2-to-1 |
|---|---|---|---|---|
| 64×64 | 28 ms (256 cols, 116 KB) | 123–126 ms, then 232–279 ms | 586–604 ms, then 1.15–1.25 s | 102.7 / 102.4 / 102.0 |
| 256×256 | 61–84 ms (450 cols, 186 KB) | 167–177 ms, then 302–332 ms | 659–707 ms, then 1.18–1.32 s | 102.8 / 102.4 / 102.0 |

The per-claim path measured back to back: leaf 11–28 ms, layer 1 0.20–0.44 s and 2-to-1 1.8–2.0 s, both
including their builds. So per leaf in a binary tree (one layer-1 and about one 2-to-1 proof), the cost drops
from about 2.3 s to about 1.6 s, and the checker has one fixed verifier per shape. The leaf itself costs about
2.5× more, which is small against the recursion.

## Chained statements (A3, 2026-11-21)

The digest leaf's statement is `[d_prev, d_halo, d_next, absent]` (spec INV-LIFE-DIG-1, -5). P3R's
`new_state_transition` merger requires the statement to be exactly `[init, final, count]`, so aggregation stays
plain 2-to-1 (statements concatenate). `umis_agg::chain` checks continuity natively and returns the first and last
board roots. With one strip per zone (decision), the halo digest is recomputable from neighbour payloads. Cost by
layout (J101, load about 10, owners built once):

| zone, layout (k = 1) | leaf width | leaf | layer-1 prove | 2-to-1 prove | per leaf in a tree |
|---|---|---|---|---|---|
| 256×256, 8 strips, rpr 3 | 762 | 107 ms, 268 KB | 0.58 s | 1.2–1.3 s | ~1.8 s |
| 256×256, 1 strip, rpr 2 | 2896 | 190 ms, 816 KB | 1.39–1.53 s | 0.93 s | ~2.4 s |
| 256×256, 1 strip, rpr 3 | 4110 | 243 ms, 1.1 MB | 1.76–1.81 s | 0.95–1.0 s | ~2.8 s |
| 256×256, 1 strip, rpr 5 | 6538 | 372 ms, 1.8 MB | 3.5–3.6 s | 1.24–1.30 s | ~4.8 s |
| 64×64, 1 strip, rpr 2 | 856 | 65 ms, 262 KB | 0.52–0.57 s | 1.24–1.28 s | ~1.8 s |

Layer-1 cost grows with the leaf's width (every query opens a whole row in-circuit), so prefer small `rpr`.

## In the protocol (A4 step 1, 2026-11-21)

`umis_agg::backend::LifeAggBackend` (genesis `stark_config_id = koalabear-agg[-test]`, native only, insecure opt-in)
proves a commitment's advancing life slots as one aggregate in the batch head format (spec §4d,
INV-LIFE-AGG-1..3). Each verifier builds its own owner tree per (shape, member count) and caches it. SimNet
`life-agg` and `life-agg-junk` pass (0 rollbacks, exact life state; about 100 s each, dominated by proving). The
beacon still receives the states. Next (step 2): Poseidon2 roots in zone commitments, the halo check from input
payloads, and leaving the states off the beacon chain.

## Native batch leaves (S5b, 2026-11-22)

Stencil leaves (`umis-stencil` `StencilBatch`) are native `p3_batch_stark` proofs of three AIRs (cells, Poseidon2
hash table, range table) joined by LogUp buses. P3R's trusted owners accept only uni-STARK leaves or P3R's own
circuit proofs, so `umis_agg::native` builds the same trusted construction from P3R's public pieces:

- `NativeRelation`: the AIRs (`StencilBatch::airs()`), heights and the EF5 setup (`ProverData`) of one relation;
  `prove` gives a `NativeLeaf`.
- `NativeLayer1::new(params, relation, representative)`: `prepare_circuit_for_verification`,
  `BatchStarkVerifierInputsBuilder::allocate`, `verify_batch_circuit` (AIRs and FRI parameters are circuit
  structure), then:
  - **INV-AGG-NB-1 (VK pin):** the `CommonData` preprocessed-commitment targets are constrained to the relation's
    commitment (`ConstrainConstantCommitment`). Needs the local P3R patch below.
  - **INV-AGG-NB-2 (statement):** `set_statement_exports` exports exactly the leaf's AIR public values (32 for a
    stencil leaf: `d_prev, d_halo, d_next, d_faces`; 24 before S6b). Every other circuit input, including the leaf's commitments and
    openings allocated with `public_input()`, is witness: `CircuitVerifier::verify(proof, statement)` checks only
    the statement table. The in-circuit transcript binds the witness commitments.
  - **INV-AGG-NB-3:** one owner per relation and shape proves every leaf of that shape (`prove` checks the shape and
    verifies the leaf natively first; `prove_unchecked` skips that for tests).
  - Prover: the backend's NPO tables plus the statement table, `BatchStarkProver::prepare_circuit`.
- **INV-AGG-NB-4:** J101 (now 60 queries) gives ≥ 101 proven bits for EF5 stencil leaves (heat 64², LBM 32²,
  life 64²) and the layer-1 proofs over them.
- Upper layers are unchanged: `AggOwner` (2-to-1) takes `NativeLayer1`'s recursive proofs; `stencil_chain` checks
  that consecutive leaf statements chain.

**Local P3R patch** (owner-approved 2026-11-22): `scripts/vendor-p3r.sh` builds `vendor/plonky3-recursion`
(gitignored) from the pinned rev plus `vendor/p3r-*.patch`; the workspace `[patch]` points P3R's six crates there.
- `p3r-common-data-preprocessed.patch`: a public accessor `CommonDataTargets::preprocessed_commitment` (the
  field is `pub(crate)`), needed for the VK pin.
- `p3r-periodic-layout.patch`: `replay_batch_stark_transcript` and `declares_interactions` built their
  `AirLayout` without the AIR's periodic columns (an upstream bug: the quotient-degree computation panics for any
  batch AIR with periodic columns). The in-circuit folding already declared them.

Measured (J101, `stencil_agg_bench`, 16 cores, load 5–9; leaf includes its native verify):

| rule, zone | EF5 leaf | layer-1 owner build (once) | layer 1 | 2-to-1 | aggregate verify |
|---|---|---|---|---|---|
| heat 64² | 161 ms, 0.64 MB | 367 ms | 852 ms, 0.41 MB | 1.12 s, 0.44 MB | 10 ms |
| LBM 32² | 610 ms, 3.35 MB | 2.86 s | 5.31 s, 0.45 MB | 1.02 s, 0.45 MB | 10 ms |
| life 64² | 15 ms, 0.23 MB | 171 ms | 359 ms, 0.39 MB | 1.03 s, 0.44 MB | 9 ms |

Tests (`tests/native.rs`, release): honest recursion and statement binding; one owner for many slots plus a 2-to-1
aggregate that chains; a leaf of another rule (same widths, same preprocessed commitment) rejected by the circuit;
a leaf proved under a forged verifying key rejected by the circuit (mutation-checked: without the pin it recurses);
101 bits.

## Tests and tools

- `tests/agg.rs` (test parameters): leaf → layer 1 → 2-to-1, each verified against its statement; a recursive
  proof against another statement and an aggregate with swapped statements are rejected; a leaf presented with
  another claim's public values does not recurse; `J101` and `U101` clear 101 bits for the CLI's leaf shapes.
- `umis_agg::recursive_shape` gives the security calculator a recursive proof's shape (tallest table, opened
  codewords, degree; constraint and LogUp counts are upper estimates).
- Bench: `PARAMS=j101|u101 cargo run --release -p umis-agg --example agg_bench -- 64,1,4,2 256,1,8,3`.

## Open

- ~~Per-claim periodic columns~~ (INV-LIFE-FS-2): solved for recursion by the digest leaf (above). The
  per-claim `leaf`/`layer1`/`aggregate` functions remain for comparison.
- **Remaining for A4:** the checker's halo check (rebuild `d_halo` from input payloads, check them against the
  neighbours' outputs), hold and birth slots inside a chain (native slots without a leaf), cross-zone 2-to-1
  (owners per pair of shapes), and the Poseidon2 state roots in zone commitments.
- **Protocol integration:** an aggregator role, an aggregation deadline after `L`, an aggregate record checked by the
  beacon chain, and leaves bound to the beacon's statements (today the leaf statement is the claim digest).
- **GPU leaves:** the GPU prover's kernels are EF4; aggregation leaves need an EF5 port to use it.
- **P3R bug worked around** in `LifeAir::eval`: `RecursiveAir::declares_interactions` builds its layout without
  `num_periodic_columns`.
