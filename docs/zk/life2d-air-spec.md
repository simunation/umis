# life2d transition proof: specification and invariants

The STARK for one advancing slot of a `life2d@1` zone (`crates/task-life2d/src/air.rs`). The layout and the
constraint derivation are in the module doc comment; this file lists what a proof claims and the invariants
(`INV-LIFE-*`) the code and tests rely on. Cite the IDs in commits and tests. The general rules of
[`air-spec.md`](air-spec.md) (Fiat–Shamir binding, parameters, determinism) apply as well; where they apply
unchanged they are cited by their water IDs.

## 1. What a proof claims

The statement `TransitionStatement { zone, from_slot, to_slot = from_slot + 1, params_hash, prev_state_root, state_root,
inputs_commitment, outputs_root }` together with the public data (params, genesis seed, both states, the input
payloads and their InputRefs) claims: `next = step(prev, inputs, from_slot)` for the task's `decide`/`apply`.

The verifier (`Life2dStark::verify_inner`) splits the claim:

| Part | Checked by |
|---|---|
| state and input roots, `outputs_root` from `next`, InputRefs ↔ payloads | native (INV-LIFE-IO-1..3) |
| canonical decoding of `prev` and `next` | native (INV-LIFE-CANON-1) |
| the decision (hold, birth or advance) and, for an advance, the extended board `E` and the `absent` flags | native: `decide` re-run on the public inputs |
| hold or birth: the whole next state | native, proof must be the single byte `0` (INV-LIFE-TAG-1) |
| advance: every header field and the `prev` board of `next` | native (`header_matches`) |
| advance: `next.grid = k generations of B3/S23 on E`, absent regions dead | **the STARK** |

So the STARK proves one thing only: the inner `n × n` board after `k` masked generations of `E`.

## 2. Arithmetic

**INV-LIFE-ARITH-1.** `f = (c − 3)(c − 3 + a) + m` with `c ∈ [0, 8]`, `a, m ∈ {0, 1}` is a non-negative integer
`≤ 31` (`(8−3)(8−3+1) + 1`), far below `p`. `f = 0 ⇔ (c = 3) ∨ (a = 1 ∧ c = 2)` when `m = 0` (Conway's B3/S23),
and `f ≥ 1` when `m = 1` (because `(c−3)(c−3+a) ≥ 0` over the integers for `a ∈ {0,1}`). The field identity is
the integer identity.

**INV-LIFE-ARITH-2.** Every generation-0 cell is boolean (`b·(b − 1) = 0`, INV-LIFE-IN-1). Each later cell `a'` is
then forced boolean and unique by `a'·f = 0` and `f·v = 1 − a'`: `f ≠ 0 ⇒ a' = 0`; `f = 0 ⇒ a' = 1`. Every main
column of the cone of every generation carries these two constraints (`tampered_traces_are_rejected` flips cells,
sets a cell to 2 and alters `v`).

**INV-LIFE-ARITH-3.** `m` is the periodic column `mask[o][class(x)]`, `o = t + i` the stacked-row offset of the cell
and `class(x)` its column class (left halo `x < k`, inner, right halo `x ≥ w + k`). The verifier computes it from
the `absent` flags it derived with `decide` (never taken from the proof): `mask = absent(row band, column band)`,
where the column band of class `c` on strip `s` is 0 only for `c = left, s = 0` and 2 only for `c = right,
s = S − 1`. This is exact because `k ≤ w = n/S` (`validate_params`; INV-LIFE-CONE-2). Mask values are 0/1
(`lying_about_absent_regions_is_rejected`).

**INV-LIFE-IN-1 (generation 0 is the claim).** Generation 0 (`rpr × Ws` cells per trace row) is committed in the
main trace. Each cell is boolean, and each run of up to 30 cells of a slot equals one periodic limb:
`Σ_j 2^j·b_{30q+j} = x0[i][q]`, which the verifier packs from the extended board `E`. A sum of ≤ 30 booleans is
below `2^30 < p` (KoalaBear), so the limb fixes every bit. The cells of slots `i ≥ rpr` are read from the next
trace row, which on the torus is the next stacked rows (row 0 after the last). Both constraints are needed:
`generation_zero_must_equal_the_claim` proves an honest Conway run from a different `E` with the same output
(caught by the limb) and non-boolean "bits" `2, −1` with an unchanged limb in a padding row, outside every inner
light cone, with all later generations recomputed (caught only by booleanity; checked by disabling it).

## 3. Layout invariants

**INV-LIFE-CONE-1 (light cone).** Generation `t` is constrained exactly on strip columns `[t, Ws − t)`; generation `k`
on the `w` inner columns. Each inner row of every strip is at distance `≥ k` from the strip's top and bottom
(halo rows) and from the torus seam/padding, so values outside the zone's true light cone (neighbouring strips,
padding, the wrap) cannot reach a constrained generation-`k` inner cell. `trace_matches_native_rule` checks
the honest witness equals `run_extended_masked` on random boards over 6 layouts, including `k = n/2`.

**INV-LIFE-CONE-2.** `rows_per_trace_row ≥ 2`, so a generation-`t+1` cell reads at most slots `i..i+2` of
generation `t`, which are in the current or the next trace row. `validate_params` enforces `2 ≤ rpr ≤ 16`,
`strips | n`, `n % 8 = 0`, `8 ≤ n ≤ 256`, `1 ≤ k ≤ n/2` and `k ≤ n/strips` (INV-LIFE-ARITH-3).

**INV-LIFE-CONE-3.** No transition gating: all constraints hold on every row including the wrap from the last row
to the first. Padding rows hold generation-0 zeros (their limbs are 0), which evolve to zeros in the honest trace,
and are outside every inner cone.

**INV-LIFE-OUT-1.** `sel[i]` is 1 exactly when slot `i` of generation `k` is an inner row of some strip; then each
packed run of up to 30 inner cells equals the claimed limb: `sel·(Σ_j 2^j·a_k[k + 30q + j] − out[i][q]) = 0`.
Generation-`k` cells are boolean (INV-LIFE-ARITH-2), so the limb fixes them. Every cell of `X_out` is covered
exactly once (each inner row of each strip appears once on the torus). `false_claims_are_rejected` includes a
prover that commits to the false `X_out` in its public values and periodic data, so only this constraint stands.

## 4. Fiat–Shamir

**INV-LIFE-FS-1 (= INV-FS-1).** Periodic columns (packed generation 0, packed `X_out`, `sel`, the mask) are not
absorbed by uni-stark. The 16 public digest limbs bind them: `keccak("life2d-air" ‖ params_hash ‖ slot ‖ pack(E)
‖ pack(X_out) ‖ absent)`. The verifier recomputes digest and periodic data from the claim. A proof for one slot or
zone does not verify for another (`false_claims_are_rejected`, replay cases).

**INV-LIFE-FS-3 (Keccak configuration).** `stark_config_id = koalabear-keccak` (the CLI default) uses the same
field, FRI parameters and AIR, with Keccak-f[1600] Merkle trees (`umis_stark::config::koala_bear_keccak`: 4-word
digests, `SerializingHasher` leaves, `CompressionFunctionFromHasher` nodes) and the Poseidon2 duplex transcript.
Merkle roots enter the transcript word by word as limbs of 22, 22 and 20 bits (`HybridChallenger`); every limb is
below `p`, so the encoding of a root is injective and two different roots never absorb the same field elements.
Grinding stays Poseidon2 (a Keccak transcript made proof-of-work grinding about 2 ms slower per proof). Water3d
keeps Poseidon2 under this id. The whole negative-test suite runs on both configurations (`LIFE_HASH=keccak`).

**INV-LIFE-FS-2.** Periodic data is per claim, so these proofs are not recursion-friendly as is (INV-PARAM-6).
For recursion, use the digest AIR (§4c, INV-LIFE-DIG-*), whose periodic data depend on the shape only.

## 4b. Batched proofs

The advancing slots of one proof commitment are proved together by its committer
(`ProofBackend::prove_batch`, one Plonky3 batch-STARK over all their claims) and verified together
(`ProofBackend::verify_group`, called by the beacon chain for every proof commitment).

**INV-LIFE-BATCH-1 (head).** The first advancing slot carries `ProofTag::BatchHead (2) ‖ m (u16 BE) ‖ m member
statement addresses ‖ postcard batch proof`, `m ≥ 2`; the head is its own first member.

**INV-LIFE-BATCH-2 (references).** Every other member carries exactly `ProofTag::BatchRef (3) ‖ keccak(head
bytes) ‖ index (u16 BE)` (35 bytes). A reference verifies only if a head in the same group verifies and lists the
reference's statement address at that index; a head verifies only if every listed member is present with its
exact reference bytes. Neither verifies alone (`verify` rejects tags 2 and 3).

**INV-LIFE-BATCH-3 (members).** Every member goes through the native checks of a single proof (roots, canonical
states, outputs, inputs, decision, header; INV-LIFE-IO, -CANON) and must be an advancing slot; the batch STARK is
verified against the claims the verifier derived (periodic data and digests per member, INV-LIFE-FS-1), so each
member is bound to its own statement and public data. Hold and birth slots keep tag 0 outside the batch.

**INV-LIFE-BATCH-4 (size).** A batch STARK has at most `MAX_BATCH = 32` members: the verifier rejects a head
listing more (`parse_batch_head`), and the prover splits a larger commitment into several batches. The batch
combines every member's codewords with one challenge, so its proven security falls with the member count; §6
asserts the bound at `MAX_BATCH`.

`batched_slots_verify_only_as_a_group` covers: the honest group; each proof alone; a missing member; swapped
references; a bit flip in the batch STARK; a member whose claimed next state is false (roots re-derived); replay
into another zone; an unbatched proof in the same group.

## 4c. The digest AIR (claim-independent, for recursion)

`air::digest::LifeDigestAir` (ROADMAP A2) proves the same `k` masked generations as `LifeAir` with the same main
columns and the same generation constraints (shared code: `Cells`, `eval_gen0_boolean`, `eval_generations`). Its
periodic columns depend only on the shape, so one AIR, one verifier circuit and one recursive owner serve every
slot of a zone shape (it resolves INV-LIFE-FS-2 for recursion). It is used by `umis-agg` only; the protocol's
EF4 P100 backend keeps `LifeAir`.

**INV-LIFE-DIG-1 (claim digests).** The public values are `[d_prev, d_halo, d_next, absent]` (8 + 8 + 8 + 8).
Each digest is a Poseidon2 (KoalaBear, width 16, rate 8, overwrite mode) sponge over every trace row's limbs, row
0 first, with 30 boolean cells per limb, zero-padded to whole permutations:
- `d_next`: the inner generation-`k` cells of each slot `i`, times `sel[i]`.
- `d_prev`: the generation-0 cells of the **same stacked rows**: slot `i + k`, read from the next trace row when
  `i + k ≥ rpr`, which needs `k ≤ rpr`, inner columns, times `sel[i]`. So `d_prev` and `d_next` absorb a board at
  identical positions, and both equal `digest::board_root(board)`.
- `d_halo`: every other generation-0 cell, at its own position (inner columns times `1 − sel0[i]`, side columns
  always): the halo, the strip copies and the padding.

Every generation-0 cell is in exactly one of `d_prev`, `d_halo`. Cells are boolean (INV-LIFE-ARITH-2), so the limbs
fix every cell, and the 8-element capacity gives about 124-bit collision resistance. The verifier computes the
digests natively (`digest::public_values`). Tests: `a_false_output_is_rejected`,
`a_false_previous_board_is_rejected`, `generation_zero_must_match_the_digest`,
`tampered_sponge_columns_are_rejected`.

**INV-LIFE-DIG-2 (sponge in the trace).** Each row carries `cap_in[8]` and, per permutation, its 128 full-round
and 20 partial-round S-box outputs (degree-3 constraints, the rounds of `Poseidon2KoalaBear<16>`;
`permutation_columns_match_the_native_permutation`). Permutation `m`'s input is `[limbs[8m..8m+8], capacity]`,
the capacity being `cap_in` for `m = 0` and permutation `m − 1`'s output otherwise. `cap_in = 0` on the first row;
on every transition, the next row's `cap_in` equals this row's last output `[8..16]`. The last row's last output
`[0..8]` equals `public[0..8]`. Without the transition constraint, a prover could restart the chain after a
tampered row (`generation_zero_must_match_the_digest`, checked by disabling it).

**INV-LIFE-DIG-3 (mask from public flags).** `public[8..16]` are the `absent` flags, in port order. For each
stacked-row offset `o`, eight shape-only indicators `L_b = [strip 0 ∧ row band b]` (b = 0, 1, 2),
`C_b = [row band b]` (b = 0, 2), `R_b = [last strip ∧ row band b]` give the mask
`m(o, inner) = C₀·a_N + C₂·a_S`, `m(o, left) = L₀·a_NW + L₁·a_W + L₂·a_SW + (C₀ − L₀)·a_N + (C₂ − L₂)·a_S`, and
`m(o, right)` likewise with `R`. Exactly one term is active per cell, so `m ∈ {0, 1}` for boolean flags, and it
equals INV-LIFE-ARITH-3's mask (`the_band_mask_equals_the_periodic_mask`, all 256 flag sets on 4 layouts). The
constraint degree stays 3: periodic and public values are constants in the trace. The verifier derives the flags
with `decide`, as for `LifeAir`. Test: `lying_about_absent_regions_is_rejected` (digest version).

**INV-LIFE-DIG-4 (one circuit per shape).** `LifeDigestAir::new(shape)` takes nothing from the claim
(`one_air_serves_every_claim_of_a_shape`). In `umis-agg`, `Layer1Owner` and `AggOwner` are built once and prove
many slots (`one_owner_per_shape_proves_many_slots`). A leaf presented with another claim's statement, or with
one flipped absent flag, does not recurse (`a_digest_leaf_with_a_wrong_claim_does_not_recurse`).

Cost (256×256, k = 1): width 294 → 450 (one permutation per row), and the leaf proves about 2.5× slower over
EF5 (`docs/zk/aggregation.md`). Security: `digest_leaf_shapes_are_secure` (J101 and U101 ≥ 101 bits).

**INV-LIFE-DIG-5 (chaining, A3).** `d_next(t) = d_prev(t + 1)` exactly when slot `t + 1` starts from slot `t`'s
output (`consecutive_slots_chain`). `umis_agg::chain` checks this over an aggregate's exported statement (the leaf
statements, left to right) and returns the first `d_prev` and the last `d_next`. The checker compares them with the
zone commitment's Poseidon2 state roots (P2 dual roots; `board_root` depends on the zone's layout params). Test:
`consecutive_slots_chain_through_an_aggregate` (chain verifies; gap and reordering rejected).

**Aggregation leaves use one strip** (`strips = 1`, owner decision 2026-11-21). Then `d_halo` depends only on the
neighbour regions and padding (`with_one_strip_the_halo_digest_ignores_the_zone_board`), so a checker without the
zone's state recomputes it from the input payloads. With several strips, the strip copies of the zone's own cells
enter `d_halo` and only a checker holding the state can recompute it. Best layout at 256×256, k = 1: `rpr = 2`
(width 2896, height 256).

## 5. I/O and tags (native, `verify_inner`)

- **INV-LIFE-IO-1.** `to_slot = from_slot + 1`, `params_hash` matches the zone; `prev`/`next` match the roots.
- **INV-LIFE-IO-2.** `outputs_root` is recomputed from the decoded `next`.
- **INV-LIFE-IO-3.** `list_root(refs) = inputs_commitment`, `inputs.len() = refs.len()`, every payload's address
  equals its ref (link id = consumer port). Payloads are decoded canonically by `decide`; a malformed payload is
  treated as missing, which can only make the zone hold.
- **INV-LIFE-CANON-1.** `decode_state` accepts exactly one encoding per state: fixed length, version byte, zero header
  padding, zero bit padding, `born ∈ {0,1}`, born states with `birth ≤ gen`, `k | gen`, `k | gen − birth`, and
  unborn states all zero but their slots (`non_canonical_states_and_wrong_params_are_rejected`).
- **INV-LIFE-TAG-1.** First proof byte: `0` = native (hold/birth; the proof is exactly `[0]`), `1` = STARK
  (advance; a postcard uni-STARK proof follows), `2` / `3` = batch head / reference (§4b). An advance with tag 0, or a hold/birth with tag 1, is rejected
  (`hold_slots_are_native_only`, `false_claims_are_rejected`).

## 6. Parameters

**Preset P100** (`FriPreset::P100`, the `koalabear` and `koalabear-keccak` ids since 2026-11-19): `log_blowup = 2`
(constraint degree ≤ 5; the AIR's is 3), fold arity 8 (`max_log_arity = 3`), `log_final_poly_len = 0`, 128 queries;
grinding 16 bits at the query, out-of-domain and batch-combination sites (and lookup, unused here), 12 bits per
commit-phase fold round. The `-test` ids keep `FriPreset::Test` (4 queries, ≈ 2 bits) and need the genesis
opt-in `insecure_proofs` (INV-PARAM-1). `FriPreset::Benchmark` (the old preset, 65.9 proven / 102 conjectured bits)
is kept for benchmarks only.

**INV-PARAM-3 for life2d.** The release criterion is ≥ 101 proven bits (100 plus ethSTARK Cor. 3's union-bound bit)
in the **unique-decoding regime**, which rests only on BCI+20's UD proximity gaps, not on the 2025–26 list-decoding
counts (`notes/fri-soundness-foundations.md` §3.5–3.6); |EF4| is fed as 123 bits and collision resistance as 124.
`p100_is_secure_on_the_whole_envelope` (`air/tests.rs`, via `umis_stark::security`) asserts it at every height of
the envelope below, for one proof and for `MAX_BATCH` members, at the envelope's width and constraint count and at
a narrow shape. It is 101.6 bits at the weakest corner (height 2^6, where an LDE has the fewest points per query)
and about 102.8 at height 2^12.

**INV-LIFE-PARAM-1 (shape envelope).** `validate_params` rejects any zone whose STARK shape (`shape.rs`) has more
than `MAX_HEIGHT = 2^12` rows, `MAX_WIDTH = 4096` main columns or `MAX_CONSTRAINTS = 8192` constraints, and
`Shape::of` pads every trace to at least `MIN_HEIGHT = 2^6` rows (padding rows are free, INV-LIFE-CONE-3). Security
falls with height, width and constraint count, so the bound holds only inside an envelope the verifier enforces
(`shapes_outside_the_envelope_are_rejected`; every CLI and SimNet layout is inside).

| shape (n, k, strips, rpr) | main width | constraints | height | P100 proven (UDR) | old `Benchmark` (UDR / best) |
|---|---|---|---|---|---|
| 256, 1, 8, 3 (CLI default) | 294 | 306 | 2^10 | 102.7 (× 32 members: 102.7) | 57.5 / 65.9 |
| 64, 1, 4, 2 | 100 | 104 | 2^8 | 102.5 | |
| 16, 2, 1, 2 (SimNet) | 176 | 180 | 2^6 (padded) | 101.6 | |
| envelope corner | 4096 | 8192 | 2^12 | 102.8 (× 32 members: 102.8) | |

Measured cost of P100 against `Benchmark`, back to back (Keccak config): CPU, 8 threads, one proof, 256×256 k = 1:
7.3 → 8.0 ms, 194 → 250 KB; 64×64: 2.6 → 6.7 ms (three 16-bit grinds dominate small zones on the CPU), 78 → 115 KB.
GPU (3090 Ti), 3 provers × batch 16: 0.62 → 0.77 ms per slot; one proof at a time 3.0 → 3.7 ms.

Dropping the query PoW for more queries was rejected before (it costs about 8 proven bits per 16 bits at blowup 2).
Without OOD and batch grinding no query count reaches 100 UDR bits for wide batches: the batch-combination term
(`123 − log2 |LDE| − log2 B`) binds.

## 7. Required negative tests (all in `src/air/tests.rs`, run with `--release`)

`cargo test --release -p task-life2d --features stark`. In release builds the prover does not check constraints,
so every rejection below is the verifier's. Each constraint family was checked by disabling it and seeing its
test fail (booleanity, generation-0 limbs, mask, output limbs).

- tampered trace: a flipped generation-0 or generation-1 cell, a non-boolean cell, a changed inverse witness, a
  flipped output cell, a zeroed trace row (INV-LIFE-ARITH-2, INV-LIFE-IN-1);
- generation 0 from another board with the same output; non-boolean generation-0 bits with an unchanged limb
  (INV-LIFE-IN-1);
- false `X_out` with re-derived roots, by an honest prover and by one that commits to the false claim (STARK,
  INV-LIFE-OUT-1); tampered header/prev board (native);
- unmasked absent halo regions proved against the masked claim (INV-LIFE-ARITH-3);
- tag misuse on advance and hold slots, empty and garbage proofs, a bit flip in the proof (INV-LIFE-TAG-1);
- replay to another slot or zone (INV-LIFE-FS-1);
- InputRef/payload mismatch and unreferenced inputs (INV-LIFE-IO-3);
- non-canonical states: trailing byte, truncation, header padding (INV-LIFE-CANON-1); other params; missing
  public data;
- batched proofs (INV-LIFE-BATCH-1..3, §4b); a batch head with more than `MAX_BATCH` members (INV-LIFE-BATCH-4);
- shapes outside the envelope (INV-LIFE-PARAM-1); the P100 security bound on the whole envelope (INV-PARAM-3).

The whole suite also runs on the production preset: `LIFE_PRESET=p100` (with `LIFE_HASH=keccak` for the Keccak
configuration).
