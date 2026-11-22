# Local-rule simulations with proofs (`umis-stencil`, `task-stencil`)

A **rule** says how one cell's next value follows from its neighbourhood. The framework proves that every zone
of the network applied the rule exactly, slot after slot, with ≥ 100-bit STARKs, and native nodes verify the
proofs. Four toy rules ship:

| rule | what it simulates | cell | proof cost per zone slot (P100, 16-core CPU) |
|---|---|---|---|
| `life` | Conway's Game of Life | 1 bit | 64×64: 18 ms prove, 9 ms verify, 0.4 MB |
| `heat` | heat diffusion (explicit, fixed point) | 16-bit temperature | 64×64: 90 ms, 24 ms, 1.1 MB |
| `lbm` | a 2D fluid (lattice Boltzmann D2Q9, incompressible BGK) | 9 × 16-bit distributions | 32×32: 358 ms, 159 ms, 5.9 MB |
| `grayscott` | Gray–Scott reaction–diffusion (spots that grow and split) | 2 × 14-bit concentrations | 64×64: 170 ms, 43 ms, 2.0 MB (8 slots per proof: 48 ms, 43 ms, 1.7 MB per slot; load ≈ 10) |

A proof commitment's advancing slots of one zone are proved together (up to 8 per proof, shared tables): per slot
heat 32² drops from 79 to 21 ms, LBM 32² from 313 to 134 ms (P100, CPU, 2026-11-22).

Why these and what comes next: `docs/simulation-fit.md` (intuitive). How the proofs work and what they check:
`docs/zk/stencil-spec.md`.

## Try it

All commands from the repository root; build once with `cargo build --release -p umis-cli -p task-stencil
--examples`.

**1. A world with a proof for every zone slot (one process, no network):**

```sh
cargo run --release -p task-stencil --example world -- --rule lbm --n 32 --region 4,2 --slots 40 --frames out/lbm
cargo run --release -p task-stencil --example world -- --rule life --n 64 --region 2,2 --slots 20 --frames out/life
cargo run --release -p task-stencil --example world -- --rule heat --n 64 --region 2,2 --slots 20 --frames out/heat
cargo run --release -p task-stencil --example world -- --rule grayscott --n 32 --region 2,2 --slots 20 --frames out/gs
```

Each slot: every zone steps with its neighbours' borders, its transition is proved through the protocol's proof
backend and verified from public data alone, and a picture of the world is written (`frame-NNNN.ppm`, any image
viewer opens PPM). Options: `--k` generations per slot, `--preset test` (fast, insecure), `--prove-every N`,
`--pattern 1` (no random noise).

**2. Long runs without proofs (the reference physics):**

```sh
target/release/umis-cli genesis --task stencil --rule lbm --n-loc 32 --region 4,2 --slot-ticks 4 \
  --proof-backend mock --keys-dir /tmp/k --out /tmp/lbm.json
target/release/umis-cli sim --genesis /tmp/lbm.json --beacon-slots 150 --frames out/lbm-sim
```

The fluid starts as a shear layer (the middle band flows right, the rest left) with a small wave; within about
200 generations it rolls up into a row of Kelvin–Helmholtz vortices that cross zone borders. Red is
anticlockwise vorticity, blue clockwise. The summary line shows mass (≈ 1, open edges) and the decaying kinetic
energy.

**3. The network (simulated):**

```sh
cargo build --release -p umis-simnet
target/release/umis-simnet --scenario stencil --seeds 3 --slots 40     # all eleven stencil scenarios
target/release/umis-simnet --scenario stencil-lbm-stark --seeds 1 --slots 30
```

`stencil-*-stark` runs real STARKs (test preset) through the whole protocol: provers, proof commitments, beacon
validators verifying every proof. `stencil-lbm-junk` adds an adversary flooding junk proofs.

**4. The network (real processes):**

```sh
HTTP=127.0.0.1:18431 GENESIS_ARGS="--task stencil --rule lbm --n-loc 32 --region 2,2 --slot-ticks 16 --stark-config koalabear" \
  scripts/devnet.sh 4
curl -s http://127.0.0.1:18431/status     # head, rollbacks, each zone's last commitment (Proven / Settled)
scripts/devnet.sh stop
```

Native nodes on localhost prove (P100) and verify every advancing zone slot. Logs are in `.devnet/`. Measured on
this 16-core machine (shared, load 5–9), 4 nodes:
- LBM 32² with `--slot-ticks 16` (4 s zone slots): 40 beacon slots, 0 rollbacks, every commitment Proven.
- With `--slot-ticks 8` the provers and validators fall behind: rollbacks.
- Life 64² with `--slot-ticks 8`: 0 rollbacks, every commitment Proven.

Capacity rule of thumb: nodes prove on 2 threads (`UMIS_PROVER_THREADS`), where a 32² LBM proof takes ~0.75 s
and a verification ~0.17 s. Every node verifies every proof, and an unaggregated beacon block carries every zone's
proofs (~6 MB each for LBM). Life 64² proofs are ~20× cheaper.

## Adding a rule

Implement `umis_stencil::Rule` (see `life.rs`, `heat.rs`, `lbm.rs`):

1. `spec()`: values per cell, bits per value (≤ 16 for lookups), radius, helper columns, constraint and range
   counts.
2. `step()`: the native update on a `(2r+1)²` neighbourhood, plus the helper values.
3. `eval_state()` / `eval_step()`: the same update as polynomial equations over the neighbourhood, written once
   for a generic ring, plus range requests `(x, b)` ("x is an integer below 2^b").

Obligations: generation-0 values are range-checked, and the step equations determine the next values uniquely and
in range. Keep every intermediate integer far below `p ≈ 2^31`: divide in stages and range-check the
quotients/remainders (`lbm.rs` shows how). Then add the rule to `task_stencil::RuleId` and its genesis board to
`init.rs`. The framework's tests (`umis-stencil/src/*tests.rs`) show how to write the release-mode forgery tests
every check needs.

## Limits and next steps

- **Recursion and aggregation (2026-11-22):** batch leaves recurse (`umis_agg::native`, a two-line local P3R
  patch). With `--stark-config koalabear-agg` a proof commitment's advancing slots are folded into one ~0.45 MB
  aggregate instead of one leaf per slot (LBM 32²: ~6 MB per slot before). Cost per slot (J101, CPU): EF5 leaf
  plus layer-1 recursion, heat 64² ≈ 1 s, LBM 32² ≈ 6 s; each 2-to-1 step ≈ 1 s. SimNet: `stencil-agg`,
  `stencil-agg-lbm`, `stencil-agg-junk`.
- **GPU:** the GPU prover (`umis-gpu`) handles single-table proofs only; the batch leaf proves on the CPU.
- **Block size:** without aggregation a beacon block carries every leaf proof of its slot. The gossip and sync size
  limits were raised to 128 MB and 256 MB for this (2026-11-22). Use `koalabear-agg` for large worlds.
- **Small zones:** the range table is a fixed cost (2^16 or 2^17 rows), so a 16² LBM zone proves almost as slowly as
  a 32² one. A byte table plus splits would help.
- **Boundary:** outside the world, cells hold the rule's boundary value each slot (dead cells, a wall
  temperature, fluid at rest): an open boundary for the fluid.
- **Fluid regime:** the proof checks density < 8 and velocity < 0.5 (lattice units) per cell. States outside
  that cannot be proved; `Lbm::provable` checks a slot natively.
