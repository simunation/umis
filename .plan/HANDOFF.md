# Session handoff (state at commit `b1dc8b2`, 2026-11-21)

Read this first after a context reset, then `ROADMAP.md` (what to do next) and, before touching proof code,
`docs/zk/README.md` → `docs/zk/air-spec.md` / `docs/zk/life2d-air-spec.md` (CLAUDE.md rule).

## Where things are

| What | Where |
|---|---|
| Next steps, goals, schedule | `.plan/ROADMAP.md` |
| Original design (milestones M0–M8, done) | `.plan/simunation-poc-plan.md` |
| As-built architecture | `docs/ARCHITECTURE.md` |
| ZK knowledge base, open-issue table, decisions | `docs/zk/README.md` |
| Water AIR invariants (`INV-*`) | `docs/zk/air-spec.md` |
| Life AIR invariants (`INV-LIFE-*`), P100 security table, shape envelope | `docs/zk/life2d-air-spec.md` |
| Aggregation design and numbers | `docs/zk/aggregation.md` |
| FRI parameter theory and presets | `docs/zk/notes/fri-soundness-foundations.md` |
| Plonky3 / P3R API guide, phase checklists | `docs/zk/notes/plonky3-implementation-guide.md` |
| Protocol proof flow (shares, proof commitments, beacon) | `docs/zk/research/protocol-proof-flow.md` |
| GPU prover | `docs/gpu.md` |
| Life task, benchmarks, demo | `docs/life2d.md` |
| Security calculator probe (untracked, ours) | `tmp/zk-research/secprobe/src/main.rs` (env switches `LIFE`, `LIFEGRID`, `LIFEB`, `FINAL`, `RECUR`, `RECUR2`, `EF5`) |

## Code map (proof-related)

- `crates/umis-stark/src/config.rs`: `FriPreset::{P100, Benchmark, Test}`, configs `goldilocks`, `koala_bear`
  (Poseidon2), `koala_bear_keccak` (Keccak Merkle + `HybridChallenger`, GPU grinder hook).
- `crates/umis-stark/src/security.rs`: `ProofShape`, `proven_udr_bits(preset, shape)` (EF4), `proven_bits(fri,
  Field::{Ef4,Ef5}, Regime::{UniqueDecoding,Johnson}, shape)`, `TARGET_BITS = 101`.
- `crates/task-water3d/src/{lib.rs,air.rs}`: water task + AIR; `decode_cells`/`VAL_RANGE` (INV-CANON-1), shift ≤ 14.
- `crates/task-life2d/src/{lib.rs,shape.rs,air.rs,air/gpu.rs,air/tests.rs}`: life task, shape envelope (`MIN/MAX_*`),
  AIR, `MAX_BATCH = 32`, GPU prover glue, tests (`LIFE_PRESET=p100`, `LIFE_HASH=keccak` switches).
- `crates/umis-gpu`: generic GPU-resident prover (`prover.rs`, JIT `jit.rs`, `resident.rs` + `cuda/*.cu`, `pow.rs`).
- `crates/umis-agg`: aggregation over EF5 (`AggParams::{J101,U101,TEST}`, `config`, per-claim `leaf`/`layer1`/`aggregate`,
  claim-independent `digest_leaf` + reusable `Layer1Owner`/`AggOwner`, `Recursive::{verify,timing}`, `recursive_shape`),
  `tests/agg.rs`, `examples/agg_bench.rs` (`DIGEST=1`).
- `crates/task-life2d/src/air/digest.rs` (+ `digest_tests.rs`): claim-independent digest AIR (three inline Poseidon2 sponges).
- `crates/umis-agg/src/backend.rs` (+ `tests/backend.rs`): aggregating life backend; wired in `umis-tasks::backend_for` (native only).
- `crates/umis-core/src/genesis.rs`: `insecure_proofs`, `SECURE_STARK_CONFIGS`.
- `crates/umis-protocol/src/node/{mod.rs,duties.rs}`: pools (`pc_pool`, `pc_trusted`, `pc_alts`, `zc_awaiting`),
  `check_pc`, `check_pc_candidates`, `evict_pc`, junk adversary; `zone/engine.rs`: `trusted` shares, `ShareVerdict`;
  `beacon/state.rs`: `check_proof_commitment`, `awaits_proof`, `with_pending`, `with_zone_commitments`.
- `crates/umis-simnet/src/scenarios.rs`: scenarios incl. `junk-proofs`, `junk-proofs-l0`, `life-stark-junk`, `life-stark-l0`.

## How to build, test, verify

- Gate (must pass before every commit; check `echo $?` or the last line `all checks passed`, not a wrapper's status): `scripts/check.sh` (fmt, clippy -D warnings, native tests; native only since
  2026-11-22). `WITH_WASM=1` adds the frozen browser build. Takes about 10–20 min; run with `CARGO_BUILD_JOBS=8`.
- STARK tests in release (CLAUDE.md): `cargo test --release -p task-water3d --features stark`,
  `cargo test --release -p task-life2d --features stark` (and with `LIFE_PRESET=p100`, `LIFE_HASH=keccak`).
- GPU: `UMIS_GPU=3090 cargo test --release -p umis-gpu --features cuda` and `-p task-life2d --features cuda`
  (run GPU tests with `-- --test-threads=1` if the JIT warns about an invalid device context; it then interprets,
  still correct). Throughput: `examples/gpu_throughput` (`PRESET=benchmark` for the old preset), profile with
  `UMIS_GPU_PROFILE=1`.
- Aggregation: `cargo test --release -p umis-agg`; bench `PARAMS=j101|u101 cargo run --release -p umis-agg --example agg_bench -- 64,1,4,2 256,1,8,3`.
- SimNet sweep: `cargo build --release -p umis-simnet && target/release/umis-simnet --scenario all --seeds 5 --slots 60 --threads 10`
  (26 scenarios × 5 seeds, about 11 min at 6 threads with the aggregation scenarios; last result 130/130 OK, 2026-11-21).
- Security numbers: always `umis_stark::security` or the secprobe; never `queries × log_blowup + PoW`.
- Mutation-check every new negative test (disable the check, see the test fail, restore). Soundness tests must
  reject in the **verifier in release mode**; a debug-build prover panic is not a rejection, and tests must also
  pass in debug (check.sh runs debug), so adversarial proofs must be satisfiable traces.

## Machine and environment quirks

- 16 cores, shared with other users' jobs (load often 5–40): use `-j 8`, compare benchmarks back to back.
- GPUs: RTX A6000 (index 0) and RTX 3090 Ti (index 1), both shared; `UMIS_GPU=3090` / `A6000` selects by name.
  Driver CUDA 13.2 < toolkit 13.5: NVRTC compiles straight to CUBIN (sm_86); PCIe link runs at about 2 GB/s.
- Devnet HTTP port 8080 is taken: use `HTTP=127.0.0.1:18431`. Chrome: `/usr/bin/google-chrome`.
- `tmp/zk-research/` is untracked: papers, upstream clones (`src/`: read only, never build or run), secprobe (ours).
- Plonky3-recursion is a git dependency of `umis-agg` at rev `3a69ba61f713c8400b14eb3d8f0e52caa13512c7`; its
  checkout is under `~/.cargo/git/checkouts/plonky3-recursion-*/3a69ba6` (read API there, it is what we build).
- Branch `life2d-gpu-demo`; main branch for PRs is `nu`. Nothing pushed.

## Conventions

- Cite `INV-*` ids in commits and tests for proof changes; add new ids to the spec docs.
- Commit messages end with the attribution lines the harness supplies (Co-Authored-By + Claude-Session).
- Commit only when the owner asks (so far: "commit and continue" for every step).
- Owner prefers short, terse replies (caveman lite mode), but explanations must stay intuitive for a non-ZK reader.
- Ask before: pushing, posting upstream issues, adding new external dependencies (the sandbox blocks unapproved
  git dependencies).

## Decisions made (do not reopen without a reason)

100 proven bits (not 128) · P100 for leaves verified natively · L = 0 allowed with STARKs · Plonky3 `=0.9.0-rc.2` ·
P3R pinned `3a69ba6` · aggregation over KoalaBear⁵ (EF4 recursive layers cap at 93–96 bits) · J101 (Johnson) as the
aggregation default because it is ~1.6× faster with half-size proofs; U101 kept · GPU FRI not built (≤ 3 % gain) · P2 dual state roots (keccak BMT kept, Poseidon2 digest added; 2026-11-21) ·
aggregation leaves use one strip per zone (2026-11-21).

## Owner constraint (2026-11-21)

life2d and water3d are frozen toys; build a generic local-rule (stencil) framework with fast toy examples. See
ROADMAP §0. **Native only** (2026-11-22): no browser participants; new code need not build for wasm. Simulation
fit and the degree intuition: `docs/simulation-fit.md`.

## Next action

B2–B5, A1, A2, the A3 core and A4 step 1 done (2026-11-21). Life commitments can be aggregated in the protocol
(`stark_config_id = koalabear-agg[-test]`, `umis_agg::backend::LifeAggBackend`; SimNet `life-agg`, `life-agg-junk`,
release only, ~100 s each). Owner decisions: P2 dual roots; one strip per zone for aggregation leaves (best
`rpr = 2`). Next: ROADMAP §0 order: S1 stencil AIR framework (life as toy 1), S2 fixed-point diffusion toy, S3 generic root and
statement, S4 generic aggregation backend, S5 lookups, S6 stateless beacon.
