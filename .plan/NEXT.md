# Next steps in detail (written 2026-11-22 at commit `c4228be`, for continuing after a compaction)

Read `.plan/HANDOFF.md` first (how to build, test, machine quirks, conventions). This file is the work plan.
`.plan/ROADMAP.md` §0 holds the goals (N1–N7) and status; `docs/stencil.md` is the user-facing guide of what
exists.

## Where we are

- `crates/umis-stencil`: generic local-rule AIR framework.
  - `Rule` trait; uni-STARK `StencilAir` (in-trace sponges, bit range checks);
  - batch leaf `StencilBatch` (cells, a Poseidon2 hash table and a range table over LogUp);
  - toys `life`, `heat`, `lbm` (D2Q9).
  - Spec `docs/zk/stencil-spec.md`; 34 tests, all forgeries mutation-checked.
- `crates/task-stencil`: protocol zone task for any rule, plus the proof backend `StencilStark` (P100, batch leaf).
  Wired into `umis-tasks`, the CLI (`genesis --task stencil --rule …`, `sim --frames`), SimNet (6 scenarios)
  and the devnet. `examples/world.rs` proves and verifies every zone slot.
- Measured per zone slot (P100, CPU): life 64² 18 ms, heat 64² 90 ms, lbm 32² 358 ms prove / 159 ms verify /
  5.9 MB. Devnet with 4 native nodes: LBM 32² needs `--slot-ticks 16`; life 64² runs at 8. Both had 0
  rollbacks.
- Not done:
  - recursion of batch leaves (S5b);
  - GPU for batch leaves;
  - smaller fixed costs for small zones;
  - stateless beacon (S6);
  - Gray–Scott (toy 4).

Owner rules that apply to all of it:
- native only (N1);
- no toy-specific optimizations (life, water and heat are toys);
- ≥ 100 proven bits computed with p3-security;
- release-mode forgery tests, mutation-checked;
- commit after each step with the attribution lines;
- don't stop at every decision. Pick the sensible option, record it, and keep going.

## Order

1. **S5b: recursion of batch leaves** (P3R patch). The biggest lever: it shrinks the 50 MB blocks and unlocks
   aggregation for every rule. About 2 sessions.
2. **S4: generic aggregation backend** over S5b (`StencilAggBackend`), SimNet `stencil-agg`. About 1 session.
3. **Speed** (independent of 1–2; can be done first if recursion stalls). About 1–2 sessions.
4. **Gray–Scott** as toy 4. About 0.5 session.
5. **S6: stateless beacon.** About 2 sessions.

---

## 1. S5b: a trusted native-batch source in Plonky3-recursion

**Problem** (ROADMAP S5b findings): P3R's trusted owners (`TrustedPreparedLayer`) accept a uni-STARK leaf without
lookups, or a P3R circuit batch proof. Our leaf is a native `p3_batch_stark::BatchProof` with three AIRs and
LogUp buses. The low-level `verify_batch_circuit` verifies it in-circuit, but on its own it makes every
leaf-proof value a public input and does not pin the leaf's VK.

**How the trusted path works** (`recursion/src/prepared/trusted.rs:284-370`, P3R rev `3a69ba6`, checkout under
`~/.cargo/git/checkouts/plonky3-recursion-*/3a69ba6`):

1. `preflight_input` / `preflight_trusted_batch` validate the representative proof.
2. A `TrustedChildStatementLayout` is built: `uni(n_public, schema)` for uni-STARK.
3. `TrustedConstruction::new(source)` makes `TrustedChildAuthority::{Uni, Batch}` and the input.
4. `capture_trusted_input_contract` records the shape contract.
5. `backend.prepare_circuit`, then `build_verifier_circuit` (uni) or `build_trusted_batch_verifier_circuit`.
6. `backend.constrain_trusted_preprocessing(&mut builder, &result, authority.expected_preprocessed())` **pins the
   VK** (the preprocessed commitment) as constants.
7. `backend.verified_statement_targets(&result, &layout, &builder)?.install(..)` **makes only the statement
   public**.
8. `prepare_prover_with_statement(&circuit, .., layout.schema())`; later `prove` packs the inputs
   (`recursion_input`), sets private data and runs.

**The patch** (a new variant in each of these places, mirroring `Uni`):

- `TrustedPreparedSource::NativeBatch { config, airs: &'air [A], common: &'p CommonData<SC>, proof: &'p
  BatchProof<SC>, public_values: &'p [Vec<Val<SC>>] }`, and the same in `TrustedPreparedInput` (without `config`
  and `airs`).
- `TrustedChildAuthority::NativeBatch { config, airs, common }`; `expected_preprocessed()` returns
  `common.preprocessed.commitment`.
- Statement layout: `uni(total_publics, Base schema)` over the concatenated public values (for us
  `[d_prev, d_halo, d_next]`, 24).
- Backend (`src/backend/fri.rs`, the D5 `impl PcsRecursionBackend<SC, A, 5>` at about `:1946` and the macros at
  `:2192` / `:2280`):
  - `build_native_batch_verifier_circuit`: `BatchStarkVerifierInputsBuilder::allocate(circuit, proof, common,
    &public_counts)` + `verify_batch_circuit::<A, SC, SC::Commitment, SC::InputProof, SC::OpeningProof, _, _,
    WIDTH, RATE>(config, airs, circuit, &targets, &public_targets, config.pcs_verifier_params(),
    &common_targets, &LogUpGadget::new(), backend.challenger_perm_config)`.
  - A new `FriVerifierResult::NativeBatch(inputs, op_ids, limits)` variant. Its `op_ids`, the preprocessing
    constraint (the common-data commitment targets = constants) and `verified_statement_targets` (the AIR public
    targets) follow the `UniStark` variant.
  - Private data: `replay_batch_stark_transcript(airs, config, proof, publics, common, &LogUpGadget::new())`, then
    `SC::set_fri_private_data(config, runner, op_ids, &proof.opening_proof, transcript)` (see
    `tests/preprocessing.rs:501-545`).
  - Packing: `verifier_inputs.pack_values(publics, proof, common)` → (public, private). With the statement
    installed, only the statement stays public. **Verify this** by checking that `CircuitVerifier::verify(proof,
    &statement24)` works.
- Note: the in-circuit verifier rejects OOD, batch and lookup PoW. Leaves must be proved with the EF5 config
  (`umis_agg::AggConfig`, J101), not P100. `StencilBatch` is hard-wired to `koala_bear::Config`: generalise it
  over the config first (it only needs `airs()` and `degrees()` exposed, plus a `ProverData` built for any `SC`;
  `generate`/`assemble` do not depend on the config).

**Delivery mechanics** (no unapproved dependency changes: the owner approved a local P3R patch on 2026-11-22):
- `scripts/vendor-p3r.sh`: copies the pinned checkout (or clones the rev) into `vendor/plonky3-recursion`
  (gitignored, about 14 MB) and applies `vendor/p3r-native-batch.patch` (committed).
- Workspace `Cargo.toml`: `[patch."https://github.com/Plonky3/Plonky3-recursion"]` for `p3-recursion`,
  `p3-circuit`, `p3-circuit-prover` and `p3-poseidon2-circuit-air`, pointing at `vendor/plonky3-recursion/<crate>`.
- `scripts/check.sh`: run `vendor-p3r.sh` if the directory is missing.
- Document it in `vendor/README.md` (what is patched and why) and in `docs/zk/aggregation.md`.

**Tests** (in `umis-agg`, release verifier, each mutation-checked):
1. A heat batch leaf (EF5, TEST params) recurses through the new owner. The layer-1 proof verifies against the
   24 digests; another statement is rejected.
2. A leaf proved for a different AIR (another rule, same widths if possible, or the same rule with other params)
   presented to an owner built for heat does not recurse (VK pin).
3. One owner proves many slots (a reused owner, as `one_owner_per_shape_proves_many_slots`).
4. An aggregate of 2–4 consecutive slots: `chain()` over the statements (`[d_prev, d_halo, d_next]` per leaf;
   adapt `umis_agg::chain` to 24-element statements).
5. Security: J101 ≥ 101 bits for the recursive layers (`recursive_shape`) and for the EF5 leaf
   (`StencilBatch::security_shape` generalised to EF5).

**Fallback if the patch turns out too invasive:** keep uni-STARK leaves (`StencilAir`, bits plus in-trace sponges)
for aggregation, through the existing trusted uni owner (works today, about 5× costlier leaves), and record why.

## 2. S4: generic aggregation backend

- `umis_agg::backend::StencilAggBackend` generalises `LifeAggBackend`:
  - same group format (`ProofTag::AggHead`/`AggRef` → move the tags to `task-stencil` or a shared place);
  - leaves = stencil batch leaves under the EF5 config;
  - owner tree per (shape, member count);
  - the verifier derives each member's `[d_prev, d_halo, d_next]` from its native claim
    (`StencilStark::native` → expose it).
- Genesis `stark_config_id = koalabear-agg[-test]` → `StencilAggBackend` for `stencil2d@1` (`umis-tasks`).
- SimNet `stencil-agg`, `stencil-agg-junk`; backend tests with forged groups (copy `umis-agg/tests/backend.rs`).
- Measure: block bytes per beacon slot before and after (target: one aggregate per commitment instead of one
  ~6 MB leaf per slot).

## 3. Speed (generic)

In order of payoff per effort:

1. **Batch several slots per proof** (`prove_batch` for stencil: a commitment's advancing slots as several cells
   instances plus one shared hash table and one shared range table). This amortises the fixed range table (2^16
   or 2^17 rows) that dominates small zones (a 16² LBM zone proves almost as slowly as a 32² one). Life2d's
   batch format (`group_head`/`group_ref`) can be reused.
2. **Parallel `assemble`** (`umis-stencil/src/batch.rs`): rows are independent except the hash blocks
   (collect per row, then concatenate in order). It is 73 ms at 256² life.
3. **Byte range table plus splits** for zones too small to amortise the table: requests above 8 bits split into
   bytes with helper columns. Make it the third `RangeTable` form and choose it by the same cost rule.
4. **GPU batch leaf** (the owner allowed the RTX 3090 Ti, GPU 1; the A6000 is someone else's). `umis-gpu` is
   single-table (uni-STARK, Keccak config, JIT constraints). Extending it to batch STARKs with LogUp is large;
   first measure where the CPU time goes (`UMIS_GPU_PROFILE`-style laps in the batch prover) to see whether the
   LDE/commit part dominates.
5. Prover threads: nodes use 2 (`UMIS_PROVER_THREADS`); document the capacity rule in `docs/stencil.md` (done) and
   consider a per-genesis recommendation.

## 4. Gray–Scott (toy 4)

`umis-stencil/src/grayscott.rs`:
- two 16-bit values `u, v` per cell, radius 1 (5-point Laplacian);
- `u' = u + Du·Δu − u·v² + F(1 − u)`, `v' = v + Dv·Δv + u·v² − (F + k)·v`, in fixed point;
- staged divisions as in `lbm.rs`, every intermediate below `2^29`, range requests;
- rule degree 3 (`u·v²`), so a helper `w = v²` may keep the degree at 2.

Tests in the LBM style: rest state fixed point, the pattern spreads, honest proofs, forgeries per range bound,
`provable()`. Add `RuleId::GrayScott`, a genesis board (a few seeds of `v`), render colours, SimNet
`stencil-gs`.

## 5. S6: stateless beacon

As designed in ROADMAP "A4 step 2 design", but on the stencil statement (`d_prev`, `d_halo`, `d_next` are
already canonical roots):
- a Poseidon2 state root in the zone block header (a record layout change: golden vectors, `seg-samples.json`);
- an outputs digest (the face payloads) bound in the leaf;
- the beacon checks `chain()` and `d_halo` from input payloads without states;
- fault evidence for a root mismatch.

Depends on S5b + S4.

## Commands (copy-paste)

```sh
CARGO_BUILD_JOBS=8 scripts/check.sh                                   # gate (native)
cargo test --release -p umis-stencil -p task-stencil -p umis-agg      # proof tests
cargo build --release -p umis-simnet && target/release/umis-simnet --scenario stencil --seeds 3 --slots 40
target/release/umis-simnet --scenario all --seeds 3 --slots 40 --threads 6   # full sweep (~11 min, 96 runs)
cargo run --release -p task-stencil --example world -- --rule lbm --n 32 --region 2,2 --slots 6
RULE=lbm BATCH=1 cargo run --release -p umis-stencil --example bench -- 32,1,2   # leaf bench
HTTP=127.0.0.1:18431 DETACH=1 GENESIS_ARGS="--task stencil --rule lbm --n-loc 32 --region 2,2 --slot-ticks 16 --stark-config koalabear" scripts/devnet.sh 4
curl -s http://127.0.0.1:18431/status; scripts/devnet.sh stop
```
