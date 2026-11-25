# Next steps in detail (rewritten 2026-11-22; updated 2026-11-25)

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
   - ~~**B9**~~ **fixed 2026-11-25:** recursive proofs had a 1-row table (by the per-instance unique-decoding
     term ~48 bits at J101). Every P3R table is now padded to `MIN_TABLE_HEIGHT = 32` rows
     (`TablePacking::with_min_trace_height`), and `recursive_shapes` reports the worse of the tallest and shortest
     table: layer 1 and 2-to-1 clear 101 (`stencil_leaves_and_their_recursion_clear_101_bits_at_j101`,
     mutation-checked with the padding removed: 0 bits).
5. **GPU batch leaf: started 2026-11-25** (stencil-spec §6c', INV-STENCIL-GPU-1..3). Built and tested on the
   RTX 3090 Ti: the wide-row leaf hash (`WideRowHash`), `FastMmcs` with a pluggable tree backend (equal to the
   stock MMCS), `FastProver` / `config_with`, and the CUDA tree (`umis_gpu::merkle::GpuTree`). Opt-in with
   `UMIS_GPU_LEAF=1`: on the 3090 Ti (PCIe ×4, 1.9 GB/s) it only matches the CPU (tree 82 vs 87 ms per 8-slot LBM
   commit), because the 133 MB LDE upload dominates. The A6000 measured idle (2026-11-25) moves only 1.7 GB/s too
   (tree 130–140 vs 129–149 ms): both cards' PCIe links look stuck at a low speed (owner: check BIOS/risers).
   Measure only on an idle card (wait for `nvidia-smi` utilisation < 10 % and a low load average).
   Next, in order: keep the LDE on the device (GPU LDE feeding the tree, download only what the openings need),
   then the quotient on the GPU (multi-table JIT path in `umis-gpu/src/jit.rs` with LogUp), which removes most
   transfers.
6. **Hygiene:** ~~B1~~ (owner 2026-11-24: not reported upstream; the local patch stays); ~~B8~~ done 2026-11-25
   (share checks off the event loop); ~~B9~~ done 2026-11-25 (recursion tables padded to 32 rows).
7. ~~**Separate aggregator role**~~ **done 2026-11-25** (INV-STENCIL-AGG-4): genesis `agg_lag`; members prove
   plain leaves, aggregators (`NodeConfig::aggregator`) replace proven commitments' leaf proofs by aggregates in a
   window; missing aggregates are harmless. Follow-ups: pruning of replaced leaf proofs (archive size, the point of
   aggregating), aggregator incentives, and a devnet run with `agg_lag` (`umis-cli genesis` needs a flag).

8. **Faster layer 1 (2026-11-25, owner: "make the LBM proof narrower so aggregation keeps up"):** generic, not
   LBM-specific. LogUp budget up to `blowup + 1` (aggregation leaf 5 → 9: LBM leaf 3.49 → 2.66 MB) and
   `LAYER1_HORNER_PACK = 8` (ALU table 2^19 → 2^17): LBM 32² layer 1 5.9 → 3.3–3.7 s, 2-to-1 unchanged, per
   aggregated slot ~7.4 → ~4.9 s (`docs/zk/aggregation.md`, cost model). Next halving needs the Poseidon2 table
   under 2^16 (~30 % fewer opened leaf columns): hash limbs as expressions (−1,800 LBM columns) plus a narrower
   LBM rule (one rounding step instead of two: −8 columns and −8 lookups per cell) might reach it; measure first.
   Aggregation parallelises across slots and machines (the `agg_lag` window), so it need not match the prover.

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
