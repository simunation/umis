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

**INV-LIFE-FS-2.** Periodic data is per claim, so these proofs are not recursion-friendly as is (INV-PARAM-6):
before aggregation, move the packed limbs into a committed preprocessed or main trace bound by the state
commitment.

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

`batched_slots_verify_only_as_a_group` covers: the honest group; each proof alone; a missing member; swapped
references; a bit flip in the batch STARK; a member whose claimed next state is false (roots re-derived); replay
into another zone; an unbatched proof in the same group.

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

Same FRI presets as water (`FriPreset::Benchmark`: `log_blowup = 1`, 100 queries, 16 query-PoW bits), which caps
the constraint degree at 3; the AIR's maximum degree is 3. p3-security 0.8 (`tmp/zk-research/secprobe`, `LIFE=1`):

| Layout `n, k, strips, rpr` | main width | constraints | height | proven | conjectured |
|---|---|---|---|---|---|
| 64, 4, 4, 2 (default) | 352 | 356 | 2^8 | 65.4 | 106.5 |
| 64, 4, 2, 2 | 640 | 648 | 2^7 | 64.9 | 106.7 |
| 128, 8, 4, 2 | 1344 | 1352 | 2^9 | 65.7 | 103.6 |
| 256, 8, 8, 3 | 2016 | 2028 | 2^10 | 65.9 | 102.0 |
| 16, 2, 1, 2 (SimNet) | 176 | 180 | 2^4 | 57.3 | 111.5 |

Proven security binds on the low-degree test (as for water, INV-PARAM-3); conjectured binds on the batch
combination, which shrinks slowly with width. The Keccak configuration has the same FRI parameters and 256-bit
digests (Keccak-256 collision resistance 128 bits, above both figures), so the table holds for it too. Dropping
the 16 query-PoW bits for 16 more queries was rejected: proven security falls to 57.3 bits (`n = 64`), and keeping
65 bits would need about 132 queries (30% bigger proofs). `koalabear-test` (≈2 bits) is for tests only (INV-PARAM-1).
`LifeAir` declares `num_constraints` (it must equal what `eval` asserts; debug builds cross-check) and
`max_constraint_degree = 3`.

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
- batched proofs (INV-LIFE-BATCH-1..3, §4b).
