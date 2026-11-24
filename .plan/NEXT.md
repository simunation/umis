# Next steps in detail (rewritten 2026-11-22 after steps 1–5 of the previous plan)

Read `.plan/HANDOFF.md` first (how to build, test, machine quirks, conventions). This file is the work plan.
`.plan/ROADMAP.md` §0 holds the goals (N1–N7) and status; `docs/stencil.md` is the user guide.

## Done in the last session (2026-11-22)

1. **S5b recursion of batch leaves** (`umis_agg::native`, INV-AGG-NB-1..4). Not the big trusted-source patch the
   old plan feared: P3R's public pieces suffice, plus two small local patches (`vendor/p3r-*.patch`, built by
   `scripts/vendor-p3r.sh`): a public accessor for the VK targets and an upstream periodic-columns layout bug.
   J101 went to 60 queries (LBM leaf 100.7 → 102.5 bits).
2. **S4 generic aggregation** (`umis_agg::stencil::StencilAggBackend`, INV-STENCIL-AGG-1..3), genesis
   `koalabear-agg` (now a secure config), SimNet `stencil-agg`, `-agg-lbm`, `-agg-junk`. Devnet heat 32²: 135 slots,
   0 rollbacks.
3. **Speed:** multi-slot leaves (`StencilBatch::new_multi`, INV-STENCIL-BATCH-4/TASK-3): a commitment's slots share
   one hash and one range table (heat 32² 79 → 21 ms per slot, LBM 32² 313 → 134 ms). Parallel trace assembly.
   Profile: symbolic degree inference in p3-batch-stark is ~25 % of prove and ~45 % of verify (LBM).
4. **Gray–Scott** toy 4 (`umis_stencil::grayscott`, `--rule grayscott`), SimNet `stencil-gs`, `stencil-gs-stark`.
5. **S6 v1 stateless beacon** (INV-STENCIL-TASK-4): stencil states start with `keccak(summary)`; proof
   commitments ship ~0.5 KB witnesses (summary + BMT inclusion proof) instead of states (37 KB for LBM 32²);
   the verifier checks everything but `outputs_root` without states.

## Order

1. ~~**S6b `d_faces`**~~ **done 2026-11-24** (INV-STENCIL-ROOT-3, BATCH-5, TASK-5): a 4th sponge over the next
   board's border in the batch leaf only (cap rows resend the `d_next` limbs, other rows send new side limbs; the
   uni AIR is test-only and keeps 3 digests). Summary v3 adds both boards' border digests; witnesses carry both
   borders; the verifier rebuilds the payloads and checks `outputs_root`. LBM 32²: +36 columns, proof +1 %,
   prove/verify within noise (A/B back to back).
2. ~~Plonky3 degree-inference cache~~ **done 2026-11-23** (owner OK): `vendor/p3-batch-stark-degree-cache.patch`,
   LBM 32² 8-slot prove 134 → 60 ms per slot, verify 156 → 82 ms.
3. ~~**Aggregation over multi-slot leaves**~~ **measured 2026-11-24, not built:** one layer 1 over an m-slot LBM
   leaf costs 4.9–5.0 s per slot (m = 2, 4) against 4.3 s + ~1 s of 2-to-1 per slot today, and m = 8 exceeds the
   2^24 FFT domain (`docs/zk/aggregation.md`). Layer 1 is in-circuit hashing of the opened columns, ∝ queries ×
   cells width. Real levers, if needed: a narrower cells AIR for LBM (fewer helper columns per cell), or fewer
   queries in Johnson at higher blowup (~10 % net, measured estimate).
4. ~~**Small zones**~~ **answered 2026-11-24: no.** The batch figure is set by the **shortest** instance
   (`ρ⁺ = (k + 2)/n` in the unique-decoding term), not the range table: cells at 2^6 give 101.6 bits, 2^5 would give
   100.5 (`the_shortest_instance_sets_the_batch_figure`). `security_shapes` now reports the worse height (it
   modelled the tallest only and overstated leaves by ~1.2 bits; they still cleared 101). Smaller padding needs
   more queries, a pure trade-off.
   - **Open (B9):** recursive (P3R circuit) proofs have tables down to one row. p3-security's per-instance model
     gives 0 bits there (its guard) and 57 bits at 2^1, meaningless for a codeword every query covers, so
     `recursive_shape` still reports the tallest table (105 bits at layer 1, 103 down to 2^5). Needs a
     multi-height FRI argument (or asking p3-security upstream how mixed heights should be modelled).
5. **GPU batch leaf:** not before 2 (the CPU-only symbolic step would dominate). `umis-gpu` is single-table.
6. **Hygiene:** B1 (P3R upstream issue draft `.plan/drafts/p3r-issue-periodic-columns.md`: now two fixes, ask
   before posting); B8.

## Commands (copy-paste)

```sh
scripts/vendor-p3r.sh                                                  # after a fresh clone (check.sh runs it)
CARGO_BUILD_JOBS=8 scripts/check.sh                                   # gate (native)
cargo test --release -p umis-stencil -p task-stencil -p umis-agg      # proof tests
cargo build --release -p umis-simnet && target/release/umis-simnet --scenario stencil --seeds 2 --slots 30 --threads 6
RULE=lbm BATCH=1 M=8 TRACE=1 cargo run --release -p umis-stencil --example bench -- 32,1,2   # leaf bench, spans
RULE=lbm cargo run --release -p umis-agg --example stencil_agg_bench -- 32,1,2               # recursion bench
HTTP=127.0.0.1:18431 DETACH=1 GENESIS_ARGS="--task stencil --rule lbm --n-loc 32 --region 2,2 --slot-ticks 16 --stark-config koalabear" scripts/devnet.sh 4
curl -s http://127.0.0.1:18431/status; scripts/devnet.sh stop
```
