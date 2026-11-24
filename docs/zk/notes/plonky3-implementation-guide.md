# Plonky3 implementation guide for umis

Status: 2026-11-03, repo `nu` @ `4c26c42`. This is a developer guide for the proof-system work in
`docs/zk/research/REPORT.md` §8 (phases 1–3). It covers the Plonky3 API as umis uses it, the
upgrade to 0.9, multi-AIR batches with an in-circuit Poseidon2 state commitment, recursion,
security settings, and a per-phase checklist.

**Path conventions**

| Abbreviation | Path |
|---|---|
| `REG/` | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/` |
| `p3-X-0.8.0/…`, `p3-X-0.9.0-rc.2/…` | crates under `REG/` |
| `P3M/` | Plonky3 main clone, `tmp/zk-research/src/p3/` (read-only, no `.git`) |
| `P3R/` | Plonky3-recursion clone, `tmp/zk-research/src/p3r/` (read-only, no `.git`) |

**What was compile-checked.** Unless a sketch says otherwise, it was built and run in a scratch
crate against **both `=0.8.0` and `=0.9.0-rc.2`** from crates.io, and the proofs verified. The
recursion sketches in §4 were **not** compiled: `P3R` isn't on crates.io in a usable version, and
building the clones was out of scope.

---

## 0. Key facts

1. **Upgrading to 0.9.0-rc.2 is just a version bump.** I copied the workspace to a scratch dir,
   changed every `=0.8.0` pin to `=0.9.0-rc.2`, and ran the tests:
   - every `umis-stark` and `task-water3d --features stark` test passes;
   - clippy is clean;
   - **`golden-proof-4.txt` still matches** (the proof bytes are identical);
   - `bench_air` n_loc=16 gives 261 KB, the same as 0.8.

   The batch-stark/FRI transcript and the `BatchProof` serde layout are unchanged. This
   corrects REPORT.md §6/§8, which predicted the golden vectors would need re-pinning.
2. **Grinding is not deterministic in 0.8.0 or rc.2** when rayon runs more than one thread. It is
   deterministic only on main (#2517, `find_map_first`).
   - Measured: with the `Benchmark` preset (16-bit query PoW), several runs give different
     proof bytes and sizes (66.3–68.4 KB on a toy AIR).
   - With `RAYON_NUM_THREADS=1` the output is stable.
   - With 0 PoW bits (the `Test` preset) the output is byte-stable on every version.
3. **Plonky3-recursion pins `0.9.0-rc.2` from crates.io** (`P3R/Cargo.toml`). Target rc.2 now (§2.5).
4. **Recursion constrains the leaf proofs in two ways.**
   - (a) Periodic columns become *circuit-build-time constants* (`P3R/recursion/src/verifier/periodic.rs:1-22`).
     umis' per-claim periodic data (X_in, X_out, halos) would therefore give a different verifier
     circuit for every slot. **Phase 2 must remove claim data from periodic columns before phase 3.**
   - (b) The in-circuit verifier rejects `batch_proof_of_work_bits ≠ 0`
     (`P3R/recursion/src/pcs/fri/params.rs:86`) and lookup or OOD PoW ≠ 0
     (`P3R/recursion/src/verifier/batch_stark.rs:1198`).
   - So the planned "add OOD/batch/lookup PoW" security fix conflicts with recursion. Get the
     security level from more queries or more blowup plus commit and query PoW instead (§5.4).
5. **A Poseidon2 commitment in the same batch works.** §3.4 has a sponge AIR that is compiled,
   proven and verified, built from `p3-poseidon2-air` with the *native* KoalaBear-16 constants
   and linked to a cell table over a global `LookupBus`. Its digest equals
   `PaddingFreeSponge<Poseidon2KoalaBear<16>,16,8,8>::hash_iter` (known-answer assert), and the
   verifier rejects a wrong digest. `p3-poseidon2-air` is deprecated on main (but not in rc.2),
   with no replacement named; `P3R` still depends on it.

---

## 1. The Plonky3 0.8 API as umis uses it

### 1.1 StarkConfig assembly (`crates/umis-stark/src/config.rs`)

```text
Val       = KoalaBear                         Challenge = BinomialExtensionField<Val,4>   (|EF| ≈ 2^124)
Perm      = Poseidon2KoalaBear<16>            default_koalabear_poseidon2_16()  (fixed constants)
Hash      = PaddingFreeSponge<Perm,16,8,8>    leaf hash, rate 8, 8-element digest
Compress  = TruncatedPermutation<Perm,2,8,16> 2-to-1 node compression
ValMmcs   = MerkleTreeMmcs<Packing,Packing,Hash,Compress,2,8>   (arity 2, cap height 0 at ::new(h,c,0))
ChallengeMmcs = ExtensionMmcs<Val,Challenge,ValMmcs>
Challenger= DuplexChallenger<Val,Perm,16,8>
Pcs       = TwoAdicFriPcs<Val,Radix2DitParallel<Val>,ValMmcs,ChallengeMmcs>
Config    = StarkConfig<Pcs,Challenge,Challenger>        (p3-uni-stark; batch-stark reuses it)
```

`StarkConfig::new(pcs, challenger)` sets the lookup and OOD PoW to 0
(`p3-uni-stark-0.9.0-rc.2/src/config.rs:120`). The knobs are
`.with_ood_proof_of_work_bits(b)` (`:139`) and `.with_lookup_proof_of_work_bits(b)` (`:153`). The FRI
knobs live in `FriParameters` (§5.1). This exact config is the one `P3R` supports in-circuit
(`P3R/test-utils/src/lib.rs:362-389`, `koala_bear_params`).

### 1.2 AIR traits (`p3-air`)

`BaseAir<F>` methods umis overrides (`air.rs:161-224`, `toy.rs:195-242`):

| Method | umis use | Notes |
|---|---|---|
| `width()` | 46 | main columns |
| `preprocessed_trace() -> Option<RowMajorMatrix<F>>` / `preprocessed_width()` | 32 columns: address, flags, coefficients, range tables | committed once in `ProverData` setup; **its height must equal the instance's trace height** |
| `main_next_row_columns()` / `preprocessed_next_row_columns()` | `vec![]` (row-local AIR) | the default is *all columns*. Declaring `[]` shrinks the openings and the batched-function count. On main, reading an undeclared next row is a typed error (#2585), and with nothing declared the prover folds zeros (#2581) |
| `num_periodic_columns()` / `periodic_columns() -> Cow<[Vec<F>]>` | 32 columns, period C | each length must be a power of two dividing the height. **Not absorbed into Fiat–Shamir** by batch-stark (0.8, rc.2 and main): bind them yourself (umis puts a keccak digest in the public values) |
| `num_public_values()` | 17 | absorbed as `Kind::Public` before any challenge |
| `max_constraint_degree()` | default | `Poseidon2Air` returns `Some(3)` |

rc.2 adds defaulted `periodic_periods`, `periodic_evaluations`, `periodic_statement` and
`boolean_columns` (`p3-air-0.9.0-rc.2/src/air.rs:33-136`). `periodic_statement` is bound only by
**p3-multi-stark**, not by batch-stark.

`Air<AB>::eval`: umis requires `AB: PermutationAirBuilder<F = Val> + InteractionBuilder` and reads:
- `builder.main().current_slice()` (`WindowAccess`);
- `builder.preprocessed().clone().current_slice()`;
- `builder.periodic_values()`;
- `builder.public_values()`.

All constraints are row-local and have degree ≤ 3 (`log_blowup = 1` gives a degree cap of
`2^lb + 1 = 3`).

### 1.3 p3-lookup buses (`p3-lookup-*/src/bus.rs`, `count.rs`)

```rust
let mem = LookupBus::new("w3d_mem");                       // global, keyed by NAME across all AIRs
mem.table_entry(b, [addr + C, qn0, qn1, qn2, qn3], p[MULT]);     // provide; pushes Count::provided(-m)
mem.lookup_key(b, [addr, q0, q1, q2, q3], Count::bounded(p[ACTIVE].into(), 1)); // query
byte.lookup_key(b, [lo], 1);                               // constant count: bound = |c| automatically
let bus = PermutationCheckBus::new("x");  bus.send(b, msg, cnt); bus.receive(b, msg, cnt); // multiset eq
```

- **One fraction column per interaction before packing.** `pack_same_bus_with_degree` merges
  interactions on the same bus while the fraction-pin degree stays within the budget. Column 0
  is the shared EF accumulator.
- **The verifier checks Σ terminals = 0 over *all* AIRs in the batch.** A bus name used in two
  AIRs therefore links them; a name used in one AIR only is a local check. Bus separation comes
  from `α + (bus+1)·β^W`, where W is the widest message on any bus. Tuple widths must be uniform
  within a bus.
- **Soundness obligations:**
  - a variable query count needs a bound that the AIR enforces (umis uses preprocessed booleans);
  - Σ weight·height < p is checked by both prover and verifier (`check_multiplicity_height_bound`);
  - table-side multiplicities may stay unconstrained.
- **Periodic columns are invisible to LogUp trace generation in 0.8 and rc.2.**
  `LookupTraceBuilder` has no periodic slice, so a bus key or count must never use a periodic
  value. Main fixes this (#2574: `generate_permutation` gains `periodic_columns`) and forbids
  `is_first_row`/`is_last_row` selectors inside lookup counts and fields. Use preprocessed
  flags, as umis already does.

### 1.4 Setup: ProverData / CommonData (`p3-batch-stark-*/src/common.rs`)

```rust
pub struct ProverData<SC> { pub common: CommonData<SC>, pub prover_only: ProverOnlyData<SC> }
pub struct CommonData<SC> { pub preprocessed: Option<GlobalPreprocessed<SC>>, pub lookups: Vec<Lookups<Val<SC>>> }
ProverData::from_airs_and_degrees(cfg, &airs, &trace_ext_degree_bits)                       // :188
ProverData::from_airs_and_degrees_with_lookup_budgets(cfg, &airs, &bits, &overrides, log_blowup) // :230
```

- `trace_ext_degree_bits[i] = log2(height_i) + cfg.is_zk()`. That is 0 without hiding; with
  `HidingFriPcs` add 1 (see `from_instances`, `:161-176`).
- `CommonData` is the verifying key: the global preprocessed commitment plus the packed lookups.
  umis rebuilds it from `(params, genesis seed)` (`air.rs:456-480`) and caches it with no eviction.
  - The verifier recomputes the lookups and never reads them from the proof.
  - The preprocessed commitment is both absorbed and opened.
  - `degree_bits` in the proof must equal the setup's.
- **Gotcha: `from_airs_and_degrees_with_lookup_budgets(.., &[3], 1)` at `air.rs:468-474` does
  nothing extra.**
  - With max degree 3, `log_chunks = 1`, so the free budget is `2^1 + 1 = 3`. The override 3
    equals the free budget, so the call behaves exactly like `from_airs_and_degrees`.
  - rc.2 documents that `0` "reproduces `from_airs_and_degrees` exactly".
  - The literal `1` is ignored unless an override exceeds the free budget. Then it is a hard
    `assert!` that `chunks ≤ 2^log_blowup`.
  - **Fix:** pass `preset.log_blowup()` and either `&[0]` or a deliberate budget. At lb = 2,
    budget 5 packs more interactions into each column: 9 EF auxiliary columns instead of 14,
    and 4 quotient chunks.

### 1.5 Prove / verify / serialize

```rust
let proof: BatchProof<Config> = prove_batch(&cfg, &[StarkInstance { air: &air, trace: &trace, public_values }], &pd)?;
verify_batch(&cfg, &airs /* &[A] */, &proof, &[public_values] /* one Vec per instance */, &pd.common)?;
let bytes = postcard::to_allocvec(&proof)?;           let proof: BatchProof<Config> = postcard::from_bytes(&bytes)?;
```

- **One AIR type per batch.** `StarkInstance<'a, SC, A>` has a single type `A`, so mix AIRs with
  an `enum` that dispatches every `BaseAir` method and `eval` (§3.1).
- **Different heights** per instance are fine.
- **Debug builds** run `check_constraints` and the bus-balance check before proving, and panic on
  a bad witness. The tamper tests rely on `catch_unwind` for this.
- **`BatchProof` fields** (`proof.rs`, identical in 0.8, rc.2 and main): `commitments`,
  `opened_values`, `opening_proof` (`FriProof`), `lookup_terminals`, `degree_bits`,
  `lookup_pow_witness`.
- **Proof bytes are not canonical:**
  - in 0.8.0 the Merkle cap could carry extra unauthenticated roots (fixed in rc.2 by #2378:
    `check_cap_size` → `MerkleTreeError::WrongCapSize`);
  - grinding witnesses vary between runs (fact 2).

  Never use `proof_address = bmt(proof bytes)` as a uniqueness key for a statement. Key pools by
  the statement, and treat the proof address only as a content address.

### 1.6 Other gotchas

- **The `parallel` feature.**
  - `p3-maybe-rayon/parallel` is enabled only off wasm (`umis-stark/Cargo.toml`). Rayon uses the
    *global* pool unless the caller installs a pool.
  - The grinding search is `find_any` in 0.8/rc.2, which makes it thread-count dependent.
  - Golden tests must use a 0-PoW preset, or run with `RAYON_NUM_THREADS=1`.
- **SIMD.** KoalaBear and Poseidon2 packing is compiled only under
  `target_feature = avx2 | avx512f | neon`. `.cargo/config.toml` sets none, so x86 runs scalar,
  about 4× slower.
  - Proofs are bit-identical with or without SIMD.
  - There is no wasm simd128 path for monty-31 in any version.
- **Field range.** Every AIR identity must not wrap mod p (2^31 − 2^24 + 1). umis needs
  `shift ≤ 14` (REPORT §3.3).
- **Preprocessed height = trace height**, for every instance, including a new hash AIR.
- **Public boundary IO.** batch-stark asserts `public_boundary_io().is_empty()`. Bind public
  values through ordinary gated constraints (`last·(out − pv) = 0`).

---

## 2. Migration 0.8.0 → 0.9.0-rc.2 → main

### 2.1 What changes in umis files (rc.2)

| File | Change |
|---|---|
| `crates/umis-stark/Cargo.toml` | every `"=0.8.0"` → `"=0.9.0-rc.2"`, including `p3-maybe-rayon` |
| `crates/task-water3d/Cargo.toml` | same |
| `config.rs`, `toy.rs`, `air.rs` | **no code change.** Verified: builds, clippy clean, all tests pass |
| `tests/golden-proof-4.txt` (water3d), toy wasm/golden | **unchanged.** Verified: the golden proof still matches |
| `toy.rs:39` doc comment | still true for rc.2. Update it only when moving to main (#2574) |

What rc.2 changed under the hood:
- the cap-size check (#2378);
- defaulted `BaseAir` methods;
- `Lookups::from_builder`, plus a weight-overflow panic (#2379);
- uni-stark `recompose_quotient_from_chunks` now returns `Result`;
- `TwoAdicFriPcs` needs `Dft: Sync`;
- the hiding PCS forks a `StdRng`.

None of these touches umis. The per-crate `CHANGELOG.md` files have no entries after 0.8.0
(`[Unreleased]` is empty), so all of the above comes from source diffs.

### 2.2 Transcript and proof format

| Item | 0.8.0 | rc.2 | main |
|---|---|---|---|
| batch-stark DomainSeparator | `VERSION=1`, `"p3-batch-stark"` | same (`transcript.rs:80,83`) | same |
| FRI / PCS separators | `"p3-fri"`, `"p3-fri-pcs"` v1 | same | same |
| Absorbed items | widths, degree bits, main commitment, public values, preprocessed commitment, LogUp terminals, quotient, ζ, FRI | same | same (comment and assert tweaks only) |
| Periodic data absorbed | no | no (`periodic_statement` exists, unused by batch-stark) | no (multi-stark only) |
| `BatchProof` / `FriProof` / `OpenedValues` serde | — | byte-identical `proof.rs` | byte-identical |
| Merkle cap | extra roots accepted | `check_cap_size`; caps on padded layers get default digests (bytes change only for cap > 0 with non-power-of-two heights; umis uses cap 0 and power-of-two heights) | same |
| Grinding | `find_any` (non-deterministic with threads) | same | `find_map_first` (deterministic, #2517) |
| PoW bound | panics in `sample_bits` above 30 bits | same | typed errors (`FriError::UnsupportedPowBits`, `UnsupportedLookupPowBits`, `UnsupportedOodPowBits`, #2580); KoalaBear max 30 |

`check_witness` is unchanged, so witnesses from main verify under rc.2 and the reverse.

### 2.3 rc.2 → main API diffs (they affect umis only on main)

- `ProvingError`, `VerificationError`, `BatchVerificationError`, `FriError`, `LookupError` and
  others are `#[non_exhaustive]` with new variants. umis only formats them (`{e:?}`), so it is
  unaffected; any exhaustive `match` would break.
- `Lookups::from_air`, `from_builder` and `from_interactions` return `Result<_, LookupError>`.
  umis does not call them.
- `LookupProtocol::generate_permutation(.., periodic_columns: &[Vec<F>])`. Lookups may read
  periodic columns; selectors are forbidden inside lookups.
- `p3_security::logup::LogUpAir` gains a `num_interactions_per_proof` field. Add it to struct
  literals.
- `ProvingError::EmptyBatch` and `ProvingError::UndeclaredNextRowRead` are new.
- `p3-poseidon2-air` (and poseidon1-air, monolith, rescue, bn254) are
  `#![deprecated(since="0.9.0")]` (`P3M/poseidon2-air/src/lib.rs:1-6`, `P3M/README.md:11-23`).

### 2.4 Re-pinning golden vectors (when a change does alter bytes)

1. Empty the pinned file: `: > crates/task-water3d/tests/golden-proof-4.txt`.
2. Run `cargo test --release -p task-water3d --features stark --test stark`. The test panics with
   `golden-proof-4.txt is empty; the proof address is <hex>` (`tests/stark.rs:12-14`). Paste that
   hex in.
3. Run the wasm suites (`tests/wasm.rs` reads the same file). They must match without edits:
   `cargo test --target wasm32-unknown-unknown -p task-water3d --features stark --test wasm`, and
   the same for `umis-stark` and `umis-core`.
4. Re-pin `golden-trace-8.txt` only if the simulation changed. The STARK never affects it.
5. Bytes change with any change to the AIR layout, preprocessed data, public values, FRI
   parameters, the hash or MMCS, or the lookup budget. They do **not** change with SIMD flags
   or a rc.2 bump (verified).

### 2.5 Recommendation: target `=0.9.0-rc.2` now

- Plonky3-recursion builds against crates.io `0.9.0-rc.2` (`P3R/Cargo.toml:49-83`, no git
  sources, no `[patch]`). Its types (`BatchProof`, `StarkGenericConfig`, AIR builders) must be
  the *same crate versions* as umis', so pinning a Plonky3 main commit would block recursion
  until `P3R` moves.
- rc.2 already has the cap-malleability fix and the LogUp weight-overflow fix. None of the
  post-rc.2 fixes is exploitable in umis' configuration (`plonky3-internals.md` §6).
- Missing in rc.2: deterministic grinding. Work around it by keeping 0-PoW test presets for
  golden tests, and by not treating proof bytes as unique.
- **Policy:** pin Plonky3 to whatever `P3R` pins. Pin `P3R` itself as a git dependency to a
  commit (its crates.io 0.1.0 is stale). Re-run the §2.4 golden check on each bump.

---

## 3. Multi-AIR batches and an in-circuit Poseidon2 state commitment

### 3.1 Two AIRs in one batch with a cross-AIR bus (compiled sketch)

```rust
#[derive(Clone)]
pub enum UmisAir { Stencil { height: usize }, Hasher { height: usize } }

impl<F: Field> BaseAir<F> for UmisAir {
    fn width(&self) -> usize { match self { Self::Stencil{..} => 2, Self::Hasher{..} => 5 } }
    fn preprocessed_width(&self) -> usize { 1 }
    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<F>> {
        let h = match *self { Self::Stencil{height} | Self::Hasher{height} => height };
        Some(RowMajorMatrix::new(vec![F::ONE; h], 1))           // preprocessed `out` flag
    }
    fn main_next_row_columns(&self) -> Vec<usize> { vec![] }
    fn preprocessed_next_row_columns(&self) -> Vec<usize> { vec![] }
    // FORWARD EVERY METHOD the inner AIRs override (periodic, public values, next-row lists, ...)
}
impl<AB: PermutationAirBuilder + InteractionBuilder> Air<AB> for UmisAir {
    fn eval(&self, b: &mut AB) {
        let main = b.main(); let m = main.current_slice();
        let prep = b.preprocessed().clone(); let p = prep.current_slice();
        let bus = PermutationCheckBus::new("state_out");
        match self {
            Self::Stencil{..} => bus.send(b, [m[0], m[1]], Count::bounded(p[0].into(), 1)),
            Self::Hasher{..} => {
                bus.receive(b, [m[0], m[1]], Count::bounded(p[0].into(), 1));
                bus.receive(b, [m[2], m[3]], Count::bounded(p[0].into(), 1));
                b.assert_eq(m[4], m[1] + m[3]);
            }
        }
    }
}
// heights 16 and 8 → degree bits [4, 3]
let airs = [UmisAir::Stencil{height:16}, UmisAir::Hasher{height:8}];
let pd = ProverData::from_airs_and_degrees(&cfg, &airs, &[4, 3])?;
let proof = prove_batch(&cfg, &[StarkInstance{air:&airs[0], trace:&st, public_values:vec![]},
                                 StarkInstance{air:&airs[1], trace:&ht, public_values:vec![]}], &pd)?;
verify_batch(&cfg, &airs, &proof, &[vec![], vec![]], &pd.common)?;      // ok; lookups per AIR = [1, 2]
```

Upstream reference: `p3-batch-stark-*/tests/simple.rs`. `enum DemoAirWithLookups` is at
`:543-588`; the tests are `test_batch_stark_global_lookups_only` (`:2780`) and
`run_batch_stark_mixed_lookups!` (`:3241`, 6 AIRs at heights 16 and 8).

What the instance shape implies:
- **The transcript binds the instance count and every width** (domain-separator instance label).
  Adding an AIR therefore changes the VK and all proof bytes.
- **The global preprocessed commitment covers all instances.** One commitment acts as the zone VK
  for both AIRs.
- **Each AIR gets its own lookup terminal**, and Σ = 0 is checked over the batch. Every global
  bus must balance *within one proof*. This also holds for recursion (`P3R/.../batch_stark.rs:1686`).

### 3.2 `p3-poseidon2-air` facts (0.8.0 = rc.2, `src/` byte-identical)

- **Types.**
  - `Poseidon2Air<F, LinearLayers, WIDTH, SBOX_DEGREE, SBOX_REGISTERS, HALF_FULL_ROUNDS, PARTIAL_ROUNDS>::new(RoundConstants)`.
  - `VectorizedPoseidon2Air<…, VECTOR_LEN>` puts VECTOR_LEN permutations side by side in each row.
  - The AIR implements `Air<AB: AirBuilder>`: constraints only, with no bus, selector or public
    values. Wrap it to add those.
- **KoalaBear parameters.**
  - `KOALABEAR_S_BOX_DEGREE = 3`, `KOALABEAR_POSEIDON2_HALF_FULL_ROUNDS = 4`, `PARTIAL_ROUNDS_16 = 20`,
    `PARTIAL_ROUNDS_24 = 23` (`p3-koala-bear-*/src/poseidon2.rs:37-61`, `poseidon1.rs:27`).
  - `SBOX_REGISTERS` must be 0 for degree 3: `(3,1)` panics in `sbox_constraint_degree`.
- **Columns per permutation row:** `W + 2·H·W·(REG+1) + P·(REG+1)`.
  - **Width 16: 164 columns**, 148 constraints, all degree 3. This fits `log_blowup = 1`.
  - **Width 24: 239 columns**, 215 constraints.
  - The inputs are columns `0..W`. The output state `ending_full_rounds[H-1].post` is the **last
    W columns**.
- **Constants matching the native permutation.** Do *not* use `RoundConstants::from_rng`, which
  is what the upstream examples do. Use:
  `RoundConstants::new(KOALABEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL, KOALABEAR_POSEIDON2_RC_16_INTERNAL, KOALABEAR_POSEIDON2_RC_16_EXTERNAL_FINAL)`.
  The `_24_` variants give width 24. `P3R/poseidon2-circuit-air/src/public_types.rs:219-226` does
  the same. The LinearLayers type is `GenericPoseidon2LinearLayersKoalaBear`.
- **Trace generation:** `generate_trace_rows::<F, LL, W, SD, 0, H, P>(inputs: Vec<[F;W]>, &rc, extra_capacity_bits)`.
  The input count must be a power of two; pad with zero inputs. There is no `is_real` column, so
  every row must be a valid permutation.
- **Deprecated on main**, with no replacement named. `P3R/poseidon2-circuit-air` *wraps* it
  rather than replacing it. Its additions (sponge and Merkle chaining, a `"WitnessChecks"` CTL
  bus keyed by recursion-witness indices, 165 main + 62 preprocessed columns) need rc.2 and its
  row format; use it as a design reference only. **Plan to vendor the AIR's ~600 lines** into
  `umis-stark` if a future Plonky3 release removes it.

### 3.3 Sponge or Merkle, and the rate and capacity

- **Native semantics to match** (`p3-symmetric/src/sponge.rs:193-227`, `PaddingFreeSponge::hash_iter`):
  1. Start from an all-zero state.
  2. **Overwrite** `state[0..RATE]` with each block, then permute.
  3. A trailing partial block keeps the stale tail. Avoid that by making lengths multiples of RATE.
  4. The digest is `state[..OUT]`.

  In the AIR this becomes:
  - first row: capacity inputs `= 0`;
  - chained rows: `next.in[RATE..W] = cur.out[RATE..W]`;
  - the rate inputs come from the bus;
  - the digest is `out[0..8]` of the last active row.
- **Value encoding.** i32 values become field elements through `Val::from_i64`, with negatives
  as `p − |x|`. The native verifier must use the same mapping. It is injective for
  |x| ≤ 2^15 ≪ p/2.
- **Options:**

| Choice | Capacity / security | Permutations for 16384 elements | Notes |
|---|---|---|---|
| W16, rate 8, capacity 8 (`PaddingFreeSponge<P16,16,8,8>`) | 8 × 31 bits, ≈124-bit collision resistance, same as the MMCS | 2048 | same permutation and constants as the PCS hash; `P3R` in-circuit tables are W16 |
| W24, rate 16, capacity 8 (`PaddingFreeSponge<P24,24,16,8>`) | same | 1024 | half the rows but 239 columns; `P3R`'s challenger and suites are fixed at W16 (W24 tables exist, `P3R/circuit/src/ops/perm.rs:285-321`) |
| Binary Merkle (`TruncatedPermutation<P16,2,8,16>`) | same | 4096 leaves + 4095 nodes (1 cell per leaf) | 3–4× more permutations, plus a self-bus or next-row wiring between levels. Only worth it if per-cell openings are needed. `MerkleTreeMmcs` hashes each row with `Hash::hash_iter` and compresses pairs, so a native verifier can rebuild it |

**Recommendation:** use a W16 sponge, one digest per object. That keeps one permutation family
everywhere: the PCS, `P3R`'s in-circuit tables, and the native verifier.

### 3.4 Compiled sketch: Poseidon2 sponge commitment linked by a bus

This was proven and verified on 0.8.0 and rc.2. A wrong public digest is rejected, and the
known-answer check against the native `Hash::hash_iter` passes. Here `Cells` stands in for
`WaterAir`'s memory-bus table side.

```rust
use p3_koala_bear::{GenericPoseidon2LinearLayersKoalaBear, KOALABEAR_POSEIDON2_HALF_FULL_ROUNDS as HF,
    KOALABEAR_POSEIDON2_PARTIAL_ROUNDS_16 as PR, KOALABEAR_POSEIDON2_RC_16_EXTERNAL_FINAL,
    KOALABEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL, KOALABEAR_POSEIDON2_RC_16_INTERNAL,
    KOALABEAR_S_BOX_DEGREE as SD, default_koalabear_poseidon2_16};
use p3_poseidon2_air::{Poseidon2Air, RoundConstants, generate_trace_rows, num_cols};
use p3_symmetric::CryptographicHasher;
use umis_stark::config::koala_bear::{Config, Hash, Val};

const W: usize = 16; const RATE: usize = 8;
const NCOLS: usize = num_cols::<W, SD, 0, HF, PR>();   // 164
const OUT: usize = NCOLS - W;                          // output state = last W columns
type P2Air = Poseidon2Air<Val, GenericPoseidon2LinearLayersKoalaBear, W, SD, 0, HF, PR>;

pub fn native_constants() -> RoundConstants<Val, W, HF, PR> {
    RoundConstants::new(KOALABEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL,
        KOALABEAR_POSEIDON2_RC_16_INTERNAL, KOALABEAR_POSEIDON2_RC_16_EXTERNAL_FINAL)
}

#[derive(Clone)]
pub enum ZoneAir {
    Cells { cells: usize },                                   // provides (addr, v0..v3)
    Sponge { inner: P2Air, cells: usize, height: usize },     // 2 cells (= 1 rate block) per row
}
// sponge preprocessed: addr of first cell, active, first, chain (next row continues), last
const S_ADDR: usize = 0; const S_ACTIVE: usize = 1; const S_FIRST: usize = 2;
const S_CHAIN: usize = 3; const S_LAST: usize = 4; const S_W: usize = 5;

impl BaseAir<Val> for ZoneAir {
    fn width(&self) -> usize { match self { Self::Cells{..} => 4, Self::Sponge{..} => NCOLS } }
    fn preprocessed_width(&self) -> usize { match self { Self::Cells{..} => 2, Self::Sponge{..} => S_W } }
    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<Val>> {
        Some(match *self {
            Self::Cells { cells } => RowMajorMatrix::new(
                (0..cells).flat_map(|i| [Val::from_usize(i), Val::ONE]).collect(), 2),
            Self::Sponge { cells, height, .. } => {
                let blocks = cells / 2;
                let mut v = Val::zero_vec(height * S_W);
                for (r, row) in v.chunks_exact_mut(S_W).enumerate() {
                    row[S_ADDR] = Val::from_usize(2 * r);
                    row[S_ACTIVE] = Val::from_bool(r < blocks);
                    row[S_FIRST] = Val::from_bool(r == 0);
                    row[S_CHAIN] = Val::from_bool(r + 1 < blocks);   // 0 on the last row: no wrap
                    row[S_LAST] = Val::from_bool(r + 1 == blocks);
                }
                RowMajorMatrix::new(v, S_W)
            }
        })
    }
    fn main_next_row_columns(&self) -> Vec<usize> {          // chaining reads next capacity inputs
        match self { Self::Cells{..} => vec![], Self::Sponge{..} => (RATE..W).collect() }
    }
    fn preprocessed_next_row_columns(&self) -> Vec<usize> { vec![] }
    fn num_public_values(&self) -> usize { match self { Self::Cells{..} => 0, Self::Sponge{..} => 8 } }
    fn max_constraint_degree(&self) -> Option<usize> { Some(3) }
}

impl<AB: PermutationAirBuilder<F = Val> + InteractionBuilder> Air<AB> for ZoneAir {
    fn eval(&self, b: &mut AB) {
        let bus = LookupBus::new("cells");
        let main = b.main(); let prep = b.preprocessed().clone(); let p = prep.current_slice();
        match self {
            Self::Cells { .. } => { let m = main.current_slice();
                bus.table_entry(b, [p[0], m[0], m[1], m[2], m[3]], p[1]); }
            Self::Sponge { inner, .. } => {
                inner.eval(b);   // UNGATED permutation constraints (deg 3); padding rows permute zeros
                let (m, nx) = (main.current_slice(), main.next_slice());
                let act = || Count::bounded(p[S_ACTIVE].into(), 1);
                bus.lookup_key(b, [p[S_ADDR].into(), m[0].into(), m[1].into(), m[2].into(), m[3].into()], act());
                bus.lookup_key(b, [p[S_ADDR] + AB::Expr::ONE, m[4].into(), m[5].into(), m[6].into(), m[7].into()], act());
                for k in RATE..W {
                    b.assert_zero(p[S_FIRST] * m[k]);                         // zero IV capacity
                    b.assert_zero(p[S_CHAIN] * (nx[k] - m[OUT + k]));          // carry capacity (deg 2)
                }
                let pv: Vec<AB::Expr> = b.public_values().iter().map(|&x| x.into()).collect();
                for k in 0..8 { b.assert_zero(p[S_LAST] * (m[OUT + k] - pv[k].clone())); }
            }
        }
    }
}

// Witness: run the native sponge to build each row's input = [block | carried capacity].
let flat: Vec<Val> = cells.iter().flat_map(|c| c.map(|x| Val::from_i64(x))).collect();
let digest: [Val; 8] = Hash::new(default_koalabear_poseidon2_16()).hash_iter(flat.iter().copied());
let (blocks, height) = (n_cells / 2, (n_cells / 2).next_power_of_two().max(4));
let perm = default_koalabear_poseidon2_16();
let (mut inputs, mut state) = (Vec::new(), [Val::ZERO; W]);
for r in 0..height {
    let mut inp = [Val::ZERO; W];
    if r < blocks {
        inp[..RATE].copy_from_slice(&flat[r * RATE..][..RATE]);
        inp[RATE..].copy_from_slice(&state[RATE..]);
        state = inp; p3_symmetric::Permutation::permute_mut(&perm, &mut state);
    }
    inputs.push(inp);
}
let rc = native_constants();
let st = generate_trace_rows::<Val, GenericPoseidon2LinearLayersKoalaBear, W, SD, 0, HF, PR>(inputs, &rc, 0);
assert_eq!(&st.values[(blocks - 1) * NCOLS + OUT..][..8], &digest[..]);    // known-answer test
let airs = [ZoneAir::Cells { cells: n_cells }, ZoneAir::Sponge { inner: P2Air::new(rc), cells: n_cells, height }];
let pd = ProverData::from_airs_and_degrees(&cfg, &airs, &[log2(n_cells), log2(height)])?;
// prove_batch(.. public_values: [vec![], digest.to_vec()] ..); verify_batch(.., &[vec![], digest.to_vec()], ..)
```

Rules for the real version:
- Keep the permutation constraints **ungated**. A selector multiplied in would raise them to
  degree 4.
- Gate only the I/O and chaining constraints, with preprocessed flags.
- Padding rows are permutations of zero inputs, and their bus counts are 0.
- The `S_CHAIN` flag must be 0 on the last row, because transitions wrap around.
- Declaring next-row columns is mandatory on main.
- For recursion, `P3R` expects the full-width next row as soon as any next-row column is used
  (`P3R/recursion/src/traits/air.rs:248-254`). This costs 164 extra opened values, which is
  minor. Alternatively, chain the capacity through a `PermutationCheckBus` keyed by row index and
  open no next row at all.

### 3.5 Wiring it into `WaterAir` (design for phase 2)

The aim is to replace the periodic columns and the keccak digest with Poseidon2 digests in the
public values. The cell data then never sits in periodic columns, which recursion requires.

1. **Inputs (X_in) are provided by the hash AIR instead of `in_mult`.** For each absorbed cell,
   the sponge AIR does `mem.table_entry([addr, q0..q3], m)`, where `m` is the preprocessed reader
   count that is now `pre::IN_MULT` (`air.rs:192`). Delete the `IN_MULT` table entry
   (`air.rs:263`) and the `first·(q − x_in)` constraints.
   - **Soundness:** every address is still provided exactly once, so every read equals the
     hashed value.
2. **Outputs (X_out) are read by the hash AIR.** Raise `pre::MULT` by 1 on the last-substep rows
   (today it is 0 there, `air.rs:191`), and have the sponge do `mem.lookup_key([C·k_sub + i, …], active)`.
   Delete the `last·(qn − x_out)` constraints.
3. **Halos.**
   - Give each neighbour direction a preprocessed address column `nb_addr_d`: `addr + off_d`
     inside the zone, `HALO_BASE + d·F + face_index` on the boundary.
   - Gate *every* neighbour read by `active` instead of `has_d`.
   - The halo sponge provides `(HALO_BASE + …, h0..h3)` with multiplicity `k_sub` (one read per
     substep).
   - This removes the 24 periodic halo columns and the `(1 − has)·(nb − halo)` constraints, and
     adds 6 preprocessed columns. The keys stay degree 1.
4. **Public values.** `tide`, then the 8-element digests of X_in, X_out, the input halos (one
   digest per port, to match producer outputs), and the output faces.
   - Hash each output face separately so a consumer's halo digest equals the producer's face
     digest, which makes cross-zone linking a plain equality.
   - The beacon tracks `p2_state_root` next to the keccak/Swarm root (REPORT decision #2).
5. **Delete `claim_data`'s keccak digest** (`air.rs:328-344`) and `N_PERIODIC`. The native
   verifier no longer needs states: `PublicData` shrinks to the digests plus `tide`.

**Cost for one slot at n_loc = 16 (4096 cells × 4 values), Poseidon2-16 sponge, rate 8:**

| Object | Elements | Permutations (rows) |
|---|---|---|
| X_in | 16384 | 2048 |
| X_out | 16384 | 2048 |
| 6 input halos | 6 × 1024 | 768 |
| 6 output faces | 6 × 1024 | 768 |
| **Total** | | **5632 → 8192 rows (2^13) × 164 columns** |

- **Main cells:** ≈1.34M, against the water AIR's 2^14 × 46 = 0.75M.
- **LogUp:** 2 interactions per row on `w3d_mem` pack into 1 fraction column + 1 accumulator,
  i.e. 8 base columns × 2^13.
- **Preprocessed:** about 5–8 columns.
- **Proving cost:** roughly **+60–90 %** (commit hashing and LDE scale with cells; quotient
  evaluation with 148 degree-3 constraints per row).
- **Width-24 variant:** 2816 permutations → 4096 rows × 239 columns ≈ 0.98M cells, but it needs a
  second permutation family and is not usable in `P3R` tables.
- **Multi-slot proofs** (§3.6) need X_in and X_out only at the ends. Per slot that leaves just the
  halos and faces (1536 permutations), so the overhead amortizes.

### 3.6 Multi-slot trace layout (one proof per commitment)

- **Height:** `n_z · k_sub · C` rows, padded to a power of two. Zone 0 (S_z = 4, k_sub = 4,
  C = 4096) has 2^16 rows; S_z = 16 has 2^18.
- **Addresses:** `addr = (j·k_sub + t)·C + i`. The memory bus already chains `addr + C` across
  substeps, so slot j's X_out is slot j+1's X_in **for free**. There is no extra I/O except at the
  ends.
- **Per-slot data:**
  - **tide:** add a main column `tide_row`, n_z public values `tide_j`, preprocessed slot
    selectors `sel_j`, and constraints `sel_j·(tide_row − tide_j) = 0` (degree 2). The stencil
    then uses `tide_row·F`, still degree 2. Don't use a periodic tide.
  - **halos:** one halo sponge per (slot, port), addressed `HALO_BASE + (j·6 + d)·F + face_idx`.
- **Preprocessed data:** the zone VK now depends on `n_z`. Recovery blocks with other ranges need
  either one setup per height (pad n_z to a power of two with inactive slots) or one proof per
  slot as today.
- **Range table:** the 2^s remainder table still fits, because height ≥ 2^s.
- **Statement:** `TransitionStatement` gains `to_slot = from_slot + n`. Remove the
  `to_slot == from_slot + 1` checks (`air.rs:529,563`; `beacon/state.rs:1050`) and make
  `leaf_count == to − from` (`state.rs:1003-1007`) one leaf per commitment.

---

## 4. Recursion with Plonky3-recursion (`P3R`)

### 4.1 Architecture

| Crate | Path | Role |
|---|---|---|
| `p3-circuit` | `P3R/circuit/src` | `CircuitBuilder<EF>` (`builder/circuit_builder.rs`): `public_input`, `alloc_private_inputs`, `add`/`mul`/`connect`, `set_statement_exports` (:748), `enable_poseidon2_perm`, `enable_recompose`. Non-primitive ops (Poseidon1/2 permutation, MMCS, recompose, Keccak/BLAKE3 for binary fields) in `ops/`; `SymbolicCompiler` turns AIR symbolic constraints into circuit ops; `statement.rs` (`StateTransitionLayout`) |
| `p3-circuit-prover` | `P3R/circuit-prover/src` | Proves a circuit as **one p3-batch-stark proof** over table AIRs (Const, Public, ALU, Poseidon, recompose) linked by a `"WitnessChecks"` LogUp bus. `BatchStarkProver` (`batch_stark_prover.rs:1776`), `prove_all_tables` (:1878), `verify_all_tables` (:1948), `BatchStarkProof<SC>` (:664), `TablePacking`, `ConstraintProfile` |
| `p3-recursion` | `P3R/recursion/src` | In-circuit verifiers (`verifier/{batch_stark,stark,periodic,quotient,limits}.rs`), PCS gadgets (`pcs/` FRI, WHIR, MMCS), `CircuitChallenger` (bit-identical to `DuplexChallenger`, W16/rate 8), backends `FriRecursionBackend<16,8>` / `WhirRecursionBackend`, high-level API in `recursion.rs` (`build_and_prove_next_layer` :465, `build_and_prove_aggregation_layer` :749, `*_cross` :685/:796), `prepared/` (`PreparedLayer`, `PreparedAggregation`, `TrustedPrepared*` :284/:509, `new_state_transition` :522), `artifact/`, `builtin_config/` (19 FRI + 2 WHIR suites) |

Book: `P3R/book/src/architecture_and_internals/{construction,circuit_building,lookups,hashing}.md`.
- The verifier is a fixed program with no PC, so there is **one circuit per proof shape**.
- Each W16/D4 duplexing costs about 70 extra primitive rows for recompose/decompose.

### 4.2 Verifying an umis batch-stark proof in-circuit

- **The config matches exactly.** umis' KoalaBear D4 + Poseidon2-16 + DuplexChallenger<16,8> +
  MMCS<2,8> + TwoAdicFriPcs is `P3R`'s `koala_bear_params` (`P3R/test-utils/src/lib.rs:362-389`),
  used with `Poseidon2Config::KOALA_BEAR_D4_W16` and `FriRecursionBackend::<16,8>`.
- **The high-level `RecursionInput` does not fit** a raw leaf:
  - `RecursionInput::BatchStark` takes only circuit-prover `BatchStarkProof`s (`recursion.rs:83-87`);
  - `RecursionInput::UniStark` rejects AIRs with lookups (`verifier/stark.rs:274-283`).
- **Use the low-level path for the first layer**, then the high-level API from layer 2 onwards.

Entry point: `verify_batch_circuit::<A,SC,Comm,InputProof,OpeningProof,LG,CP,16,8>(cfg, airs: &[A],
&mut CircuitBuilder, &BatchProofTargets, public_values: &[Vec<Target>], &PcsVerifierParams,
&CommonDataTargets, &LogUpGadget, CP)` (`P3R/recursion/src/verifier/batch_stark.rs:904`).
- `A: RecursiveAir` is blanket-implemented for any `Air<InteractionSymbolicBuilder<F,EF>>`, which
  `WaterAir` already is.
- Full flow, including proving the verifier circuit: `P3R/recursion/tests/zk_hiding_mmcs.rs:244-560`.
- Multiple AIRs with preprocessed columns: `tests/preprocessing.rs:319`.

Condensed, **not compiled**:

```rust
let mut cb = CircuitBuilder::<Challenge>::new();
cb.enable_poseidon2_perm::<KoalaBearD4Width16, _>(generate_poseidon2_trace::<Challenge, KoalaBearD4Width16>, perm);
cb.enable_recompose::<Val>(generate_recompose_trace::<Val, Challenge>);
let inputs = BatchStarkVerifierInputsBuilder::<Config, MerkleCapTargets<Val, 8>, InnerFri>
    ::allocate(&mut cb, &leaf_proof, &prover_data.common, &[N_PUBLIC])?;
let fri = FriVerifierParams::with_mmcs(log_blowup, log_final_poly_len, max_log_arity,
    commit_pow, query_pow, num_queries, Poseidon2Config::KOALA_BEAR_D4_W16);
let mmcs_ops = verify_batch_circuit::<_, _, _, _, _, _, _, 16, 8>(&cfg, &airs, &mut cb,
    &inputs.proof_targets, &inputs.air_public_targets, &fri, &inputs.common_data,
    &LogUpGadget::new(), Poseidon2Config::KOALA_BEAR_D4_W16)?;
// expose / pin: leaf public values (digests), preprocessed commitment (zone VK) via statement exports
let circuit = cb.build()?;  let mut runner = circuit.runner();
let (pubs, privs) = inputs.pack_values(&pvs, &leaf_proof, &prover_data.common);
runner.set_public_inputs(&pubs)?;  runner.set_private_inputs(&privs)?;
let (OpeningTranscript { mut challenger, commitments_with_opening_points }, _) =
    replay_batch_stark_transcript(&airs, &cfg, &leaf_proof, &pvs, &common, &lookup_gadget)?;
observe_opened_values::<Config>(&mut challenger, &commitments_with_opening_points, 0);
let paths = restore_fri_query_paths(&fri_params, &val_mmcs, &val_mmcs, &leaf_proof.opening_proof, &mut challenger, &claims)?;
set_fri_mmcs_private_data::<Val, Challenge, 8>(&mut runner, &mmcs_ops, &paths, Poseidon2Config::KOALA_BEAR_D4_W16)?;
let traces = runner.run()?;
// prove the verifier circuit: get_airs_and_degrees_with_prep → CircuitProverData::new(ProverData::from_airs_and_degrees(..))
// → BatchStarkProver::new(out_cfg).with_table_packing(p); register_poseidon2_table / register_recompose_table;
// prove_all_tables(&traces, &pd) → BatchStarkProof (feed into high-level API from here)
```

**Supported leaf-AIR features** (`batch_stark.rs`):
- preprocessed columns (:1131-1160), with mixed widths;
- public values;
- periodic columns, **as circuit constants** (:1641-1655);
- LogUp with several global and local buses, Σ terminals = 0 in-circuit (:1686, :1724-1737);
- several instances with different heights (:972-985);
- quotient chunks > 1 (:1113-1130);
- `HidingFriPcs` inner proofs;
- no hard constraint-degree cap.

**Leaf requirements:**
- **no lookup PoW and no OOD PoW** (:1197-1200);
- **no FRI batch PoW** (`pcs/fri/params.rs:86`);
- commit and query PoW are allowed;
- `VerifierLimits` default to 4096 instances and log domain ≤ 32.

### 4.3 2-to-1 aggregation and state transitions

```rust
// high-level (inputs are circuit-prover proofs, i.e. layer ≥ 2)
let backend = FriRecursionBackend::<16, 8>::new(Poseidon2Config::KOALA_BEAR_D4_W16).for_extension_degree::<4>();
let out = build_and_prove_aggregation_layer::<_, _, _, _, 4>(&left, &right, &config, &backend, &ProveNextLayerParams::default())?;
let next = out.into_recursion_input::<BatchOnly>();
// trusted, statement-checked chaining (book user_guide/aggregation.md):
let layout = StateTransitionLayout::base(/*state coeffs*/ 8, /*count bits*/ 16)?;
let owner = TrustedPreparedAggregation::new_state_transition(left_src, right_src, out_cfg, backend, params, layout)?;
let agg = owner.prove(left_input, right_input)?;   // exports [left.init, right.final, count_l + count_r]
```

- **Statement schema.** `new_state_transition` requires each child's statement to be exactly
  `[init_state…, final_state…, count]`. It connects `left.final == right.init`.
  - For umis: state = the 8-element Poseidon2 state digest, count = slots.
  - Halo/face digests and the zone id need a wider schema or a separate binding (see §4.5).
- **Heterogeneous children** (different AIRs or FRI shapes) are allowed:
  `tests/aggregation_different_shapes.rs`.
- **N-to-1** exists only in example code (`recursion/examples/common/n_to_one.rs`).
- **Verifying keys:** in the low-level path the inner preprocessed commitment, public values, lookup
  terminals and PoW witnesses are **public inputs of the outer circuit**. Nothing binds the
  VK unless the caller does. Options:
  - pin it with `Trusted*` owners, which keep the trusted AIR plus preprocessed commitment; or
  - export the VK and check it natively against the zone's genesis-derived VK, or against a
    Merkle root of all zone VKs.

  Untrusted `Prepared*` owners check only shapes.

### 4.4 PCS and parameters

- **FRI** (prime field) is fully supported.
- **WHIR** is supported for KoalaBear/BabyBear D4 Poseidon2 and custom D2/D4 configs; defaults
  are 66 bits per term, PoW budget 18, folding 4.
- **Binary PCS** recursion is incomplete.
- **Book recommendation** (`user_guide/configuration.md`): lb 3, arity 2^4, final poly 2^5,
  query PoW 16.
- **Benchmark defaults:** lb 2, arity 2 (3 for aggregation), final poly 5 (6), query PoW 15.
- **Intermediate layers** can run at reduced strength (query PoW 20 with 26 queries), with full
  strength only at the outermost layer. The `*_cross` APIs let the output config differ from the
  input config.

### 4.5 A umis aggregation pipeline

```
leaf (zone z, slots a..b)          batch-stark: WaterAir + Poseidon2 sponge AIR; pv = [tide.., d_in, d_out, halo/face digests]
   │  layer 1 (low-level verify_batch_circuit; one circuit per zone shape (n, k_sub, s, n_z))
   ▼
zone-commitment proof              exports [zone, VK-hash, d_in, d_out, face digests, input digests, slots]
   │  layer 2..k: TrustedPreparedAggregation::new_state_transition along a zone's slots (if >1 leaf)
   ▼
zone aggregate (per beacon slot)
   │  2-to-1 tree over zones (heterogeneous OK); at each node check: neighbour face digest == consumer halo digest
   ▼                                 (or just export the lists and let the beacon check equality natively)
beacon-slot aggregate (~1 proof)   verified by beacon validators against zones_root/proofs_root
```

- **Leaf constraint:** no periodic claim data (§3.5) and no lookup, OOD or batch PoW (§5.4).
- **VK:** per zone, from `(params, seed)`. Export a VK hash so the beacon can check it, or make
  the aggregator `Trusted`.
- **Roles:** browsers keep proving leaves; native nodes aggregate. Add a second deadline after
  `L` for aggregation (REPORT §8 phase 3).
- **Circuit-shape explosion:** one verifier circuit per distinct inner proof shape. Keep few leaf
  shapes: a fixed n_loc and n_z padded to a power of two. `P3R` suggests "proof lifting"
  (`advanced_topics/scaling.md`); multi-shape FRI verification is on its roadmap.

### 4.6 Limitations, TODOs, numbers

- **Maturity:** README says "under active development and hasn't been audited… do not
  recommend… production", and "field extensions are not fully parametrizable".
- **Roadmap** (`book/src/appendix/roadmap.md`): WIDTH/RATE fixed at 16/8 for 32-bit fields;
  multi-shape FRI verification not done; binary-field recursion incomplete.
- **Code TODOs:**
  - hiding batch-proving pool not re-validated under rayon (`circuit-prover/src/batch_stark_prover.rs:143`);
  - uni-stark verifier lacks lookups (`verifier/stark.rs:274-285`).
- **Soundness:** `soundness.md` warns that the `queries × log_blowup` heuristic is not a proven bound.
- **Benchmarks** (`book/src/appendix/benchmark.md`, Apple M4 Pro 14 cores, KoalaBear D4, 124-bit
  target). **No proof sizes are published.**

| Workload | Base | Layer 1 | Layers ≥ 2 |
|---|---|---|---|
| Keccak, 1000 hashes | 707 ms | 337 ms | 100 ms |
| Fibonacci, 10k | 39.2 ms | 61 ms | 100 ms |
| 2-to-1 aggregation | 13.9 ms | 85 ms | 174 ms |
| 8-to-1 aggregation | 13.9 ms | 229 ms | 641 ms |

- **Circuit size:** a single recursive verifier is about 43k public ops + 60k ALU ops, giving
  height 2^16 at packing (1,1) and 2^14 at (4,4). An aggregation circuit is about 2× a single
  verifier. Halving the height saves 40–50 % of FRI proving time.
- **Estimate for umis**, extrapolated from the table above (no measurement): roughly 0.1–0.3 s
  per 2-to-1 node on a desktop. A slot with about 11–44 leaves needs a tree of depth 4–6. That
  fits in 4 s natively with parallel nodes, but not in a browser.
- **Extra cost for umis leaves:** 100 queries over about 86 opened base columns plus 14 FRI
  rounds. Use fewer, wider leaf proofs (multi-slot), or lb 2–3 with fewer queries.

---

## 5. Security settings

### 5.1 `FriParameters` (`p3-fri-0.9.0-rc.2/src/config.rs:25-50`; the same struct in 0.8 and on main)

```rust
pub struct FriParameters<M> {
    pub log_blowup: usize,               // rate 2^-lb; AIR degree cap 2^lb + 1
    pub log_final_poly_len: usize,       // stop folding at 2^lfp coefficients
    pub max_log_arity: usize,            // fold 2^arity per round
    pub num_queries: usize,
    pub batch_proof_of_work_bits: usize, // before sampling the opening-batching α  (must be 0 for P3R)
    pub commit_proof_of_work_bits: usize,// per commit-phase round (paid every round)
    pub query_proof_of_work_bits: usize, // before query indices
    pub mmcs: M,
}
```

| Constructor | lb | lfp | arity | q | batch | commit | query |
|---|---|---|---|---|---|---|---|
| `new_testing(mmcs, lfp)` | 2 | given | 1 | 2 | 0 | 1 | 1 |
| `new_benchmark` (umis `Benchmark`) | 1 | 0 | 1 | 100 | 0 | 0 | 16 |
| `new_benchmark_high_arity` | 1 | 0 | 3 | 100 | 10 | 0 | 16 |
| `new_benchmark_zk` | 2 | 0 | 1 | 100 | 10 | 0 | 16 |

Helpers in rc.2: `security_regime() -> FriRegime` (`:104`), `grinding_sites()` (`:137`, batch PoW
only) and `conjectured_soundness_bits()` (`:93`).

**PoW sites**

| Site | Field | Where it's ground | Recursion |
|---|---|---|---|
| FRI query | `query_proof_of_work_bits` | once, before the query indices | allowed |
| FRI commit | `commit_proof_of_work_bits` | once per fold round (14 rounds at lb 1), so keep it ≤ 16 | allowed |
| FRI batch | `batch_proof_of_work_bits` | once, before the batching α | **rejected** |
| LogUp | `StarkConfig::with_lookup_proof_of_work_bits` | before α, β | **rejected** |
| OOD | `StarkConfig::with_ood_proof_of_work_bits` | before ζ | **rejected** |

The KoalaBear maximum is 30 bits per site. Above that, rc.2 panics and main returns an error.

### 5.2 The p3-security model (`p3-security-0.9.0-rc.2/src/`)

- **Regimes:**
  - `UniqueDecoding`;
  - `ListDecoding{m}`: the Johnson regime, DKT26 MCA bound, maximised over m;
  - `Conjectured`: the random-words conjecture, list size 1;
  - `Legacy`: ethSTARK.
- **"Proven"** = max(UDR, LDR).
- **Terms** (`report.rs:17-51`), the result being their minimum:
  - `air-composition`;
  - `deep-ali` (raised by OOD PoW);
  - `low-degree-test` (query + commit; reported separately as conjectured);
  - `batch-combination` (k − 1 over the batched functions; raised by batch PoW);
  - `logup-fingerprint` (N·(W+2)/|EF|; raised by lookup PoW);
  - `commitment-collision` (capped at 124 by the 8-element digest).
- **API:**
  - `proven_security_report(&FriRegime, &StarkAirParams, &InstanceShape, &extras, &GrindingSites) -> SecurityReport`
    (`stark.rs:239`), with `.security_bits()` and `.binding()` (`report.rs:429,435`);
  - `conjectured_security_report` (`:399`) and `legacy_security_report` (`:478`), same arguments;
  - `logup::security_term(&LogUpAir, &InstanceShape, &GrindingSites)` (`logup.rs:100`);
  - `p3_batch_stark::num_batched_openings(main_w, main_next, pre_w, pre_next, chunks, num_lookups, 4, OpeningShape::new())`
    (`p3-batch-stark-0.9.0-rc.2/src/security.rs:29`), which counts what the batching term needs.
- **There is no helper that derives the shape from `ProverData`/`CommonData`.** The uni-stark
  `StarkSecurityParams::from_air` ignores LogUp. Build the shape by hand, and test that it
  matches the real AIR (see the guard test below).
- `tmp/zk-research/secprobe/src/main.rs` (written for 0.8) **compiles unchanged on rc.2 and gives
  the same numbers.** On main, add `num_interactions_per_proof: 0` to the `LogUpAir` literals.

### 5.3 CI test sketch (compiled against rc.2)

Put this in `crates/umis-stark/tests/security.rs`, or in `task-water3d/tests/` next to the AIR,
with dev-dependencies `p3-security`, `p3-uni-stark` and `p3-batch-stark`.

```rust
use p3_security::fri::FriRegime;
use p3_security::logup::{self, LogUpAir};
use p3_security::report::SecurityTerm;
use p3_security::stark::{conjectured_security_report, proven_security_report};
use p3_security::{GrindingSites, InstanceShape, StarkAirParams};

pub struct AirShape {
    pub log_rows: usize, pub main: usize, pub pre: usize, pub base_constraints: usize,
    pub lookup_cols: usize,   // packed LogUp fraction columns (excl. accumulator)
    pub interactions: usize,  // unpacked bus interactions
    pub max_msg_width: usize, pub max_degree: usize, pub quotient_chunks: usize,
}

pub fn bits(fri: &FriRegime, g: &GrindingSites, a: &AirShape) -> (f64, f64) {
    let air = StarkAirParams { num_constraints: a.base_constraints + a.lookup_cols + 3,
        max_constraint_degree: a.max_degree, num_quotient_chunks: a.quotient_chunks, max_combo: 2 };
    let batched = p3_batch_stark::num_batched_openings(a.main, false, a.pre, false,
        a.quotient_chunks, a.lookup_cols, 4, p3_uni_stark::OpeningShape::new());
    let shape = InstanceShape { log_trace_length: a.log_rows, modulus_bits: 124,
        collision_resistance: 124, num_batched_functions: batched };
    let lk = LogUpAir { num_interactions: a.interactions, max_message_width: a.max_msg_width };
    let extras: Vec<SecurityTerm> = logup::security_term(&lk, &shape, g).into_iter().collect();
    (proven_security_report(fri, &air, &shape, &extras, g).security_bits(),
     conjectured_security_report(fri, &air, &shape, &extras, g).security_bits())
}

#[test]
fn network_preset_meets_target() {
    // derive from the real preset: FriPreset::Network.fri_params(()) → .security_regime(); site PoW from StarkConfig
    let fri = FriRegime { log_blowup: 2, num_queries: 128, log_final_poly_len: 3, max_log_arity: 2,
                          commit_pow_bits: 16, query_pow_bits: 16 };
    let g = GrindingSites { out_of_domain: 0, batch_combination: 0, lookup_challenge: 0 }; // recursion-compatible
    // water3d at lb2 / lookup budget 5: 4 deg-1 interactions per column → 3+2+3 = 8 fraction cols, degree 5, 4 chunks
    let w3d = AirShape { log_rows: 14, main: 46, pre: 32, base_constraints: 40, lookup_cols: 8,
        interactions: 23, max_msg_width: 5, max_degree: 5, quotient_chunks: 4 };
    let (proven, _conj) = bits(&fri, &g, &w3d);
    assert!(proven >= 100.0, "proven security {proven:.1} < 100");
}
```

Two guard tests should go with it, so the hand-written shape can't silently drift:
- assert `WaterAir`'s width, preprocessed width, constraint count and lookup count against the
  constants. The packed lookup count is `pd.common.lookups[0].len()`, and the constraint count
  comes from `p3_air::symbolic` (`get_symbolic_constraints`). Also assert the preset fields
  equal the `FriRegime` the test uses;
- assert `koalabear-test` stays under 10 bits, so nobody ships it by mistake.

### 5.4 Measured options (rc.2 calculator, water3d shape)

All rows use arity and final-poly settings as listed. The lookup budget is `2^lb + 1` (capped at
9). "No site PoW" means OOD = batch = lookup = 0, which is what recursion requires.

| Rows | FRI | Proven (no site PoW) | Conjectured (no site PoW) | Proven (site PoW 16) | Conjectured (site PoW 16) |
|---|---|---|---|---|---|
| 2^14 | lb1 q100 qpow16 (current `Benchmark`) | 65.9 | 101.4 | 65.9 | 109.0 |
| 2^14 | lb2 q100 a2 lfp3 cpow16 qpow16 | 91.8 | 100.6 | 104.5 | 116.6 |
| 2^14 | **lb2 q128 a2 lfp3 cpow16 qpow16** | **100.6** | 100.6 | 107.8 | 116.6 |
| 2^14 | lb3 q64 a2 lfp3 cpow16 qpow16 | 89.9 | 99.7 | 103.2 | 115.7 |
| 2^14 | lb4 q50 a2 lfp3 cpow16 qpow16 | 87.9 | 98.7 | 103.9 | 114.7 |
| 2^16 | lb2 q128 a2 lfp3 cpow16 qpow16 | 98.6 | 98.6 | 105.8 | 114.6 |

With no site PoW, the batching and LogUp terms cap security at about 100 bits; only more
queries lift the proven Johnson term up to that cap. So:
- **≥ 100 proven bits that stay recursion-compatible:** lb2, about 128 queries, commit and query
  PoW 16, at 2^14 rows. The estimated proof is about 285 KB.
- **Taller multi-slot traces (2^16 and up)** lose about 1 bit per doubling, so use about 136–140
  queries.
- **Leaves that are never recursed** (phase 1 and 2) can add 16-bit site PoW for about 105–108
  proven bits.
- **Beyond about 100 bits** you need a smaller LogUp error (fewer interactions or rows) or
  KoalaBear⁵ (REPORT decision #1).

---

## 6. Per-phase checklists

### Phase 1: hygiene (days, no protocol change)

- [ ] **Build flags.** In `.cargo/config.toml`, add
  `[target.x86_64-unknown-linux-gnu] rustflags = ["-C", "target-feature=+avx2"]`, or ship
  avx2/avx512 builds. Run `scripts/check.sh`; the golden vectors must not change.
- [ ] **Version bump.** Move every crate in `crates/umis-stark/Cargo.toml` and
  `crates/task-water3d/Cargo.toml` from `=0.8.0` to `=0.9.0-rc.2`. No code change and no golden
  change (§2.1). Run the wasm tests.
- [ ] **`crates/umis-stark/src/config.rs`:**
  - fix the `Benchmark` doc to say ≈101 conjectured and ≈66 proven bits;
  - add a `FriPreset::Network` (e.g. lb2, q128, arity 2, lfp 3, cpow 16, qpow 16, no batch PoW);
  - add `pub const fn log_blowup(self)`;
  - make `config()` take an optional `StarkConfig` site-PoW (keep it 0 if recursion is planned);
  - update the "proofs are non-deterministic" comment: true with PoW > 0 and multiple threads.
- [ ] **`crates/task-water3d/src/air.rs:468-474`.** Replace `&[3], 1` with `&[budget], preset.log_blowup()`,
  where `budget = (1 << lb) + 1` capped at 9, or `&[0]`.
  - Add `"koalabear-100"` (or similar) to `from_config_id` (`:444-450`).
  - Changing the budget changes the VK and proof bytes, so re-pin the golden vectors (§2.4).
- [ ] **AIR guards:**
  - in `task-water3d/src/lib.rs:208`, change `8..=16` to `8..=14`;
  - in `verify_inner` (`air.rs:550`), check |v| ≤ 2^15 for every X_in and halo value;
  - in `halos()` (`air.rs:294-299`), return an error for a wrong-length halo instead of zero-filling.
- [ ] **Genesis** (`crates/umis-core/src/genesis.rs:196`). Reject unknown `proof_backend`, reject
  `stark_config_id == "koalabear-test"` outside dev genesis, and validate `L`.
- [ ] **CI security test** (§5.3) plus the shape guard test.
- [ ] **DoS and protocol** (REPORT §4.2, `protocol-proof-flow.md` §8):
  - `umis-protocol/src/zone/engine.rs:691` (`None => true`): stop pooling unverified shares;
  - `node/mod.rs:511` `on_proof_commit`: check membership and leaves;
  - allow several candidates per key;
  - memoize failed verifications (`ctx.rs:63-70`);
  - move verification off the event loop;
  - stop gossiping proofs on `/out`.
- [ ] **`crates/umis-web/src/prover.rs:24-27`.** Stop passing the secret key to workers.
- [ ] **SimNet.** Add the `stark` scenario to `tests/scenarios.rs` with per-node contexts and
  adversarial proofs (junk share, junk commitment, mauled proof).

### Phase 2: proof shape (weeks)

- [ ] **Poseidon2 AIR crate wiring.** Add `p3-poseidon2-air = "=0.9.0-rc.2"` to umis-stark. Add
  `umis-stark/src/p2.rs` with `native_constants()`, a sponge-AIR wrapper (§3.4), trace
  generation, and the known-answer test against `Hash::hash_iter`.
- [ ] **`task-water3d/src/air.rs`:**
  - [ ] `enum ZoneAir { Water(WaterAir), Sponge(P2Sponge) }` that forwards *every* `BaseAir`
    method (§3.1).
  - [ ] Remove `N_PERIODIC`, `P_XIN`, `P_XOUT`, `P_HALO` and the periodic columns.
  - [ ] Add per-direction `nb_addr_d` preprocessed columns, gate neighbour reads by `active`,
    and move halo values onto the bus (§3.5 step 3).
  - [ ] `IN_MULT` table entries move to the sponge AIR. `MULT` gets +1 on last-substep rows.
  - [ ] `claim_data` returns the public values `[tide…, d_in(8), d_out(8), halo_d(6×8), face_d(6×8)]`.
    Delete the keccak digest.
  - [ ] `setup()`: two AIRs, degree bits `[log_h_water, log_h_sponge]`, budget/log_blowup from
    the preset.
  - [ ] `prove_with` / `verify_inner`:
    - one `StarkInstance` per AIR;
    - the verifier checks the statement's Poseidon2 roots against the public values and no
      longer needs states;
    - keep the keccak/Swarm `check_roots` only where the data layer still needs it, or move it
      out of the STARK backend.
- [ ] **Multi-slot** (§3.6):
  - `Shape` gets an `n_slots` field;
  - `addr = (j·k_sub + t)·C + i`;
  - add a `tide_row` column with slot selectors and per-slot halo sponges;
  - remove `to_slot == from_slot + 1` at `air.rs:529,563` and `beacon/state.rs:1050`;
  - `state.rs:1003-1007` becomes one leaf per commitment;
  - `engine.rs:593-703` and `duties.rs:293-374` change to per-commitment jobs;
  - `ProveJob` (`umis-proof/src/lib.rs:64-75`) carries a slot range plus per-slot inputs.
- [ ] **`umis-proof/src/lib.rs`:**
  - `PublicData` becomes digests (no states);
  - `LeafData` (`:56-60`) is removed or reduced to input refs;
  - `proof_leaf`/`ProofLeaf` gain the Poseidon2 roots (`umis-core/src/records.rs:49,91,160`, with
    layout version bumps);
  - the trait gets a multi-slot statement.
- [ ] **Beacon and zone records:** add `p2_state_root` next to `state_root`. Size limits in
  `umis-net` can shrink.
- [ ] **Lookup slimming:** replace the `qn + 2^15` byte limbs with one 14-bit limb against `ADDR`
  plus 2 boolean bits. That drops `T_BYTE` and 4 interactions.
- [ ] **Security and golden:** re-run the §5.3 test with the new shape (sponge AIR: 164 main
  columns, ≈164 + 8 constraints, 2 interactions; batched functions sum over both AIRs), then
  re-pin all golden vectors.
- [ ] **Browser:** a wasm simd128 KoalaBear packing prototype in a fork of `p3-monty-31`, plus
  `wasm-bindgen-rayon`.

### Phase 3: aggregation (1–3 months)

- [ ] **Prerequisites:** phase 2 is done, so there are no periodic claim columns. The leaf preset
  has `batch_proof_of_work_bits = 0` and no lookup or OOD PoW. Plonky3 is on the exact version
  `P3R` pins.
- [ ] **New crate `crates/umis-agg`** (native only, no wasm):
  - git dependency on `Plonky3-recursion` pinned to a commit, with `features = ["parallel"]`;
  - **layer 1:** `verify_batch_circuit` over `ZoneAir` (§4.2). Export
    `[zone, vk_hash, d_in, d_out, face/halo digests, slot range]` with `set_statement_exports`.
    Cache one prepared circuit per zone shape;
  - **layer ≥ 2:** `TrustedPreparedAggregation`. Use `new_state_transition` for the slots of one
    zone, and plain 2-to-1 across zones. Pin VKs (trusted sources, or a VK-root check);
  - **config:** inner layers with reduced queries and higher PoW; the outer layer at the network
    target (§4.4). Assert it with the §5.3 test applied to the outer circuit's shape.
- [ ] **Protocol:**
  - a new record type `AggregateProof { slot, zones_root, proof }`;
  - an aggregator role and an aggregation deadline after `L` (`duties.rs:622-667`);
  - `apply_proof_commitment` (`beacon/state.rs:987-1076`) verifies one aggregate per beacon slot
    and checks halo/face digest equality across neighbours (natively, or inside the top circuit);
  - wire structs `types.rs:90-134,246-250`;
  - executors stay leaf-only in the browser.
- [ ] **Measure:** layer-1 circuit height for one umis leaf, plus aggregate prove time and size.
  `P3R` publishes no proof sizes. Revisit D, member re-execution and finality.
- [ ] **Risk register:** `P3R` is unaudited, the API is moving, and there is one circuit per leaf
  shape. Keep a non-recursive fallback mode (per-commitment leaf proofs verified directly by the
  beacon) behind a genesis flag.

---

## 7. Open blockers and decisions

1. **Recursion and site PoW conflict.** If phase 3 is planned, do not ship lookup, OOD or batch
   PoW in the network preset. Use lb2 with about 128 queries (≈285 KB leaf) for ≥ 100 proven bits.
2. **Periodic claim data cannot be recursed efficiently.** Every slot would need its own circuit,
   with about 32 × 4096 constant-coefficient Horner steps. Phase 2's bus-based I/O redesign is a
   hard prerequisite.
3. **`p3-poseidon2-air` is deprecated upstream (main).** It is fine on rc.2; plan to vendor it.
4. **Deterministic grinding exists only on main**, while `P3R` pins rc.2. Proof bytes stay
   non-unique, so key pools by statement.
5. **`P3R` is unaudited and its git commit can't be identified** (the clone has no `.git`). Choose
   and pin an upstream commit before phase 3.
6. **No published recursion proof sizes.** Measure before committing to beacon size budgets.
