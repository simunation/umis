# Plonky3 0.8.0 internals as used by umis (water3d AIR)

Scope: read-only analysis of `~/.cargo/registry/src/*/p3-*-0.8.0` against
`crates/task-water3d/src/air.rs`, `crates/umis-stark/src/config.rs`, `crates/umis-stark/src/toy.rs`.
Paths below are relative to the registry `src/` dir (e.g. `p3-batch-stark-0.8.0/src/transcript.rs:587`).
Measured numbers come from two scratch programs (`scratchpad/secprobe`, `scratchpad/profprobe`), see §4/§5.

**Bottom line**

- I found no soundness bug in how umis uses Plonky3 0.8.0. The Fiat–Shamir binding is complete for umis' statement, the LogUp usage meets every requirement, and the claims in `air.rs` hold.
- The real gaps are in the security level and in the build:
  1. `FriPreset::Benchmark` gives ≈101 conjectured bits for this instance, not the "~116" the `config.rs` comment says. Its proven soundness is only ≈66 bits.
  2. The native builds have no `target-cpu`, so KoalaBear and Poseidon2 run with **no SIMD packing**. Enabling AVX2 makes proving 3–4× faster.
  3. `koalabear-test` (4 queries, about 2–4 bits) is a genesis-selectable config id. Gate it.

---

## 1. Fiat–Shamir transcript (prove_batch / verify_batch)

The transcript is a declared *pattern*, driven by `p3_challenger::fs` (`p3-batch-stark-0.8.0/src/transcript.rs:14-62`, pattern at `:249-400`).

**Seeding.** A `DomainSeparator` is built from:
- `VERSION=1` and `NAME="p3-batch-stark"` (`:80-83`);
- the hash of the step pattern;
- an instance label (`domain_separator`, `:408-432`) holding the instance count, every main-trace width and every preprocessed width.

It is absorbed into a fresh `DuplexChallenger` before anything else (`p3-challenger-0.8.0/src/fs/domain_separator.rs:136-158`, `fs/state/prover.rs:72-85`). The pattern fixes these at seed time:
- the number of instances;
- each instance's public-value count (`Length::Fixed(count)`, `:280-287`);
- whether a preprocessed commitment, lookups or a randomization commitment exist;
- the lookup and OOD PoW difficulties.

**Order of absorption and sampling** (prover `prover.rs:234-656`, verifier `verifier/mod.rs:629-710`):

| # | Step | Code |
|---|------|------|
| 1 | per instance: extended `degree_bits` (absorbed as an EF element) | `transcript.rs:587-594`, `prover.rs:253` |
| 2 | main trace commitment (Merkle root) | `:601-606`, `prover.rs:271` |
| 3 | per instance: **public values** (`Kind::Public`, absorbed, not on the wire) | `:608-611`; `fs/state/prover.rs:182-205`, verifier `fs/state/verifier.rs:574-596` |
| 4 | **preprocessed commitment** (the verifying key), if any | `:615-620`, `prover.rs:272` |
| 5 | lookup PoW (`lookup_proof_of_work_bits`; 0 in umis: nothing absorbed, witness pinned to 0) → sample `alpha`, `beta` (EF4) | `:628-659`; verifier zero-bit pinning `:872-876` |
| 6 | permutation (LogUp aux) commitment, then **every lookup terminal** (EF) → sample the constraint-folding challenge | `:670-685`, `prover.rs:371` |
| 7 | quotient-chunks commitment (+ randomization commitment under zk) | `:688-695`, `prover.rs:552` |
| 8 | OOD PoW (`ood_proof_of_work_bits`; 0) → sample `zeta` | `:703-712` |
| 9 | bracketed sub-protocol: the FRI PCS | `:721-727`, `prover.rs:647` |

Step 9 continues on the same sponge with its own seed (`p3-fri-0.8.0/src/pcs_transcript.rs:1-50`):
- absorb every claimed opened value, per (commitment, matrix, point);
- `batch_pow`, then sample the batching `alpha`.

Then FRI runs its own sub-protocol (`p3-fri-0.8.0/src/transcript.rs:1-60`):
- per round: commitment, then `commit_pow`, then the folding challenge;
- the final polynomial;
- `query_pow`, then the query indices.

FRI's blowup, arity and per-round arities sit in FRI's own instance label. The query count is fixed by the step count.

**What is *not* absorbed**

- **Periodic column values.** `BatchShape` has no periodic field and no step exists for them. The verifier evaluates its *own* periodic data at `zeta` (`verifier/mod.rs:798-802`).
- **The AIR's constraints and identity.** Only widths and counts are bound. The constraints are *checked*: the verifier re-derives the symbolic constraints, the lookups and the quotient-chunk count from its own AIR and `CommonData` (`verifier/mod.rs:426-473, 723-832`).
- **`CommonData.lookups`.** These are recomputed by the verifier in `setup` and are never read from the proof.

The preprocessed commitment is **both absorbed (step 4) and checked**:
- it is opened through the PCS round at `verifier/mod.rs:253-313`;
- `meta.degree_bits == proof.degree_bits` is enforced (`:285`), which pins the trace height to the one used at setup.

**Weak-FS assessment for umis: no issue found.**

The statement is (shape, coefficients, `tide`, X_in, X_out, halos). Each part is bound before the first challenge (`alpha`, `beta` in step 5):

| Part of the statement | How it is bound |
|---|---|
| shape constants (`shift`, `cells`, offsets) | `params_hash`, inside the keccak digest in the public values |
| coefficients (`wv`, `wc`, `fv`, `fc`, `has`, `mult`) | preprocessed commitment |
| `tide` | public value 0 |
| X_in, X_out, halos | keccak digest (16 × 16-bit limbs; domain-tagged `water3d-air`) |

The verifier recomputes the periodic columns and the digest from its own data (`air.rs` `verify_inner`/`claim_data`), so the digest is a deterministic function of the periodic data. Keccak collision resistance makes the binding complete.

The digest does not need to appear in any constraint; it only needs to be absorbed. batch-stark asserts `public_boundary_io().is_empty()` (`prover.rs:136-143`), and nothing else in the protocol treats public values specially.

The toy AIR copies X_in ‖ X_out straight into the public values. That is equally sound, just bigger.

**Minor, not exploitable here.**
- **Merkle cap malleability (#2378, `32ec6c30f`, landed 2026-09-29, *after* 0.8.0).**
  - The 0.8.0 MMCS verifier accepts a cap carrying extra, unauthenticated roots, and the transcript absorbs them.
  - Consequences: proofs are malleable (non-unique bytes), and challenges can be re-rolled cheaply. Under the usual random-oracle counting this is not a forgery.
  - It matters only if umis treats proof bytes or proof hashes as unique identifiers. If it does, canonicalize: re-serialize, or check the cap length.
- **Zero-difficulty PoW witnesses** are pinned to 0 at both sites, so they are not malleable (`transcript.rs:173-176, 252-255` region).

## 2. LogUp in p3-lookup 0.8

**How the AIR calls compile to lookups**

- `LookupBus::lookup_key(key, count)` becomes `push_interaction(name, key, count)` (`p3-lookup-0.8.0/src/bus.rs:54`).
- `table_entry(key, m)` pushes `Count::provided(-m)`: a negative count with weight 0 (`bus.rs:74-87`).
- `Count::bounded(e, w)` carries a per-row magnitude bound `w`. Constants carry `|c|` (`count.rs:57-102`). The bound is "sound only if the AIR constrains the count".
- Every `LookupBus` interaction is **global**, keyed by bus name (`types.rs:190-201`). One `Lookup` (one fraction column) is created per interaction.

**Packing (`pack_same_bus_with_degree`, `types.rs:272-357`).** Same-bus interactions are folded greedily into one column while the fraction-pin degree stays ≤ the budget.

**Gadget columns and constraints (`logup.rs:83-98, 314-464`).** Column 0 is a shared EF accumulator, plus one EF fraction column per packed lookup.

| Constraint | Applies to | Degree |
|---|---|---|
| Fraction pin `U·f − V = 0` | every row (not gated) | `max(1+Σdeg e_i, max_i(deg m_i + Σ_{j≠i} deg e_j))` (`:113-156`) |
| `acc[0] = 0` | first row | — |
| `acc[r+1] = acc[r] + Σ_c f_c[r]` | transitions | — |
| `terminal = acc[n−1] + Σ f[n−1]` | last row | — |

- Denominators are `prefix[bus] − Σ_k β^{k−1−i}·e_i`, with `prefix[bus] = α + (bus+1)·β^W`. W is the widest payload, 5 here (`challenges.rs:70-91`; layout in `transcript.rs:454-543`). Tuple widths per bus must be uniform; this is asserted.
- Each AIR's terminal is absorbed (step 6).
- The verifier rejects unless Σ terminals = 0 (`logup.rs:466-481`, called at `verifier/mod.rs:843`).
- **Local vs global:** with one AIR, a global bus degenerates to a local check. The single per-AIR sum must be 0, and bus separation comes from the β^W prefix offset.

**Multiplicity soundness.** `check_multiplicity_height_bound` enforces Σ_i weight_i·height_i < p, run by both prover and verifier (`types.rs:474-502`; `prover.rs:159`, `verifier/mod.rs:478`).
- umis query weights: mem 7 (`active`, `has_d` are preprocessed booleans) + rem 4 + byte 8 = 19.
- 19·2^14 ≈ 3.1·10^5 ≪ p ≈ 2^31. ✓

**Budget call `from_airs_and_degrees_with_lookup_budgets(.., &[3], 1)`** (`p3-batch-stark-0.8.0/src/common.rs:230-405`).
- It computes `log_chunks` from the AIR plus the *unpacked* lookups. Max degree is 3, so `log_chunks = 1`.
- So `free_budget = 2^1 + 1 = 3` (`:351`).
- The override 3 equals the free budget. The call therefore behaves **exactly like `from_airs_and_degrees`**, and the `log_blowup=1` argument is only used when the override exceeds the free budget (`:360-399`).
- It is harmless, but redundant. Also, the hard-coded `1` would go stale if the preset's `log_blowup` changed.

**Resulting layout for water3d.** Every key is degree 1 and every multiplicity has degree ≤ 1, so two interactions fit per column (degree 3):

| Bus | Interactions | Columns |
|---|---|---|
| mem | 9 (2 tables + 7 reads) | 5 |
| rem | 5 | 3 |
| byte | 9 | 5 |

- That is 13 fraction columns + 1 accumulator = 14 EF = **56 base columns** committed, against 96 unpacked. The profile confirms a `56x16384` LDE.
- Constraints: 40 base + 13 pins + 3 accumulator = 56.
- Quotient: 2 chunks.

**Soundness requirements, and whether umis meets them**
- (a) The query count per value is < p, so `Σ m_j/(X−t_j) = Σ c_i/(X−a_i)` forces every queried value into the table: ✓ (height bound above).
- (b) Query-side counts must really be bounded by their weights: ✓. `active` and `has_d` are preprocessed, and the byte counts are the constant 1.
- (c) Challenges come from the extension field and are sampled after the main commitment: ✓ (EF4, about 124 bits).
  - p3-security's bound is ε ≤ N(W+2)/|EF| with N = 23·2^14 (`p3-security-0.8.0/src/logup.rs:1-98`).
  - That gives **102.7 bits** (100.7 at 2^16 rows), with no lookup PoW.
- (d) Uniform tuple widths per bus: ✓ (5 / 1 / 1).

**The specific claims in `air.rs`**
- *"Byte lookups are not gated."* True and fine.
  - Padding rows set `qn = 0`, so `lo = 0` and `hi = 128`. These are counted in `byte_counts`.
  - Ungated lookups only cost table multiplicity.
  - `rem` is gated by `active`. Its padding value is otherwise free, but unused: `mult` and `in_mult` are 0 there, so a padding `qn` is never read.
- *"M_REM / M_BYTE are unconstrained prover columns."* True and sound. LogUp never needs the table-side multiplicity constrained.
  - A wrong value only makes the sum fail to be zero, except with ≈2^-103 probability.
  - Table values repeat (`r mod 2^s`, `r mod 256`), and the prover may split credit across repeats.
- *Memory bus.*
  - Table multiplicities are preprocessed (`MULT`, `IN_MULT`), and each address (`addr+C` from row `addr`; `addr < C` at t=0) is provided exactly once.
  - So every `(addr, value)` read must equal the value written.
  - The toy uses a *main* `MULT` column. That is still sound for the same reason: addresses are unique, so subset inclusion is enough.
- *"LogUp trace generation does not see periodic columns."* True in 0.8.
  - `LookupTraceBuilder` has no periodic slice (`traits.rs` around `:5-60`), so the AIR keeps periodic data out of bus messages.
  - Fixed on main (#2574), but not in any release.

**AIR-level note (not Plonky3).** The field identity `Σ w·val + tide·F = 2^s·qn + rem` relies on `|vals| ≤ 2^15` so that it does not wrap mod p.
- In-AIR, values read from memory at t ≥ 1 are range-checked.
- `x_in` and the halos are trusted verifier inputs. They are bounded only because they came from earlier valid proofs or from genesis.
- If a zone could ever accept an unproven or out-of-range input payload, wrap-around becomes possible. Consider asserting `|v| ≤ 2^15` natively in `verify_inner`.

## 3. Periodic columns

**Verifier** (`verifier/mod.rs:798-802`; `p3-commit-0.8.0/src/periodic.rs:132-154`, `domain.rs:395-440`).
- `PeriodicColumns::new` requires every length to be a power of two that divides the trace height. That holds: C = n³ with n ∈ {4, 8, 16}.
- Columns are grouped by period. For each group, ζ is projected to `ζ^(N/C)`, and `interpolate_coset` evaluates all k columns with one batch inversion.
- For umis that is one group: 4096 points, 32 columns. Cost is one batch inversion of 4096 EF4 elements plus about 131k EF×base multiplications, roughly 1 ms.
- Measured: "verify constraints" took **1.2 ms** of the ~17 ms `verify_batch`.

**Degree accounting.** With periodic columns present, the symbolic degree uses the exact `poly_degree(trace_len, periods)`. A period-C column contributes (C−1)·N/C = N−4 (`p3-batch-stark-0.8.0/src/symbolic.rs:131-171`). So `active·(1−has)·(nb−halo)` is degree 3, as intended.

**Prover** (`prover.rs:893-915, 1030-1035`; `p3-fri-0.8.0/src/periodic.rs:43-120`).
- Builds a compact table of `max_period × blowup` = 8192 rows × 32 columns by coset DFT.
- Reads it with modular, pack-aligned indexing during quotient evaluation. Cost is negligible: dozens of 4096-point DFTs.
- Periodic values are visible only to the constraint folders. LogUp trace generation does not see them in 0.8.

## 4. FRI parameters and security

**`FriParameters::new_benchmark`** (`p3-fri-0.8.0/src/config.rs:179-192`):

| Field | Value |
|---|---|
| `log_blowup` | 1 |
| `log_final_poly_len` | 0 |
| `max_log_arity` | 1 |
| `num_queries` | 100 |
| `batch_proof_of_work_bits` | 0 |
| `commit_proof_of_work_bits` | 0 |
| `query_proof_of_work_bits` | 16 |

- Folding is binary. `fold_schedule` (`config.rs:323-363`) folds from the 2^15 LDE down to a constant, so there are 14 commit-phase rounds.
- `StarkConfig::new` sets lookup and OOD PoW to 0 (`p3-uni-stark-0.8.0/src/config.rs:120-125`). `with_lookup_proof_of_work_bits` and `with_ood_proof_of_work_bits` exist.
- `new_benchmark_high_arity`: arity 8, batch PoW 10.

**Probe.** p3-security 0.8 provides `proven_security_report`, `conjectured_security_report`, `legacy_security_report` and `logup::security_term`. `scratchpad/secprobe/src/main.rs` calls them with the umis shape:

| Parameter | Value |
|---|---|
| \|EF\| | 2^124 |
| collision resistance | 124 bits |
| constraints | 56 |
| max degree | 3 |
| quotient chunks | 2 |
| `max_combo` | 2 |
| batched functions | 198 (`p3-batch-stark` `security.rs` formula: 46 + 32 + 2·4 + 2·14·4) |
| LogUp | 23 interactions, width 5 |

Its proof-size model, using pruned multiproofs and 4-byte elements, predicts 255 KB for `new_benchmark`; the measured size is **261 KB**.

**Results at 2^14 rows** (2^16 rows in brackets):

| Preset | Proven (best regime) | Conjectured | Binding conjectured term | Est. size |
|---|---|---|---|---|
| umis `Test` (lb1, q4, no PoW) | **2.0** | **3.9** | ldt-query | 22 KB |
| `new_benchmark` | **65.9** (UDR 57.5) | **101.4** [99.4] | batch-combination 101.4 (ldt-query 113.2, LogUp 102.7) | 255 KB |
| high_arity (a3, batch PoW 10) | 65.9 | 102.7 | logup | 221 KB |
| lb1 q100 a2 lfp3 | 65.9 | 101.4 | batch | 212 KB |
| lb2 q50 a2 lfp3, lookup budget 5 (8 aux cols, 4 chunks) | 66.0 | 100.6 | batch | 130 KB |
| lb3 q34 a3 lfp3 qpow20 | 70.9 | 99.4 | batch | 99 KB |
| `new_benchmark` + OOD/batch/lookup PoW 8 | 65.9 | 109.4 | batch | 255 KB |

**Grid** (arity 4, final poly 8; all non-FRI PoW sites at 16; "cpow" is commit-phase PoW):

| Config | Proven | Conjectured | Size |
|---|---|---|---|
| lb1 q200 pow16 cpow16 | 104.0 | — | 378 KB |
| lb2 q100 pow16 cpow0 | 96.5 | 106.4 | 231 KB |
| lb2 q100 pow16 cpow16 | 104.5 | 116.6 | 231 KB |
| lb2 q128 pow16 cpow0 | 102.8 | — | 284 KB |
| lb3 q64 pow16 cpow16 | 103.2 | — | 177 KB |
| lb4 q50 pow16 cpow16 | 103.9 | — | 158 KB |

Conclusions:
- **Conjectured.** The `config.rs` comment ("~116 conjectured bits") counts only the query term.
  - The full composite is ≈101 bits. It is bound by the opening-batching proximity term, `(k−1)·n/|F|` with k = 198 batched columns.
  - LogUp is next, at 102.7.
  - Both are cheaply raised by `batch_proof_of_work_bits` and `with_lookup_proof_of_work_bits`; 8 bits each gives ≈109.
  - A third FRI site also needs attention: the commit-phase term (107–109) needs `commit_proof_of_work_bits` beyond that.
  - The air-composition cap is about 118.
- **Proven** (Johnson/list decoding, DKT26 Johnson-regime line correlated-agreement bound, as cited by p3-security): ≈66 bits for `new_benchmark`. Even 100 queries at rate 1/2 over a 124-bit field cannot go higher.
  - Reaching ≥100 proven bits needs lb2 with q100–128 (230–285 KB), or lb3–4 with q50–64 (160–180 KB).
  - Either way, 16-bit grinding at OOD, batch, lookup and commit is required.
  - Prover cost grows with the blowup.
- **Proof size levers.** Pruned multiproofs already deduplicate the top ~log2(q) Merkle levels, so `cap_height > 0` buys almost nothing; it adds 2^c digests per tree. The real levers:
  - higher `max_log_arity` (2–3);
  - `log_final_poly_len` 3–5;
  - more blowup with fewer queries.

## 5. Where prover time goes, and levers

Measured with `scratchpad/profprobe`: `bench_air` plus a tracing subscriber, n_loc 16, 2^14 rows, `new_benchmark`, second proof. The CPU has AVX2 but no AVX-512.

| Span | Default build, 2 threads | `-C target-cpu=native`, 2 threads |
|---|---|---|
| main LDE 46×2^14 → 2^15 | 15 ms | 6 ms |
| Merkle main 46×2^15 (Poseidon2-16) | 88 ms | 12 ms |
| generate LogUp trace (23 batch inversions) | 10 ms | 7 ms |
| perm LDE 56 cols | 18 ms | 5 ms |
| Merkle perm 56×2^15 | 112 ms | 16–21 ms |
| quotient evaluation (EF4, 56 constraints, 2^15 points) | 106 ms | 38–40 ms |
| Merkle quotient 8×2^15 | 40 ms | 5.5 ms |
| openings (evaluate + reduce) | ~30 ms | ~10 ms |
| FRI (14 rounds) | 38 ms | 8 ms |
| **prove_batch total** | **450–510 ms** (1 thread: 742 ms) | **115–165 ms** (1 thread: 190–266 ms; 24 threads: 55–62 ms) |

The default-build total matches the plan's "397 ms native on 2 threads" and "682 ms wasm".

**Findings**
1. **No SIMD on native today.** `p3-monty-31` packing is compiled only under `target_feature = "avx2"`, `"avx512f"` or `"neon"` (`p3-monty-31-0.8.0/src/lib.rs:19-50`). umis' `.cargo/config.toml` sets no `target-cpu`, so x86 builds use `no_packing`: scalar fields and scalar Poseidon2.
   - Fix: `RUSTFLAGS="-C target-cpu=native"`, or an explicit `-C target-feature=+avx2` (`+avx512f`) build per target. Result: **3–4× faster**.
   - aarch64 already has NEON by default.
2. **wasm.** There is no wasm32 `simd128` packing for monty-31/KoalaBear in 0.8 (only `p3-goldilocks/src/lib.rs:44-48` has one), nor on main. Browser proving stays scalar, so Poseidon2 hashing is about half of prover time.
   - Options:
     - **(a)** A byte-oriented MMCS (Keccak via `p3-keccak`, or Blake3) plus `SerializingChallenger32`. About 14 MB of leaf data, so hashing costs tens of ms instead of ~250 ms. Recursion-unfriendly, but umis does not recurse.
     - **(b)** A wider sponge (Poseidon2-24, rate 16), which roughly halves leaf permutations.
     - **(c)** `-C target-feature=+simd128` for LLVM autovectorization only. A small gain at best.
3. **Column count drives hashing and LDE.**
   - The LogUp aux trace (56 base columns) is bigger than the main trace.
   - Fewer interactions mean fewer columns. One option: range-check `qn+2^15` as one 14-bit limb against the `ADDR` column, which already lists 0..2^14−1, plus 2 boolean bits. That replaces 8 byte lookups with 4 and drops `T_BYTE`.
   - Merging the rem and byte buses only helps with a tag element, and saves about 1 EF column.
   - The 24 `nb` copy columns and their 6 memory reads are the dominant AIR cost; changing that means an AIR redesign.
4. **Blowup.**
   - lb1 is cheapest for the prover. lb2 (budget 5: 9 aux EF columns, 4 quotient chunks) costs about 1.8× in hashing but halves queries, giving roughly 130 KB at the same conjectured level.
   - The budget override must then be raised to 5, and the `log_blowup` argument passed from the preset instead of a literal `1`.
5. **FRI.**
   - `max_log_arity` 2–3 and `log_final_poly_len` 3–5 cut FRI trees and proof size by 15–20%, with little prover change.
   - Grinding: query PoW 16 costs about 65k permutations, which is cheap. Commit-phase PoW is paid every round (14 × 2^bits), so keep it ≤ 12 or skip it.
6. **Parallelism.** `p3-maybe-rayon/parallel` is on natively. Scaling is good: 24 threads gives 55 ms.

## 6. Upstream after 0.8.0 (git main 0d803737b, 2026-10-26; crates.io newest is 0.9.0-rc.2, 2026-10-16)

- **Releases.**
  - rc.1 (`5b23a4fbf`) and rc.2 (`fe5e46d75`) are version-bump commits on side bases.
  - rc.2 includes #2378 (the cap fix), #2379, #2381, #2372 and #2401.
  - It does **not** include later fixes: #2566, #2574, #2577–2581, #2585, #2517, #2471.
  - Main's `Cargo.toml` still says 0.8.0.
- **Soundness/acceptance fixes reviewed against umis' configuration.** None is exploitable here.

| PR | What changed | Why umis is not affected |
|---|---|---|
| #2566 | transition-factor degree undercount | only on the no-periodic path; umis has 32 periodic columns |
| #2574 | lookups may now read periodic columns; selectors are forbidden inside lookup counts | umis uses preprocessed `FIRST`/`LAST`, not selectors |
| #2581 | the prover folded the real next row while the verifier folded zeros when no next-row columns are declared | completeness only; umis reads `current_slice()` only |
| #2585 | an undeclared next-row read is now a typed error | diagnostic only |
| #2579 / #2580 | bus/tuple widths and grinding bounds become errors instead of panics | widths 5/1/1 are consistent; PoW 16 is supported |
| #2378 | cap malleability | see §1 |
| #2517 | deterministic grinding | makes the "proofs are non-deterministic with grinding" comment in `config.rs` outdated |

- **Transcript.** batch-stark's transcript is unchanged (`VERSION 1`). It still does not absorb periodic data or the AIR. #2381 adds `BaseAir::periodic_statement`, but binds it only in **p3-multi-stark**. Keep the keccak digest.
- **Deprecations (#2559).** Only bn254, monolith(-air), poseidon1-air, poseidon2-air and rescue. **p3-batch-stark is not deprecated.**
- **p3-multi-stark** (already in 0.8) is a different stack: a multilinear zerocheck with WHIR or binary PCS, and LogUp-GKR via `p3-bus`, `fractional_gkr` and the new `p3-gkr` crate. Its API is `setup` / `prove` / `verify`, plus `*_with_security`. Moving to it means rewriting `config.rs` and the prove/verify glue. The AIR `eval` would mostly survive (`InteractionSymbolicBuilder`/`BusSymbolicBuilder` bounds).
- **Other.**
  - FRI, WHIR and STIR are all present. WHIR is usable only through multi-stark.
  - zk is via `HidingFriPcs`, which now requires hiding MMCSs.
  - No recursion crate.
  - No wasm simd128 for monty-31.
- **Migration effort to main or a future 0.9.**
  - `config.rs`: only the version pins.
  - `air.rs`: none. Optionally, lookups may now read periodic columns.
  - `toy.rs`: none, apart from updating the stale doc comment at `toy.rs:39`.
  - Then re-pin the test vectors, since grinding witnesses change.
  - Less than an hour. The practical blocker is that no stable 0.9.0 exists, and rc.2 lacks the late fixes.
  - `BatchProof`, FRI and MMCS serde formats are unchanged.

## 7. Recommendations

1. Gate `koalabear-test` (≈4 bits) out of any non-dev genesis.
2. Fix the `config.rs` comment: ≈101 conjectured and ≈66 proven bits.
   - Cheap improvement: `batch_proof_of_work_bits` 8–12, `with_lookup_proof_of_work_bits(8)` and `with_ood_proof_of_work_bits(8)` give about 109 conjectured bits at the same proof size.
   - If ~100 *proven* bits is the goal: lb2, q≈100–128, PoW 16 at every site (≈230–285 KB).
3. Build native nodes with AVX2 or AVX-512 enabled (3–4× prover speedup).
4. For browsers, benchmark a Keccak/Blake3 MMCS, and lb2 with lookup budget 5. Pass `log_blowup` from the preset into `setup`.
5. If proof bytes are ever used as identifiers, canonicalize them against the cap-length malleability.
6. Optionally, assert natively that x_in and halo values are within ±2^15 in `verify_inner`.
