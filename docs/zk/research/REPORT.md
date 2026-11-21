# umis: proof system and simulation, analysis report

Date: 2026-11-03 (corrections from the knowledge-base pass are marked *corrected*; the authoritative, up-to-date docs are in `docs/zk/`) · Repo state: `nu` @ `4c26c42` · Author: Claude (Opus 5.5), with three research sub-reports.

This report covers how the zero-knowledge (STARK) proofs and the simulation work, how sound and fast they are, what the current research and libraries offer, and an ordered upgrade plan. Measured numbers come from this machine unless marked *(doc)*, which means they are quoted from `docs/ARCHITECTURE.md`.

Supporting material. The markdown reports sit next to this file in `docs/zk/research/`; the binaries and code are in `tmp/zk-research/`, which git doesn't track:

| File | Contents |
|---|---|
| `docs/zk/research/protocol-proof-flow.md` | How a proof moves through the protocol, from job creation to settlement, with file:line references and a list of files a change would touch |
| `docs/zk/research/plonky3-internals.md` | Plonky3 0.8.0 source audit: transcript, LogUp, periodic columns, FRI, security calculation, where the prover spends its time, upstream changes |
| `docs/zk/research/research-literature.md` | Literature and ecosystem survey with 45 citations |
| `tmp/zk-research/secprobe/` (+`out.txt`) | Runs Plonky3's `p3-security` calculator on umis' exact shape and on alternative presets |
| `tmp/zk-research/profprobe/` | Profiling harness for the water3d prover |
| `tmp/zk-research/scripts/sec*.py` | Independent Python re-implementation of the security formulas |
| `tmp/zk-research/papers/` | All 23 papers (PDF + text); catalogued in `docs/zk/papers.md` |
| `tmp/zk-research/src/` | Read-only clones of upstream repos: Plonky3 main (`p3`), Plonky3-recursion (`p3r`), Stwo, soundcalc |
| `tmp/zk-research/repro-aliasing/` | Reproducer for the non-canonical state-blob exploit (`docs/zk/air-spec.md` §2.2) |
| `tmp/zk-research/sketches/` | Compile-checked Plonky3 0.8 and 0.9.0-rc.2 sketches (Poseidon2 commitment AIR, security CI test) |

---

## 0. Summary

1. **The environment is set up and everything passes.** Toolchain: Rust 1.99.1 with the wasm32 target, wasm-bindgen-cli 0.2.108, and npm dependencies for `web/` and `scripts/`. These all pass:
   - the whole workspace test suite (release build);
   - the STARK feature tests;
   - SimNet's `stark` scenario;
   - the wasm determinism tests: the wasm prover produces exactly the same bytes as the native golden proof;
   - the web build.
2. **These are validity proofs, not zero-knowledge proofs.** umis uses Plonky3 STARKs with no hiding: every state and halo is public and shipped to verifiers. "ZK" here means *succinct verifiable computation*. That's fine for water, but it would need `HidingFriPcs` and a different data flow for private tasks.
3. **I found no soundness bug in how the AIR uses Plonky3, but there is one in umis' own encoding** *(found 2026-11-03 in the knowledge-base pass; confirmed exploit)*.
   - **What's sound:** the transcript binding, LogUp multiplicities, the memory bus and the periodic-column digest. Two agents and I checked this independently.
   - **The bug:** the verifier reduces i32 blob values mod p without range-checking them. A malicious prover can therefore get a valid proof for a *non-canonical* state blob (`v ± p`) with a different `state_root`.
   - **Today:** it is only caught because members still re-execute. Once proofs replace re-execution it is a real break.
   - **Fix:** `docs/zk/air-spec.md` INV-CANON-1. Reproducer: `tmp/zk-research/repro-aliasing/`.
4. **The security level is lower than the code claims.** `config.rs` says "~116 conjectured bits". Under the formulas Plonky3 now ships, the current preset gives:
   - **≈101 bits conjectured**, limited by the batching and LogUp terms;
   - **≈66 bits provable** (Johnson bound) and 57.5 bits under unique decoding.

   The up-to-capacity proximity-gap conjectures behind "conjectured" security were disproven in 2025–26. The Ethereum Foundation now accepts only provable security (100 bits now, 128 bits target). The `koalabear-test` preset is about 2 bits and any genesis can select it.
5. **Native provers run about 4× slower than they need to.** `.cargo/config.toml` sets no `target-cpu`, so Plonky3's KoalaBear AVX2/AVX-512 packing is compiled out. I measured n_loc=16 with 2 prover threads on this machine at the same load:

   | Build | Prove | Verify |
   |---|---|---|
   | Default (as shipped) | 355–419 ms | 10.8 ms |
   | AVX2 (`-C target-cpu=native`) | 89–92 ms | 8.0 ms |

   Browsers have the same problem in another form: Plonky3 has no wasm `simd128` path for KoalaBear, so wasm proving is scalar.
6. **The architecture won't scale as built.** Every zone slot produces its own 261 KB proof, and the beacon chain verifies all of them and receives full 64 KiB states for each one (`LeafData`).
   - With the 4 genesis zones that is about **3.9 MB per beacon block every 4 s**, and each proof crosses the network about 4 times.
   - At the lattice bound (128 zones) it would be about 130 MB and about 5 s of verification per 4 s slot.
   - The future-work items (recursive aggregation and in-circuit state commitments) are required, not optional.
7. **There are cheap denial-of-service paths around proofs.** Proof shares are sometimes stored without verification, and proof-commitment pools are first-writer-wins with no membership check. One junk item can trigger a `MissingProof` rollback of every zone. This is a liveness attack, not a safety one.
8. **Recommended path:**
   - **Phase 1 (days):** fix the build flags, raise the security parameters, and fix the DoS paths.
   - **Phase 2 (weeks):** move to Plonky3 0.9, prove one proof per commitment, and commit to state with Poseidon2 inside the circuit so verifiers don't need the states.
   - **Phase 3 (1–3 months):** aggregate per beacon slot with Plonky3-recursion, either with FRI or with WHIR on the inner layers.
   - A full move to Stwo, folding schemes or zkVMs isn't justified yet (§7).

---

## 1. Environment setup (done)

| Item | State |
|---|---|
| Rust | stable 1.99.1 (rustup synced it from `rust-toolchain.toml`; the workspace needs ≥ 1.88), plus `wasm32-unknown-unknown`, clippy and rustfmt |
| wasm-bindgen-cli | 0.2.108, which provides `wasm-bindgen-test-runner` (`~/.cargo/bin`) |
| Node | 22.14 (system). The README asks for Node 24, but Node 22 builds the web app and runs tsc and vite. I left the system Node alone; `n 24` can install 24 if Playwright needs it |
| npm | `web/` and `scripts/` dependencies installed (`npm ci`) |
| Browser | `/usr/bin/google-chrome` exists. Playwright's bundled browser isn't installed; `cd web && npx playwright install chromium` installs it before `scripts/e2e.sh` |

Results:

| Command | Result |
|---|---|
| `cargo build --release --workspace --all-targets` | ✓ |
| `cargo test --workspace --release` | ✓, all suites |
| `cargo test --release -p task-water3d --features stark` | ✓, including tampered-trace rejection and the golden proof |
| `cargo test --target wasm32-unknown-unknown -p {umis-core, task-water3d --features stark, umis-stark} --test wasm` | ✓, native and wasm proofs byte-identical |
| `umis-simnet --scenario stark --seeds 1` | ✓: 40 slots, 0 rollbacks, 0 slashes |
| `scripts/build-web.sh` | ✓ (wasm, wasm-bindgen, tsc, vite) |
| Not run | `scripts/e2e.sh` (Playwright browser download) and a real devnet (`scripts/devnet.sh`) |

**Benchmark caveat:** other jobs on this machine pushed the load average to 14–43 on 24 cores while I measured. Absolute timings are noisy. The A/B comparisons were run back to back and are reliable.

---

## 2. How the simulation works

**Model** (`crates/task-water3d`):
- Each zone is an n³ block of cells (n_loc = 16, so 4096 cells) at a lattice point. Each cell holds 4 values: velocity (vx, vy, vz) and dye c, stored as i32 fixed point bounded by ±2^15.
- The background current Ū is the discrete curl of a random vector potential built from vortex tubes, jets and drift (`field.rs`). That makes it exactly divergence-free, and every zone evaluates it in global coordinates so it is continuous across zones.
- A tide table (integer sine) modulates the forcing over time.

**One substep** is a linear 7-point stencil per cell and value (`stencil.rs`):

```
q'(x) = ⌊ ( Σ_{k∈{self,±x,±y,±z}} w_k(x)·q(x+k)  +  tide(j)·F(x) ) / 2^s ⌋,   w_k ≥ 0,  Σ w_k ≤ 2^s
```

- The weights fold together upwind advection along Ū, diffusion (ν for velocity, κ for dye) and relaxation or decay (λ, μ).
- A per-cell flux limiter keeps the weights non-negative.
- The forcing is λ·Ū for velocity and the dye source for dye.
- One zone slot runs `k_sub = 4` substeps. Defaults: s = 12, Δt = S_z·dt_tick/k_sub.

**Zones are coupled through halos.**
- Each of a zone's 6 face ports outputs a one-cell-thick face of 4 KB.
- A neighbour uses that face as its halo. A missing neighbour is treated as still water.
- A consumer picks the freshest causal producer output, or reuses the last one it had. Inputs are used immediately and checked later.

**Realism.** It's a damped linear Oseen-type model, chosen because it is easy to prove. Velocity relaxes toward tide·Ū, and dye is carried by Ū rather than by v. The review in `tmp/umis-review.tar.gz` measured the consequences:
- flow almost never crosses zones, because decay beats transport;
- a zone can legally ignore its neighbours, since input freshness isn't enforced.

**Determinism.** Consensus code uses integers only, the RNG is ChaCha20 seeded from the genesis seed, and the golden trace and golden proof are identical natively and in wasm.

**Protocol (outline):**
- Zones are mini-chains with seed-ranked minters and fallback ranks.
- The beacon chain (B = 4 s) holds time, seed, membership and stake.
- Each zone commits once per beacon slot (a `ZoneCommitment` covering `n_z` slots), and the proof commitment follows within `L = 1` slot.
- A fault (invalid transition, a bad InputRef, equivocation) leads to slashing and a rollback of every zone. A missing proof or an orphaned input leads to rollback without slashing.
- Settlement needs: proven, D = 2 slots since inclusion, and all producers settled.

The detailed proof flow is in `protocol-proof-flow.md`.

---

## 3. How the proof works

### 3.1 Statement

One proof covers **one zone slot** (`TransitionStatement {zone, params_hash, from_slot, to_slot=from_slot+1, prev_state_root, state_root, inputs_commitment, outputs_root}`). The roots are keccak/Swarm BMT addresses (bmt-js compatible). **The AIR proves none of these hashes.** Instead, the verifier is given the actual data and checks the roots itself (`air.rs:550-595`):

1. `bmt(prev) == prev_state_root` and `bmt(next) == state_root`.
2. `outputs(next)` hashes to `outputs_root`. It recomputes the faces itself.
3. Each input payload's `bmt` equals its `InputRef.payload_address`; the list root equals `inputs_commitment`; and there are no extra inputs.
4. It builds the AIR's periodic columns from `X_in`, `X_out` and the halos, and the public values from `tide(j)` and a keccak digest of `params_hash ‖ slot ‖ X_in ‖ X_out ‖ halos`.
5. It runs `verify_batch` against the zone's preprocessed commitment. That commitment is rebuilt locally from the on-chain params and the genesis seed, and it acts as the verifying key.

So the STARK proves exactly this: *"there is a trace that runs k_sub stencil substeps from these public cells and halos to these public cells, using the genesis coefficients for this zone."* The hashing is done outside the circuit, which is why verifiers need the full states.

### 3.2 AIR layout (`crates/task-water3d/src/air.rs`, Plonky3 0.8 `p3-batch-stark`)

- **Field:** KoalaBear (p = 2^31 − 2^24 + 1). Challenges come from the degree-4 extension (~124 bits).
- **Rows:** one per (substep t, cell i). For n_loc = 16 that's 4 × 4096 = 16,384 rows (2^14).
- **Main trace, 46 columns:** `q[4]`, the neighbours `nb[6][4]`, `qn[4]`, `rem[4]`, the byte limbs `lo[4]` and `hi[4]` of `qn + 2^15`, and the multiplicities `m_rem` and `m_byte`.
- **Preprocessed, 32 columns, committed once per zone:** address, `active`, `first`, `last`, `has[6]`, the read multiplicities, the 14 stencil weights, the 4 forcing shapes, and the range tables (`r mod 2^s`, `r mod 256`).
- **Periodic, 32 columns, period 4096, evaluated by the verifier:** `x_in[4]`, `x_out[4]`, `halo[6][4]`.
- **Public values, 17:** `tide` and the keccak digest as 16 × u16.

**Constraints** (all row-local, degree ≤ 3):

- Stencil: `Σ w·val + tide·F = 2^s·qn + rem`.
- Bounds: `qn + 2^15 = lo + 256·hi`, with `lo`, `hi` and `rem` each looked up in a table. That gives |qn| ≤ 2^15 and rem ∈ [0, 2^s).
- Halo: `active·(1−has_d)·(nb_d − halo_d) = 0`.
- I/O: `first·(q − x_in) = 0` and `last·(qn − x_out) = 0`.
- **LogUp memory bus:**
  - each row provides `(addr + C, qn)` with a preprocessed multiplicity (how many cells read it), plus `(addr, q)` on the first substep;
  - each row reads `(addr, q)` and `(addr + off_d, nb_d)` for every neighbour inside the zone.

  Since each address is provided exactly once with a fixed multiplicity, every read must equal the value written.

**LogUp cost.** There are 13 extension-field fraction columns plus an accumulator, which comes to 56 base-field columns. That's more than the main trace itself. The quotient is split into 2 chunks.

### 3.3 Why it is sound (verified)

- **No wraparound.** With s = 12 and |q| ≤ 2^15, the left side spans less than 2^28 and the right side spans 2^(s+16) = 2^28. Both are well below p ≈ 2^30.99, so the field identity is the integer identity.
- **That breaks at s ≥ 15.** The right side then spans ≥ 2^31 > p. Example at s = 16: `2^16·32512 + 1 = p`. `validate_params` accepts `shift ∈ 8..=16`. **Cap it at 14** (review finding #2, which I confirmed).
- **Fiat–Shamir.** p3-batch-stark 0.8 absorbs, in order:
  1. a domain label with the widths and instance count;
  2. the degree bits;
  3. the main commitment;
  4. the public values;
  5. the preprocessed commitment;
  6. then the lookup challenges, the permutation commitment and the LogUp terminals;
  7. the quotient commitment;
  8. the out-of-domain point;
  9. and finally FRI.

  It does **not** absorb periodic columns. umis binds them through the keccak digest in the public values, which closes the weak Fiat–Shamir gap (Dao et al., 2023). Upstream main still doesn't bind periodic columns in batch-stark (only in multi-stark), **so keep the digest.**
- **LogUp.** The multiplicity bound (19 × 2^14 < p) is checked by both sides, the `Count::bounded` gates are preprocessed booleans, and the challenges are drawn after the main commitment. The unconstrained table-side multiplicities and the ungated byte lookups are fine. LogUp fingerprint security is ≈102.7 bits.
- **Defense-in-depth gap.** `X_in` and the halos aren't range-checked in the circuit or natively. They are bounded by induction, since they are earlier proven outputs, but a native check that every value is within ±2^15 costs nothing (`verify_inner`).
- **Inconsistency.** The AIR treats a halo of the wrong length as zeros (`air.rs:296`), while `decode_inputs` rejects it. They should agree, and rejecting is the right behaviour.

### 3.4 Backends and protocol flow

- `ProofBackend` (`umis-proof`) has two implementations:
  - `MockBackend`: the "proof" is a signature, and validity comes from re-execution and fault evidence;
  - `Water3dStark`: id `plonky3`.
- Genesis picks one: `umis-cli genesis` defaults to plonky3 + `koalabear`, while `devnet_genesis()` and SimNet default to mock.
- **Jobs:** each accepted or minted block becomes one `ProveJob` per slot, carrying a full copy of the previous state and the inputs. Jobs go into a bounded, deadline-ordered queue (`duties.rs:311`) and run on a 2-thread rayon pool natively or in at most 4 Web Workers in the browser. The result becomes a `ProofShare`, gossiped on `zone/<z>/proof` and `zone/<z>/out`.
- **Commitment:** the delegate collects the shares, proves any missing ones itself at B/8, and publishes a `ProofCommitment`. That is the BMT root over the `ProofLeaf`s, plus the proofs and `LeafData`: the state at every leaf boundary and each slot's input payloads.
- **Beacon checks:** `apply_proof_commitment` (`beacon/state.rs:987`) checks one leaf per slot and that the leaves chain, then verifies every leaf. That runs in parallel natively, sequentially in wasm, and is memoized by (statement address, proof address).
- **Proofs don't replace re-execution.** Zone members still re-execute every block under Plonky3 (`duties.rs:204`, `engine.rs:394`). Proofs serve beacon validators and cross-zone consumers, and fault evidence is still checked by re-execution.

---

## 4. Security assessment

### 4.1 Cryptographic level

Computed with `p3-security` 0.8 on the exact umis shape (`secprobe/out.txt`), and cross-checked with an independent Python implementation:

| Preset | Proven (Johnson) | Proven (unique decoding) | Conjectured | What limits it | Proof size |
|---|---|---|---|---|---|
| `koalabear` = `new_benchmark` (lb1, 100 queries, 16-bit query PoW) | **65.9** | 57.5 | **101.4** | batching (101.4) and LogUp (102.7), not FRI queries | 261 KB measured |
| `koalabear-test` (4 queries, no PoW) | 2.0 | 1.7 | 3.9 | queries | ~22 KB |
| Same + 8-bit PoW at the OOD, batch and lookup steps (**not recursion-compatible**) | 65.9 | 57.5 | 109.4 | — | 255 KB est. |
| lb2, 128 queries, commit and query PoW 16, other PoW 0 (**recursion-compatible**) | **100.6** | — | — | — | ~230–285 KB est. |
| lb2, 64 queries, arity 2, final poly 2^3, 16-bit query PoW, 10-bit other PoW, lookup budget 5 | 79.9 | 59.4 | 110.6 | — | 159 KB est. |
| lb3, 34 queries, arity 2^3, 20-bit query PoW | 70.9 | 48.2 | 99.4 | — | ~99 KB est. |

Context from `research-literature.md` §2:

- **The conjectures fell.** In 2025–26, Diamond–Gruen, Crites–Stewart, BCHKS and Krachun–Kazanin–Haböck disproved the up-to-capacity proximity-gap conjectures. One counterexample works over prime-field multiplicative subgroups, which is exactly the setting here. "Conjectured" numbers now rely on weaker, still-open conjectures.
- **Proven bounds are improving.** BCHKS25 and Haböck 2025/2110 tighten the Johnson regime. Dao–Kominers–Thaler 2026 proves a first-order beyond-Johnson result, but it isn't usable for parameters over EF4: the multiple-correlated-agreement count caps the commit term near 90 bits. See `docs/zk/notes/proximity-gaps-status.md`. *(Corrected 2026-11-03.)*
- **The Ethereum Foundation and soundcalc** accept only provable security: 100 bits now, 128 bits by the end of 2026. SP1, for example, runs KoalaBear⁴ at rate 1/4 with 124 queries and 16 PoW bits for about 100 proven bits.
- **The 128-bit ceiling.** With an 8-element Poseidon2 digest and a degree-4 extension, everything is capped at about 124 bits. Reaching 128 bits needs KoalaBear⁵ and a digest of at least 9 elements.

**Recommendation.** Decide on a target (for example, ≥100 proven bits for mainnet-like settings) and add a `koalabear-100p` preset:
- `log_blowup = 2` with about 100–124 queries, or `log_blowup = 3–4` with about 50–64 queries;
- ~16-bit query PoW and 8–10 bits at the other PoW steps;
- arity 4–8 and a final polynomial of 8–32.

That costs about 160–285 KB per proof, depending on the choice, and would be smaller once aggregated. `log_blowup ≥ 2` also raises the constraint-degree cap from 3 to 5, which the planned nonlinear (v·∇)v physics needs. Also:
- Pass the preset's `log_blowup` into `setup()` instead of the literal `1` at `air.rs:472`. That budget argument currently does nothing and would go stale silently.
- **Forbid `koalabear-test` outside dev genesis files.** Genesis validation currently checks neither the backend, the `stark_config_id` nor `L`.
- **Malleability.** In 0.8.0 the MMCS verifier accepts extra unauthenticated cap roots (fixed upstream in #2378). That makes proof bytes malleable but not forgeable. umis hashes proof bytes (`proof_address`) and uses first-writer-wins pools, so a mauled copy of a valid proof is a different leaf. Consider keying on the statement rather than the proof address, or upgrade.

### 4.2 Protocol-level issues around proofs

These come from `protocol-proof-flow.md` and the earlier review; I checked the code for the first three.

1. **Unverified items can force a global rollback.**
   - `ZoneEngine::on_share` returns `true` (store and relay) when it can't build public data, i.e. for multi-slot blocks or a missing parent state (`engine.rs:688-691`, the `None => true` branch).
   - `on_proof_commit` (`node/mod.rs:511`) only checks the signature, from any key.
   - Both pools are first-writer-wins.
   - Result: one junk share or proof commitment means the beacon rejects it, `MissingProof` fires, and every zone rolls back. Fix: verify or check membership before pooling, keep several candidates per key, and let a valid item replace an invalid one.
2. **STARK verification runs on the event loop.** That's about 11 ms per share natively and more in wasm, and failed verifications aren't memoized, so block building re-verifies bad proofs on every trial apply.
3. **Bandwidth.** Each proof crosses the network about 4 times: two share topics, proof-commitment gossip and the beacon block. Consumers subscribe to producers' `/out` and download proofs they never use.
4. **Settlement after D = 2 is only safe under plonky3.** In mock mode it can settle invalid work; this is a known part of the POC.
5. **The beacon header doesn't commit to `LeafData` or the proof bytes.** That allows variants of the same block hash that carry bad data.
6. **The node secret key is posted to every prover Web Worker** (`umis-web/src/prover.rs`) but never used. Remove it.
7. **SimNet's `stark` scenario** shares one context and memo across all nodes, isn't in `tests/scenarios.rs`, and has no adversarial proof cases.

---

## 5. Performance and scaling

### 5.1 Per-proof numbers

| n_loc=16 (2^14 rows) | Prove | Verify | Size |
|---|---|---|---|
| Native, 24 threads *(doc)* | 91 ms | 11 ms | 261 KB |
| Native, 2 threads, **as shipped** (measured, A/B) | 355–419 ms | 10.8 ms | 261 KB |
| Native, 2 threads, **`target-cpu=native`** (measured, A/B) | **89–92 ms** | 8.0 ms | 261 KB |
| Native, 24 threads, AVX2 (profprobe) | 55–62 ms | — | — |
| wasm, 1 thread *(doc)* | 682 ms | 11 ms | 261 KB |

**Where the time goes** (2 threads, scalar build; `plonky3-internals.md` §5):

| Phase | Time |
|---|---|
| Poseidon2 Merkle hashing, total | ≈240 ms |
| — main trace commit | 88 ms |
| — LogUp/permutation trace | 112 ms |
| — quotient | 40 ms |
| Quotient evaluation | 106 ms |
| FRI | 38 ms |
| LDE | 33 ms |

The LogUp auxiliary trace (56 base columns) costs more to commit than the main trace.

### 5.2 System-level cost (the real problem)

These are per beacon slot (4 s) with the genesis layout: 4 zones with S_z = {4, 8, 4, 16}, giving 11 leaves.

- **Proofs:** 11 × 261 KB ≈ 2.9 MB.
- **LeafData:** 15 states × 64 KiB ≈ 0.94 MiB, plus inputs.
- **Beacon block:** ≈3.9 MB, while the proofs cross gossip about 4 times.
- **Beacon verification:** ≈120 ms per slot in wasm, sequential.

At the lattice bound (8×8×2 = 128 zones, about 4 leaves each):

- ≈130 MB of proofs per 4 s;
- ≈5 s of wasm verification per 4 s slot.

That's infeasible. The bounds that limit this are (a) proof count × proof size, and (b) shipping states to verifiers. Faster provers fix neither; only the architectural changes in §8 phases 2–3 do.

---

## 6. Upstream libraries

- **Plonky3**
  - **Versions:** 0.8.0 (2026-09-23) is pinned. The newest release is 0.9.0-rc.2 (2026-10-16), and there is no stable 0.9 yet. Main also has fixes after rc.2 (#2566–2585, #2517). None of them is exploitable in umis' configuration.
  - **Migration:** *(corrected 2026-11-03, verified)* moving to `=0.9.0-rc.2` is a pure version bump. With no code changes, all STARK tests pass, `golden-proof-4.txt` still matches, and the proof size is unchanged. The transcript and serialization are byte-identical across 0.8.0, rc.2 and main. Target rc.2, the version Plonky3-recursion pins, rather than main.
  - **What 0.9 adds:**
    - `p3-security`;
    - STIR, WHIR and the Circle PCS;
    - `p3-multi-stark`, a SuperSpartan-style design with LogUp-GKR buses and periodic-column binding;
    - HVZK pieces;
    - the LogUp weight-overflow fix (#2379).
  - **Browsers:** there is no wasm simd128 packing for KoalaBear on main either; only Goldilocks, Keccak, SHA-256 and BLAKE3 have it. That's the main wasm lever, and it's a good candidate to contribute upstream.
- **Plonky3-recursion** (`src/p3r`)
  - **Coverage:** in-circuit verification of uni- and batch-STARK proofs, with FRI or WHIR inside the circuit, 2-to-1 aggregation and ZK.
  - **Status:** tracks 0.9.0-rc.2 and is unaudited. The `p3-recursion` 0.1.0 on crates.io is stale; use git.
  - **Fit:** it's the most natural aggregation route, because it uses the same field, hash and AIR style.
- **Stwo** (`src/stwo`, Circle STARKs over M31)
  - **Strengths:** production-grade, has a wasm simd128 M31 path, and is the fastest prover in this class. FibRace ran 2.2M client proofs on phones.
  - **Cost of switching:** a new field, a new AIR DSL, and a rewrite of the AIR and config.
  - **When:** only if browser proving becomes the bottleneck after phase 1.
- **Folding/IVC** (Nova, HyperNova, LatticeFold+, Neo/SuperNeo, Arc)
  - **Fit:** conceptually perfect, because a zone chain is an IVC of the same step. But these schemes can't fold existing STARK proofs, and the lattice-based ones are research-grade.
  - **When:** a long-term, post-quantum option to watch.
- **zkVMs** (SP1 Hypercube, OpenVM 2.0, RISC Zero)
  - **Fit:** only useful as a heavy final wrap. Running a 4096-cell stencil inside a RISC-V VM is orders of magnitude more expensive than a custom AIR.
- **Prior art:** there is essentially no published ZK work on proving PDEs or stencils. The closest is zkML convolutions (zkCNN, sumcheck/FFT), which hints that a GKR or sumcheck formulation could prove linear stencils very cheaply at scale. This would be a research direction for umis, not something to copy.

---

## 7. Options I weighed

| Option | Verdict |
|---|---|
| Keep the Plonky3 batch-stark AIR, tune parameters and the build | **Do now.** Low risk, big win |
| Plonky3 0.9 + multi-stark (WHIR, LogUp-GKR) | Later. Requires rewriting the config and glue; gains are smaller proofs and fewer lookup columns |
| Aggregation via Plonky3-recursion | **Main medium-term path** |
| Port to Stwo/M31 | Defer. Benchmark first after the browser SIMD work |
| Folding schemes | Research track only |
| zkVM wrap | Only for an eventual on-chain or EVM settlement layer |
| Binius / binary fields | No. Poor fit for integer arithmetic |

---

## 8. Recommended roadmap

> **Status 2026-11-21:** Phase 1 is done except the items listed as B1–B8 in `.plan/ROADMAP.md` (shares on
> `/out`, failed-verification memo, off-loop share verification, worker secret key, STARK scenarios in the test
> suite). The security preset differs from item 2 below: leaves verified natively use P100 (grinding at every
> site, ≥ 101 UDR bits); aggregation runs over KoalaBear⁵ (`docs/zk/aggregation.md`). Phase 2.1 (Plonky3 0.9 rc.2,
> p3-security asserts in tests) is done. Phase 3 has a working prototype (`crates/umis-agg`). The live plan is
> `.plan/ROADMAP.md`.

### Phase 1: hygiene and quick wins (days, no protocol changes)

1. **Build flags (≈4× native prove time).**
   - Add `[target.x86_64-unknown-linux-gnu] rustflags = ["-C", "target-feature=+avx2"]` to `.cargo/config.toml`, or ship separate AVX2 and AVX-512 binaries.
   - Don't use `target-cpu=native` for distributed binaries.
   - Results are bit-identical: field arithmetic is exact, and the test preset golden proof still matches. Re-run `scripts/check.sh` to confirm.
2. **Security parameters.**
   - Fix the `config.rs` comment.
   - ⚠️ **Do not add PoW at the OOD, batch or lookup steps** if these proofs will ever be recursively aggregated: the Plonky3-recursion in-circuit verifier rejects non-zero PoW there. A recursion-compatible ≥100-proven-bit preset is `log_blowup 2`, 128 queries, commit and query PoW 16, others 0, which gives **100.6 proven bits** (`docs/zk/notes/plonky3-implementation-guide.md` §5). *(Corrected 2026-11-03.)*
   - Add a ≥100-bit-proven preset and choose the network target.
   - Pass `log_blowup` through to `setup`.
   - Reject `koalabear-test` and unknown backends in genesis validation.
3. **AIR guards.**
   - Cap `shift ≤ 14`.
   - Check natively in `verify_inner` that X_in and halo values are within ±2^15.
   - Reject wrong-length halos in `halos()`.
4. **DoS fixes.**
   - Never pool a share or proof commitment that hasn't been verified, or at least whose signer isn't a member.
   - Allow replacement or several candidates per key.
   - Memoize failed verifications.
   - Move share verification off the event loop.
   - Stop gossiping proofs on `/out`.
5. **Tests.**
   - Add the `stark` scenario to `tests/scenarios.rs` with per-node contexts.
   - Add adversarial cases: a junk share, a junk proof commitment, a mauled proof.
   - Make the oracle verify leaves.
6. **Browser.** Remove the secret key from worker messages.

### Phase 2: proof shape (weeks)

1. **Upgrade to Plonky3 0.9** (rc.2 now, stable when it ships). Use `p3-security` in CI to assert the chosen security level.
2. **One proof per commitment, not per slot.**
   - Prove `n_z` slots (or a recovery block's whole range) in one trace of height `n_z·k_sub·C`.
   - FRI and Merkle costs amortize: proof size grows with the log of the trace length.
   - For zone 0, 4 × 261 KB becomes about 300 KB.
   - Touches: the `to_slot == from_slot + 1` checks (3 places), `ProofLeaf`/`ProofCommitment`, `apply_proof_commitment`, `make_shares`, and the job types.
   - Trade-off: it gives up parallel per-slot proving by helpers; aggregation in phase 3 brings that back.
3. **Commit to state inside the circuit with Poseidon2**, so verifiers don't need states.
   - Add a Poseidon2 commitment of `X_in`, `X_out` and the halos as public values. It is computed in the circuit by a Poseidon2 AIR alongside the stencil AIR in the same batch, connected over a bus.
   - The beacon then tracks a `p2_state_root` next to the keccak/Swarm `state_root`.
   - This removes `LeafData` and the periodic columns, and with them the keccak digest workaround.
   - Cost: about 2–4k Poseidon2 permutations per state, a few tens of percent more proving.
   - Proving keccak BMT in the circuit instead is possible but about 10× more expensive. Keep keccak only for the Swarm/data-availability layer and bind the two with the minter's signature plus fault evidence.
4. **Shrink the lookups.** Range-check `qn + 2^15` with a 14-bit limb against `ADDR` plus 2 boolean bits instead of 8 byte lookups. Use LogUp-GKR if moving to multi-stark. The LogUp trace is currently the biggest commit cost.
5. **Browser proving.**
   - Implement (and upstream) wasm simd128 packing for KoalaBear in `p3-monty-31`.
   - Try `wasm-bindgen-rayon` with cross-origin isolation for multi-threaded workers.
   - Consider a Keccak or BLAKE3 MMCS for browser-heavy configurations; these have wasm SIMD.

### Phase 3: aggregation (1–3 months)

1. **Use Plonky3-recursion** to fold each zone's per-commitment proofs, then all zones' proofs per beacon slot, into one proof. Use 2-to-1 trees with WHIR on the inner layers and FRI on the outer one.
2. **Beacon validators** then verify one proof per slot, of about 100–200 KB, against the zone roots: the `zones_root`/`proofs_root` lattice commitment the plan already describes.
3. **Who aggregates.** Native full nodes; browsers keep proving leaves. This needs a new prover role and deadline (a second proof lag after `L`) and an aggregation record type.
4. **Then reconsider** settlement delay D, members' full re-execution, and finality. Fast proofs make zone-local fast finality and shorter rollback windows realistic.

### Phase 4: research tracks

- Nonlinear physics: degree-2 constraints, needing `log_blowup ≥ 2`. Add a pressure projection (iterative and expensive) only if realism matters.
- Enforce input freshness in the protocol (review §4).
- A GKR or sumcheck stencil prover, along the lines of zkCNN.
- Folding-based zone IVC (Neo/SuperNeo, LatticeFold+) once the crates mature.
- Hiding ZK (`HidingFriPcs` or HVZK-WHIR) for private tasks.

### Files each phase touches

The details are in `protocol-proof-flow.md` §8:

- **Phase 1:** `.cargo/config.toml`, `umis-stark/src/config.rs`, `task-water3d/src/{air.rs, lib.rs}`, `umis-core/src/genesis.rs`, `umis-protocol/src/{zone/engine.rs, node/mod.rs, ctx.rs}`, `umis-web/src/prover.rs`, `umis-simnet`.
- **Phases 2–3:** add `umis-core/src/records.rs` (record layouts), `umis-proof/src/lib.rs` (`LeafData`, `PublicData`, the trait), `beacon/state.rs:987-1076`, `node/duties.rs:293-374, 622-667`, `zone/engine.rs:593-881`, `types.rs`, the executors, the `umis-net` size limits, and the golden vectors.

---

## 9. Decisions needed from you

1. **Security target.** *Decided 2026-11-19: 100 proven bits.* (Aggregation now uses KoalaBear⁵ anyway, for a different reason: no batch grinding in-circuit.)
2. **Swarm compatibility of state roots.** Keep keccak BMT as the canonical root (dual roots), or make Poseidon2 canonical and use the keccak BMT only for storage?
3. **Browser role.** Leaf provers only, or also verifiers of every proof? That decides how much browser optimization is worth: SIMD and threads, versus moving to Stwo.
4. **Physics roadmap.** Is nonlinear advection or pressure planned? It changes the constraint degree and with it the FRI parameters.
5. **Version policy.** Track Plonky3 main or git commits (for the fixes and recursion), or only stable crates.io releases (0.9 isn't stable yet)? *Now: pinned `=0.9.0-rc.2` and Plonky3-recursion `3a69ba6`.*

---

## 10. Key references

The full list of 45 is in `research-literature.md` §6.

- Ben-Sasson et al., *Proximity Gaps for Reed–Solomon Codes* (2020/654); ethSTARK (2021/582); Haböck, *A summary on FRI* (2022/1216).
- Diamond–Gruen 2025/2010; Crites–Stewart 2025/2046; BCHKS 2025/2055; Krachun–Kazanin–Haböck 2026/782; Dao–Kominers–Thaler 2026/2056: the proximity-gap conjecture results.
- Dao–Miller–Wright–Grubbs, *Weak Fiat–Shamir* (2023/691); Khovratovich–Rothblum–Soukhanov 2025/118.
- Haböck, *LogUp* (2022/1530); Papini–Haböck, *LogUp-GKR* (2023/1284).
- Arnon–Chiesa–Fenzi–Yogev, *STIR* (2024/390) and *WHIR* (2024/1586); Haböck–Levit–Papini, *Circle STARKs* (2024/278).
- Nova (2021/370), HyperNova (2023/573), LatticeFold+ (2025/247), Neo/SuperNeo (2026/242), Arc (2024/1731).
- EF zkEVM security foundations (blog, 2025-12-18) and soundcalc; FibRace (arXiv 2510.14693).
