# life2d: a globally exact Game of Life on the zone lattice

`crates/task-life2d` (task `life2d@1`, layer `life`) is a 2D cellular automaton (Conway's B3/S23) that replaces water
as the simple test environment: easy to check, easy to look at, cheap to prove. Unlike water, which takes the freshest
neighbour data it has and holds otherwise, **every committed life state is exactly the global Game of Life at its
own generation**. No zone can run ahead of its neighbours, whatever the network does.

## Model

- The world is the `z = 0` plane of the lattice. Zone `(x, y)` owns the `n × n` cells
  `[x·n, (x+1)·n) × [y·n, (y+1)·n)`. Cells outside every zone are permanently dead.
- One zone slot advances `k` generations (`gens`). Advancing needs the `k`-wide halo around the zone, so the `life`
  layer links in-plane **diagonals** too (`TopologySpec::Lattice3d { diagonal_layers: ["life"] }`, ports 6..9),
  which gives a Moore neighbourhood of 8 links: 4 edges (`n × k` strips) and 4 corners (`k × k`).
- Genesis zones (`genesis_region`, `rx × ry` points from the origin) start at generation 0 with a seeded random soup
  (`keccak(seed ‖ "life2d-soup" ‖ point ‖ n)` → ChaCha20, `density_pct`).

## Exactness rule (inside the task's transition function)

State: `born, generation g, birth, next_slot, start_slot, grid, prev` (`prev` = the board at `g − k`). Every port
payload carries the producer's `born, slot_ticks, generation, birth, next_slot` and its facing strip at `g` and
at `g − k`.

A born zone at generation `g` advances only if, for every port, it can get the neighbour's strip **at exactly `g`**:

| Neighbour input | Halo at `g` |
|---|---|
| producer at `g` | its current strip |
| producer at `g + k` | its previous strip |
| producer born after `g` (`g < birth`) | dead (absent) |
| no producer at that point, outside the genesis region | dead (absent) |
| no input yet, genesis neighbour, `g = 0` | its genesis soup |
| anything else (older, newer, unborn, malformed) | **hold**: keep state, no generation change |

Absent halo regions stay dead in every one of the `k` generations (`run_extended_masked`): a missing world region
never grows cells. Because a zone needs exact-`g` strips and keeps `g − k`, adjacent zones are never more than `k`
generations apart, and a stalled zone stops its neighbourhood within one slot per ring.

**Created zones** start unborn. While a neighbour is unborn its neighbours hold. It is born (dead) at generation
`g0 = max` of its born neighbours' generations, once every born neighbour's payload was produced after the new zone's
first causal tick (`next_slot·S_p ≥ (start_slot + 1)·S_self`), i.e. once they are all frozen waiting for it. Before
its birth its region was absent for them, which matches "dead until born". This also merges groups of zones at
different generations consistently.

**Safety** (every state exact) holds for any input schedule: a wrong, stale or missing input can only cause a hold.
**Liveness** needs fresh inputs. Mixed slot lengths self-synchronise: a fast zone advances at the pace of its
slowest neighbourhood.

### Protocol support

- `ZoneTask::STRICT_INPUTS` (life: true). A block must carry an InputRef on every link whose producer has a causal
  block (`(j_start_p + 1)·S_p ≤ from·S_c`). Otherwise `validate` rejects it and `mint` waits. Without this rule a
  minter could drop an input and make a live neighbour look absent.
- Multi-slot blocks (`from < j`) hold every input they had, but a link without any input yet takes its first causal
  input. `validate` always allowed this; `mint` did not, which deadlocked strict zones whose first neighbour output
  arrived after their slot-1 mint window (found on the devnet, regression scenario `life-slow`).
- **Batched proving** (`ProofBackend::batches/prove_batch/verify_group`). For life the committer of a zone
  commitment proves all its advancing slots in one batch STARK as soon as the commitment exists (a container
  `ProveJob` whose `members` are the slot jobs; every executor runs it through `JobRunner`), instead of priority
  provers proving slot by slot; other members and full nodes prove the commitment themselves if no proof commitment
  has appeared `slot/4` after the beacon slot. The beacon chain verifies each proof commitment's leaves as a group
  (`Ctx::warm_proofs`, memoized). `UMIS_LIFE_BATCH=0` turns batching off. Water is unchanged.
- `umis_proof::MultiBackend` dispatches `plonky3` proofs per task (`PublicData::task_id`).
  `crates/umis-tasks` holds the build's task registry, `backend_for(genesis)` and `zone_proposal(...)`, used by
  the CLI, the web node and SimNet.

## Proofs

See [`zk/life2d-air-spec.md`](zk/life2d-air-spec.md) for the invariants. Summary:

- **Hold and birth slots carry no STARK.** The verifier re-runs `decide` and `apply` natively (cheap: unpack and
  compare), and the proof is the byte `0`.
- An advancing slot is a uni-STARK over KoalaBear with no lookups. Board rows go in trace rows and generations
  sit side by side in columns, so all 8 neighbours of a cell lie in the current or the next trace row and every
  constraint is a local two-row constraint. Per cell and generation there are 2 committed columns (the cell and
  an inverse witness), and the constraints have degree 3: `a'·f = 0`, `f·v = 1 − a'`, with
  `f = (c−3)(c−3+a) + mask`. There is no memory bus and no extension-field LogUp, unlike water.
- Generation 0 is committed too and bound to the claim by bit-packed periodic limbs (30 cells per limb, plus
  booleanity); the output is checked against packed limbs the same way, and the absent-region mask is one
  verifier-computed periodic column per (row offset, column class). This keeps periodic columns few: Plonky3
  extends each periodic column with its own DFT in the prover and interpolates each one in the verifier, and
  the first version (one periodic column per generation-0 cell and a 3·S-term mask per cell) spent most of its
  proving time there.
- Layout knobs (`strips`, `rows_per_trace_row`) trade trace width (proof size) against height. Light cones keep
  halo garbage out of the inner board. `k ≤ n/strips` is required.

### Benchmarks

Since 2026-11-19 genesis ids use `FriPreset::P100` (≥ 101 proven bits, `zk/life2d-air-spec.md` §6): at k = 1,
256×256 proves in 8.0 ms instead of 7.3 ms (250 KB instead of 194 KB), 64×64 in 6.7 instead of 2.6 ms; on the GPU,
3 provers × batch 16 take 0.77 instead of 0.62 ms per slot. The tables below are the earlier `Benchmark` runs.

Quiet machine (load ≈ 2–6 of 16 cores), 8 rayon threads (16 did not help), `FriPreset::Benchmark`, best of 5, native
build with AVX2 (`x86-64-v3`, `.cargo/config.toml`). Command:
`cargo run --release -p task-life2d --features stark --example bench_life_air -- n,k,strips,rpr …`
(`LIFE_HASH=keccak` for the Keccak configuration, `TRACE=1` for Plonky3's phase timings). Native "step" is the
task's own `step` (byte-per-cell, unoptimised). One proof per slot:

| n | k | strips | rpr | trace (w × h) | step | prove Poseidon2 | prove Keccak | verify P2 / Keccak | proof |
|---|---|---|---|---|---|---|---|---|---|
| 64 | 4 | 4 | 2 | 352 × 256 | 0.12 ms | 3.8 ms | 4.4 ms | 4.1 / 2.0 ms | 182 KB |
| 128 | 8 | 4 | 2 | 1344 × 512 | 0.85 ms | 12.9 ms | 10.7 ms | 13.8 / 6.0 ms | 615 KB |
| 256 | 8 | 8 | 3 | 2016 × 1024 | 3.0 ms | 27.5 ms | 19.3–20.3 ms | 23 / 11 ms | 917 KB |
| 256 | 16 | 8 | 3 | 4704 × 1024 | 6.5 ms | — | 43–46 ms | — / 23 ms | 2.1 MB |

Batched (per slot, one batch STARK over the slots of a commitment; `batch_timing` test, load ≈ 6):

| n | slots per batch | Poseidon2 single → batch | Keccak single → batch | batch verify per slot (Keccak) |
|---|---|---|---|---|
| 64 | 4 | 4.3 → 2.5 ms | 3.7 → **2.2 ms** | 1.2 ms |
| 64 | 16 | 5.1 → 1.9 ms | 4.2 → **1.5 ms** | 1.1 ms |
| 256 | 4 | 29.5 → 25.0 ms | 23.9 → **17.6 ms** | 5.9 ms |

A commitment covers `B / S` slots (4 for the devnet's `S = 4`, `B = 16`). Per cell-generation, the best
configuration (Keccak, batched) costs about 130 ns at `n = 64` and 67 ns at `n = 256`; the native step costs about
6 ns, so proving is about 11× the simulation at `n = 256` and about 18× at `n = 64`.

How it got here, `256, 8, 8, 3` at 8 threads: the first AIR proved in 206 ms (scalar build). AVX2 gave 121 ms. The
first AIR spent about 100 ms on 627 full-height periodic columns (Plonky3 extends and interpolates each one
separately) and a 3·S-term mask per cell, and 23 ms on single-threaded trace generation; packing generation 0 and
the output into limbs, collapsing the mask and parallel trace generation gave 50 ms under load and 27.5 ms on a
quiet machine. Keccak Merkle trees give 19–20 ms, batching 4 slots 17.6 ms per slot. At `n = 64`: 20 ms → 2.2 ms.

Measured and rejected:

- **More generations per slot** (`k`): per cell-generation about 1.2× cheaper at `n = 64`, neutral above; light
  cones grow with `k`. Bigger zones amortise far better (about 450 ns → 77 ns per cell-generation from `n = 64` to
  `n = 256`, single proofs).
- **Other layouts** (wider and shorter, better power-of-two fill): fewer cells but slower and much bigger proofs,
  since the out-of-domain evaluation and every FRI query scale with width (`256, 8, 4, 5`: 6080 × 256, 56–61 ms,
  2.6 MB). The defaults are the fastest measured.
- **No query PoW, more queries**: loses 8 proven bits (spec §6).
- **Keccak transcript**: its PoW grinding cost about 2 ms per proof; the hybrid challenger keeps Poseidon2.
- **GPU coset LDE alone** (`umis_gpu::GpuDft`): about 2× slower than the CPU on this machine's GPU links,
  because the trace and its LDE cross the bus. Superseded by the device-resident prover below.

**GPU-resident prover** (`air::gpu::GpuProver` on the generic `umis_gpu::prover::GpuStarkProver`, [`gpu.md`](gpu.md)):
trace generation, LDE, Keccak Merkle trees, quotient (a kernel compiled at run time from `LifeAir`'s symbolic
constraints) and FRI inputs on the GPU, Plonky3's transcripts and FRI on the CPU; the proof is the CPU Keccak
configuration's, verified unchanged. RTX 3090 Ti, quiet machine: `256, 8, 8, 3` 7.0 ms (CPU 18.7 ms; 2.3× the
native step), `256, 16, 8, 3` 12.8 ms (CPU 44.7 ms), `128, 8, 4, 2` 4.6 ms, `64, 4, 4, 2` 2.9 ms (CPU 4.3 ms).

For comparison, water3d `n_loc = 16` proved in 173 ms at 8 threads before AVX2 (about 10 µs per cell update).

## Evaluation

- **Exactness harness** (`tests/global.rs`): every state any zone reaches is compared with one monolithic Game of Life
  of the whole world at that state's generation (`reference::check_global`). It covers fresh schedules (lockstep,
  no generation lost), mixed slot lengths, adversarial schedules (random stale or out-of-order causal inputs, 20%
  skipped slots, 6 seeds, neighbours always ≤ k apart), zones created at runtime next to each other with different
  slot lengths, and a silent neighbour (the stall spreads, nobody runs ahead).
- **AIR tests** (`src/air/tests.rs`, release mode): the witness equals the native rule on 6 layouts; honest proofs
  verify on 5 layouts for centre and corner zones; the rejection cases are listed in the spec, §7.
- **SimNet** (real engine, beacon, gossip, faults). The oracle's check 9 compares every settled life state with the
  global Game of Life:

  | Scenario | What it covers |
  |---|---|
  | `life` | lockstep, every zone advances `k` per slot |
  | `life-mixed` | slot lengths 4/8/4/16 |
  | `life-create` | a zone created at runtime is born and advances |
  | `life-faults` | corrupt blocks, rollbacks, slashes; still exact |
  | `life-churn` | members leave and return |
  | `life-partition` | zones stall during a partition, then resume |
  | `life-stark` | real Plonky3 proofs (test preset) for every advancing slot |
  | `life-slow` | a zone's first outputs arrive a slot late (the multi-slot first-input regression) |

  `umis-simnet --scenario life-all`; `--scenario all` runs water and life.
- **Devnet:** `GENESIS_ARGS="--task life --n-loc 64 --gens 1 --region 3,2 --slot-ticks 4,4,8" scripts/devnet.sh 4`
  ran with 4 native nodes and real `koalabear` proofs. All 6 zones committed every beacon slot, every commitment
  was proven, and neighbouring zones stayed within `k` generations.
- **Browser end to end** (`web/e2e/life.spec.ts`, real proofs): with `MEMBERS=6` native joiners, a page node joins,
  founds a zone next to its own, and the new zone is born and advances with proven commitments; adjacent zones stay
  within `k` (zones with a rollback in the last 3 beacon slots are left out: right after one, the bootstrap's zone
  views mix the old and the new branch). Browser members prove their priority slots in Web Workers. Notes:
  - devnet validators are full nodes, so a zone has members only from joiners. A proposal is refused when any
    adjacent zone (diagonals included, for life) has no members or the founder would leave its zone empty;
    `MEMBERS` gives every zone one;
  - many browser nodes on one loaded machine mint late, so fallback minters fork their zones and consumers that took
    a losing block's outputs are rolled back (`CrossLink`). The rollback restores a consistent cut and life recovers
    (as water does); it costs progress, not exactness.

## Running it

```sh
umis-cli genesis --task life --n-loc 64 --gens 1 --region 3,2 --slot-ticks 4,4,8 --proof-backend plonky3
                                                                # (--stark-config koalabear-keccak is the default)
umis-cli sim --genesis genesis/devnet.json --beacon-slots 10    # reference run: gen + population per zone
GENESIS_ARGS="--task life …" scripts/devnet.sh 4                # bootstrap + nodes; the web UI shows the board
MEMBERS=6 GENESIS_ARGS="--task life …" scripts/e2e.sh life.spec.ts  # browser end to end (HTTP=… for another port)
umis-cli propose-zone --layer life --point 3,0,0 …              # grow the world
```

The web UI detects a life genesis and shows the 2D board. Zones are stitched at their global coordinates, with
trails for the cells alive `k` generations ago. Each zone border is coloured by its latest commitment status
(optimistic, proven, settled, invalid; red after a rollback), and each zone is labelled with its generation, its
lag behind the leader, and its population. Dashed tiles are free points: click one to propose a zone. A status
line shows the generation spread and the client-side check that adjacent zones are within `k`.

## Limits and next steps

- **GPU batching.** The GPU prover makes the same batch STARK as the CPU (INV-LIFE-BATCH-1..3): at k = 1 (the
  default), 256×256 costs 1.7 ms per slot in batches of 4 and 0.83 ms with three concurrent batch provers of 16,
  1.3× the native step ([`gpu.md`](gpu.md)). FRI folding on the GPU is the next lever.
- **Batching across zones.** Batches cover one zone's commitment (4 slots on the devnet). A prover holding several
  zones' commitments could batch them (×16 measured: 1.5 ms per slot at `n = 64`); this needs proof commitments
  to reference proofs outside their own envelope.
- Periodic columns hold per-claim data (generation 0 and `X_out`), so these proofs are not ready for recursion yet
  (INV-LIFE-FS-2).
- Batched shares (tags 2 and 3) do not verify on their own, so other nodes drop them on receipt; only the
  committer (or a backup prover after `slot / 4`) assembles the proof commitment.
- The native step is byte-per-cell. A bit-sliced step would be about 30–60× faster and would make the prove/step
  ratio look worse, without changing proving time.
- The `oracle` life check covers settled states. Optimistic states are checked by the harness, not by SimNet.

## Local demo (`scripts/demo/`)

`run.py` starts a whole network on this machine: a bootstrap, genesis validators (beacon chain, full nodes) and
joining zone members, each proving on the GPU (the RTX A6000 only: `CUDA_DEVICE_ORDER=PCI_BUS_ID`,
`CUDA_VISIBLE_DEVICES=0`) or on CPU cores; it fires scenario events (crash and restart a prover or a validator,
members joining late) and records each zone's generation, who minted every zone block, who committed and who proved
every commitment (`/lattice`: `minters`, `signer`, `proofs`), node status, A6000 memory and free RAM (it stops at
limits), and live boards from the network. `build_player.py` steps the global reference world from the same genesis
(`examples/world.rs`), checks every sampled board against it, and writes one self-contained HTML player (boards
bit-packed and deflated). The showcase genesis (`--pattern showcase`, `Life2dParams::pattern = 1`) puts gliders
through every zone junction, a glider gun, spaceships leaving the world and methuselahs on zone junctions;
`showcase_world_is_exact` checks it against the monolithic board.

    cargo build --release -p umis-cli --features cuda && cargo build --release -p task-life2d --example world
    python3 scripts/demo/run.py --out .demo/large --n 128 --region 4,3 --duration 300 \
      --validators gpu,cpu,cpu,cpu --members gpu,cpu,cpu,gpu,cpu,cpu,gpu,cpu,cpu,gpu,cpu,cpu --late cpu,gpu \
      --events "0.30:crash:m0,0.55:restart:m0,0.40:crash:v3,0.65:restart:v3"
    python3 scripts/demo/build_player.py .demo/large/recording.json player.html

Recorded run (18 nodes, 6 on the GPU, 12 zones of 128×128, 300 s): 67/67 live boards equal the global Game of Life;
steady 1 generation per second until the scenario events; then network-wide rollbacks (CrossLink) after the crash,
the late joins and the restarts, each recovered; A6000 ≤ 3.4 GB in total, free RAM ≥ 75 GB. Members joining at
genesis can cause one early rollback; on a loaded machine (other jobs) commitments sometimes miss their beacon slot.
