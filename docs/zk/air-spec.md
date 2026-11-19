# water3d transition proof: specification and invariants

Status: describes the code at `4c26c42` (Plonky3 0.8.0). This is the reference document for anyone changing
`crates/task-water3d/src/air.rs`, `crates/umis-stark`, `crates/umis-proof` or the protocol's proof paths.
Every invariant has an ID (`INV-…`). Cite the IDs in commits and tests.

## 1. What a proof claims

The input is a `TransitionStatement` (`umis-core/src/records.rs`):
`zone, params_hash, from_slot, to_slot = from_slot+1, prev_state_root, state_root, inputs_commitment, outputs_root`.

The claim is: running the water3d stencil for `k_sub` substeps from the state at `prev_state_root`, with the halos
given by the inputs under `inputs_commitment`, at slot `from_slot`, gives the state at `state_root`, whose output
faces hash to `outputs_root`.

The claim is split into two parts:

| Part | Checked by | Where |
|---|---|---|
| Blob ↔ root (keccak BMT, Swarm compatible) | **natively by the verifier**, outside the STARK | `air.rs:483-500, 565-585` |
| Faces of `X_out` ↔ `outputs_root` | natively (the verifier recomputes the faces) | `air.rs:567-570` |
| Input payload ↔ `InputRef.payload_address`, refs ↔ `inputs_commitment` | natively | `air.rs:571-585` |
| `X_in` →(k_sub stencil substeps with halos)→ `X_out`, as **field elements** | STARK (AIR below) | `air.rs:226-285` |
| Coefficients (`w`, `F`) = genesis(params, seed) | the verifier rebuilds the preprocessed commitment from `params` and `genesis_seed` | `air.rs:456-480` |
| `tide(j)` | public value that the verifier computes itself | `air.rs:339-340` |

The STARK proves nothing about hashes. The verifier must be given the full blobs (`LeafData` in the protocol).

## 2. Arithmetic

One value per substep: `num = Σ_{k=0..6} w_k·val_k + tide·F`, then `qn = ⌊num / 2^s⌋`, `rem = num − 2^s·qn`.
The constants come from the genesis water field (`stencil.rs`, `field.rs`):
- `w_k ≥ 0`, `Σ w_k ≤ 2^s`;
- `tide ∈ [256−96, 256+96]`.

In-circuit:
```
num_expr == 2^s·qn + rem                      (stencil)
qn + 2^15 == lo + 256·hi,  lo,hi ∈ [0,256)    (so qn ∈ [−2^15, 2^15))
rem ∈ [0, 2^s)                                 (lookup into T_REM = r mod 2^s)
```

### 2.1 Why the field identity is the integer identity (no wraparound)

KoalaBear `p = 2^31 − 2^24 + 1 ≈ 2^30.99`. If every `val_k` lies in `[−2^15, 2^15)`, then
`|Σ w·val| ≤ 2^s·2^15`. The right side `2^s·qn + rem` lies in `[−2^(s+15), 2^(s+15))`. If the honest `|tide·F| ≤ B_F`,
then `|lhs − rhs| ≤ 2^(s+16) + B_F`. That must be `< p` for `lhs ≡ rhs (mod p)` to imply `lhs = rhs`.

- s = 12 (devnet): `2^28 + B_F`, a large margin.
- s = 14: `2^30 + B_F`. Sound as long as `B_F < p − 2^30 ≈ 1.06·10^9`, which holds for the genesis bounds.
- **s ≥ 15: unsound.** Example at s = 16: `2^16·32512 + 1 = p`. `validate_params` currently accepts `8..=16`.

**INV-ARITH-1.** `shift ≤ 14`, enforced in `validate_params` (fixed 2026-11-19; `shift_above_14_is_rejected`).

**INV-ARITH-2.** Every value that enters a stencil row as an integer, i.e. `X_in` and the halo cells, lies in
`[−2^15, 2^15)`. In-circuit, `qn` is range-checked, but `q` at t = 0 (pinned to the periodic `x_in`) and the halo values
are not. They need the native check in INV-CANON-1.

**INV-ARITH-3.** Weights are non-negative, `Σ w ≤ 2^s`, and `|tide·F| ≤ B_F` with `2^(s+16) + B_F < p`. These are
genesis-time checks (`check_coefficient_bounds`); add the `B_F` check explicitly when changing the field or the shift.

**INV-ARITH-4.** LogUp multiplicities: Σ over lookups of (count weight) × height < p. Today that is 19 × 2^14. It is
checked by p3-lookup (`types.rs:474`). It must be rechecked if `k_sub`, `n_loc` or the number of lookups grows; for
example, a multi-slot trace multiplies the height.

### 2.2 Canonical encoding (confirmed soundness bug)

The blobs are little-endian i32. The verifier decodes them and maps them with `Val::from_i64`, which reduces mod p
(`air.rs:287-291, 315-316, 323`). Every honest value `v ∈ [−2^15, 2^15)` therefore has other i32 encodings, `v + p`
and/or `v − p`, that map to the same field element. These produce different keccak roots but the same field values.

Reproduced on 2026-11-03 (`tmp/zk-research/repro-aliasing/`):
1. A prover that builds the periodic `x_out` and the keccak digest from an aliased `X_out` blob (cell 0 vx: 79 → 2130706512) gets a proof.
2. The **unmodified verifier accepts** that proof (`Ok`) for a `state_root` that differs from the honest one.
3. Native `step()` from the aliased blob then diverges from the honest run.

The same attack works on `X_in` (a prior state) and on the halos. Today members still re-execute natively, which
catches it as InvalidTransition. **It becomes a full soundness break as soon as proofs replace re-execution.**

**INV-CANON-1.** Before `claim_data`, the verifier must reject unless every decoded i32 in `X_in`, `X_out` and every
halo lies in `[−2^15, 2^15)`. The halos must also have exactly `face_bytes` length (see INV-IO-2). The prover side
should assert the same. **Implemented 2026-11-19:** `decode_cells` (`lib.rs`, `VAL_RANGE`) accepts only in-range
cells of the exact length, for the native task (`decode_state`, `decode_inputs`) and for the verifier
(`check_roots`, `halos`), so every state and halo has exactly one encoding (INV-CANON-2).
`non_canonical_blobs_are_rejected` replays the exploit (aliased X_out, X_in and halo with re-derived roots, digest
and a proof for the aliased claim) and a short halo; with the range check disabled the aliased X_out is accepted.

**INV-CANON-2.** Every committed blob has exactly one valid encoding (the `seg` codec rule 4 applies to records; the
raw blobs need INV-CANON-1). Test: `non_canonical_blobs_are_rejected`.

## 3. Trace layout (summary; the authoritative version is the doc comment at `air.rs:1-38`)

The height is `max(k_sub·C, 2^s, 256)` rounded up to a power of two (C = n³). Row `r = t·C + i`. Rows `≥ k_sub·C` are
padding: `active = 0`, `qn = 0`, so `lo = 0` and `hi = 128`.

| Group | Columns |
|---|---|
| main (46) | `q[4] nb[6][4] qn[4] rem[4] lo[4] hi[4] m_rem m_byte` |
| preprocessed (32) | `addr active first last has[6] mult in_mult wv[7] wc[7] fv[3] fc t_rem t_byte` |
| periodic (32, period C) | `x_in[4] x_out[4] halo[6][4]` (halo is 0 on interior cells) |
| public (17) | `tide`, keccak(`"water3d-air"` ‖ params_hash ‖ slot_be ‖ X_in ‖ X_out ‖ halos) as 16 × u16 |

Constraints (all row-local, degree ≤ 3):
- stencil;
- byte decomposition;
- halo: `active·(1−has_d)·(nb_d − halo_d)`;
- input: `first·(q − x_in)`;
- output: `last·(qn − x_out)`.

Buses (LogUp, global, one AIR):

| Bus | Provides | Reads |
|---|---|---|
| `w3d_mem` (addr, v0..v3) | `(addr + C, qn) × mult`, `(addr, q) × in_mult` | `(addr, q)` gated by `active`; `(addr + off_d, nb_d)` gated by `has_d` |
| `w3d_rem` | `(t_rem) × m_rem` | `rem_f` gated by `active` |
| `w3d_byte` | `(t_byte) × m_byte` | `lo_f`, `hi_f`, **ungated** (padding rows contribute lo = 0, hi = 128, and those are counted) |

**INV-BUS-1.** Every memory address is provided at most once, with a **preprocessed** multiplicity equal to the exact
number of readers. A prover-chosen multiplicity on the memory bus would break the "read = write" property. Only the
table-side multiplicities of the range buses (`m_rem`, `m_byte`) may be prover columns.

**INV-BUS-2.** Every read is gated by a preprocessed boolean (`active`, `has_d`) or is constant 1. Never gate with
a main-trace column unless that column is constrained to be boolean and that gate is part of the claim.

**INV-BUS-3.** Address arithmetic stays inside `[0, k_sub·C + C)`, so no mod-p collisions. The current maximum is
`< 2^17`.

**INV-BUS-4.** The payload width is uniform per bus. LogUp challenges come from EF4 after the main commitment (the
p3 0.8 batch-stark order). See `docs/zk/notes/fiat-shamir-and-lookups.md`.

## 4. Fiat–Shamir binding

The p3-batch-stark 0.8 transcript absorbs the widths, instance count, degree bits, main commitment, **public values**,
preprocessed commitment, permutation/lookup commitment and terminals, quotient commitment, OOD point and the FRI
transcript. It does **not** absorb periodic columns.

**INV-FS-1.** Everything the verifier evaluates outside the transcript must be bound by a public value: periodic
`x_in`, `x_out` and the halos go through the keccak digest. Remove the digest only if periodic data becomes bound
by the framework (multi-stark has `periodic_statement`; batch-stark on main still doesn't) or the data leaves the
periodic columns (in-circuit commitment, roadmap phase 2).

**INV-FS-2.** The digest's domain tag, the field order and the byte layout are protocol constants. Changing them is a
proof-format break; re-pin the golden vectors.

**INV-FS-3.** The verifier computes every public value itself (`tide`, digest). Never take one from the proof or the
prover.

**INV-FS-4.** The preprocessed commitment (the effective verifying key) is rebuilt from on-chain `params` and the genesis
seed, never received. When verifying keys are cached, key them by `(keccak(params), seed)`, as `setup()` does.

## 5. I/O and statement checks (native, in `verify_inner`)

- **INV-IO-1.** `to_slot == from_slot + 1` and `params_hash` matches the zone setup. This changes when proofs cover a
  whole commitment; update all three sites listed in `protocol-proof-flow.md` §8.
- **INV-IO-2.** Halos: a present input must have exactly `face_bytes` bytes. `halos()` and `decode_inputs` both
  reject any other length (fixed 2026-11-19; it used to be read as an absent neighbour).
- **INV-IO-3.** `inputs.len() == refs.len()`, each payload's bmt address equals its ref, and
  `list_root(refs) == inputs_commitment`. All three hold today.
- **INV-IO-4.** `outputs_root` is recomputed from `X_out`. That holds today.

## 6. Parameters and configuration

- **INV-PARAM-1.** No production genesis may select `koalabear-test` (≈2 bits). Since 2026-11-19 `Genesis::validate`
  rejects a plonky3 genesis whose `stark_config_id` is not in `SECURE_STARK_CONFIGS` (`koalabear`,
  `koalabear-keccak`) unless it sets `insecure_proofs: true`, and `umis-cli node` refuses such a genesis without
  `--allow-insecure-proofs` (`insecure_stark_configs_need_the_opt_in`). Unknown ids fail in `backend_for`. Still
  open: rejecting `L = 0` with plonky3 (SimNet runs that combination on purpose).
- **INV-PARAM-2.** The lookup degree budget passed to `ProverData::from_airs_and_degrees_with_lookup_budgets` must be
  derived from the preset's `log_blowup` (max constraint degree `2^log_blowup + 1`), not the literal `1`. Done:
  `self.preset.log_blowup()`; the per-AIR lookup budget stays 3, the shape the bound below is computed at.
- **INV-PARAM-3.** The stated security level is computed with `p3-security` for the exact shape and asserted in a test.
  Since 2026-11-19 the `koalabear*` ids use `FriPreset::P100` (blowup 4, arity 8, 128 queries, 16-bit query/OOD/
  batch/lookup grinding, 12-bit commit grinding): ≥ 101 proven bits in the **unique-decoding** regime (no
  dependence on 2025–26 list-decoding results) at every height `validate_params` allows (2^8…2^16),
  `p100_is_secure_for_every_water_shape`, via `umis_stark::security::proven_udr_bits`. The old `Benchmark`
  preset (≈66 proven / ≈101 conjectured) remains for benchmarks only. `p100_proofs_verify_and_bind_the_preset`
  proves and verifies under P100 and rejects a Test-preset proof.
- **INV-PARAM-4.** Trace height × blowup must not exceed KoalaBear's two-adicity (2^24).
- **INV-PARAM-5 (recursion compatibility).** Proofs that may be recursively aggregated (roadmap phase 3) must use
  0 PoW bits at the batch, lookup and OOD steps. The Plonky3-recursion in-circuit verifier rejects non-zero PoW at
  those steps. A compatible ≥100-proven-bit preset is `log_blowup 2`, 128 queries, commit and query PoW 16, which gives
  ≈100.6 bits (`notes/plonky3-implementation-guide.md` §5).
- **INV-PARAM-6 (recursion compatibility).** Per-claim data must not live in periodic columns of a proof that will be
  recursed: the recursive verifier circuit hard-codes periodic columns, so every slot would need its own circuit. Move
  X_in, X_out and the halos onto a bus fed by a Poseidon2 AIR first (phase 2; guide §3.5). That removes the keccak
  digest (INV-FS-1) as well.

## 7. Determinism

- **INV-DET-1.** Proving with the test preset (no grinding) is byte-deterministic, and native and wasm output match
  (`tests/golden-proof-4.txt`, checked by `tests/stark.rs` and `tests/wasm.rs`). With grinding and more than one rayon
  thread, proofs are valid but not byte-reproducible on 0.8.0 and 0.9.0-rc.2 (measured); deterministic grinding exists only
  on Plonky3 main. Key pools and memos by statement, not by proof hash. Never use proof bytes as a consensus identifier without accounting for that, or for the
  0.8.0 MMCS cap-root malleability fixed upstream in #2378.
- **INV-DET-2.** CPU feature flags (AVX2/AVX-512) must not change any proof or state. Verified on 2026-11-03: with
  `-C target-cpu=native`, the golden proof and golden trace still match.

## 8. Required negative tests

The existing tests are in `air.rs` (`tampered_traces_are_rejected`, `honest_proof_verifies_and_tampering_is_rejected`)
and `umis-stark/src/toy.rs`. Add:
- done: aliased (non-canonical) X_out, X_in and halo with a re-derived digest (INV-CANON-1);
- done: shift 15 and 16 rejected by `validate_params` (INV-ARITH-1);
- done: a wrong-length halo rejected (INV-IO-2);
- the digest omitted or altered while the periodic data stays the same: rejected (INV-FS-1);
- a proof replayed for a different `slot`, `zone` or `params_hash`: rejected;
- done: a proof made under the `Test` preset rejected by a P100 verifier (preset binding);
- a tampered table multiplicity or memory multiplicity, and an extra read on padding rows;
- done: `koalabear-test` in a genesis without `insecure_proofs` rejected (INV-PARAM-1).
