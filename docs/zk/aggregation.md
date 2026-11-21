# Recursive aggregation (`umis-agg`)

`crates/umis-agg` (native only) aggregates life2d leaf proofs with Plonky3-recursion (P3R), pinned to commit
`3a69ba61f713c8400b14eb3d8f0e52caa13512c7` (unaudited upstream; approved by the owner on 2026-11-20). Plonky3 is on
`=0.9.0-rc.2`, the version P3R pins. Status: working prototype, not wired into the protocol.

## Design

- **Field: KoalaBear⁵** (`QuinticTrinomialExtensionField`, about 2^155) for every aggregation proof. P3R's
  in-circuit verifier cannot check proof-of-work at the out-of-domain, batch or lookup steps (INV-PARAM-5). Over
  KoalaBear⁴ the batch-combination term (`F − log2|LDE| − log2 B`) then capped the recursive proofs at 93–96 bits
  (first spike, 2026-11-20). EF5 adds about 31 bits to every field-size term. P3R's cross-config APIs need the same
  challenge field on both sides, so leaves are EF5 proofs too.
- **Config:** upstream's built-in `KoalaBearD5Poseidon2BinaryConfig` (Poseidon2 Merkle trees, binary, 8-element
  digests) at `AggParams`; backend `FriRecursionBackend::new_d5(KOALA_BEAR_D1_W16)`.
- **Owners:** `TrustedPreparedLayer` (layer 1: verify one leaf in-circuit) and `TrustedPreparedAggregation` (2-to-1).
  Their `CircuitVerifier`s check each recursive proof against its expected statement: the leaf's public values (the
  claim digest), and for a 2-to-1 proof the left statement followed by the right one.
- **Parameters** (`AggParams`, all without OOD/batch/lookup grinding, 16-bit query and per-round commit grinding,
  fold arity 8):

  | preset | regime | blowup | queries | leaf / layer 1 / 2-to-1 proven bits |
  |---|---|---|---|---|
  | `J101` (default) | Johnson (DKT26 line count, `fri-soundness-foundations.md` §3.6) | 8 | 58 | 102.9 / 102.4 / 102.0 |
  | `U101` | unique decoding only | 4 | 128 | 102.7 / 102.8 / 102.8 |

  Over EF4 the Johnson regime gave nothing (the batch term bound in both); over EF5 it needs about half the queries,
  and recursive verifier circuits scale with the query count. The owner's rule (2026-11-20): take the newer bound
  where it is faster.

## Measured (16 cores, load about 6)

| zone | params | leaf | layer 1 | 2-to-1 | verify |
|---|---|---|---|---|---|
| 64×64 | J101 | 16.8 ms, 66 KB | 167 ms, 347 KB | 1.62 s, 404 KB | 9–11 ms |
| 256×256 | J101 | 23.2 ms, 136 KB | 324 ms, 347 KB | 1.79 s, 401 KB | 10–12 ms |
| 64×64 | U101 | 9.1 ms, 122 KB | 270 ms, 669 KB | 2.52 s, 835 KB | 17 ms |
| 256×256 | U101 | 20.9 ms, 260 KB | 447 ms, 705 KB | 3.07 s, 831 KB | 18–22 ms |

The earlier EF4 spike (insecure at about 93 bits) took 1.3 s and 620 KB per 2-to-1 step. Recursion cost does not
grow with the zone. Leaves are proved on the CPU: the GPU prover is EF4-only.

## Where the time goes (A1, 2026-11-21, J101, load 13–15)

Each recursive step is two parts: building the prepared owner (the verifier circuit for its children, P3R
`TrustedPrepared*::new`) and proving with it (`prove(&self, …)`, reusable).

| zone | step | build | prove | table log heights (6 tables) |
|---|---|---|---|---|
| 64×64 | layer 1 | 93–100 ms | 181–182 ms | 11, 6, 13, 12, 10, 0 |
| 256×256 | layer 1 | 165–189 ms | 338–367 ms | 14, 7, 14, 13, 11, 0 |
| any | 2-to-1 | 733–959 ms | 1.43–1.60 s | 9, 8, 16, 16, 13, 0 |

- The build takes about a third of every step.
- **The per-claim circuits cascade.** An aggregation owner fixes its children's verifiers. Every layer-1 circuit
  differs per claim, because of the periodic columns (INV-LIFE-FS-2). So every 2-to-1 owner must be rebuilt too,
  and nothing can be built once and reused.
- **The checker cannot use a fixed verifier.** It would have to rebuild the whole tree of circuits from the claims
  to check one aggregate (about 1 s per 2-to-1 node). A2 (claim-independent leaf) is therefore **required** before
  protocol integration (A4), not just an optimization. With reusable owners a 2-to-1 step drops to its prove time
  (about 1.6× faster).
- Leaves: under the same load, an EF5 J101 leaf on the CPU (including its native verify) took 20/36 ms
  (64×64 / 256×256), against 5.3/18.6 ms for an EF4 P100 proof (`bench_life_air`). The GPU EF4 single-proof
  latency is 3.7 ms at 256×256 (`docs/gpu.md`). Leaves are not the bottleneck: one 2-to-1 step costs as much as
  about 60 leaves.

## Claim-independent leaves (A2, 2026-11-21)

`task_life2d::air::digest::LifeDigestAir` (spec §4c, INV-LIFE-DIG-*) replaces the per-claim periodic data with a
Poseidon2 sponge in the trace. Its statement is the digest plus the 8 `absent` flags. `umis_agg::digest_leaf`
proves it; `Layer1Owner::new(p, representative)` and `AggOwner::new(p, left, right)` are built **once per zone
shape** and then `prove` any slot (`DIGEST=1` in `agg_bench`). J101, load 10–15, two runs:

| zone | leaf (width, size) | layer 1: build once, then prove | 2-to-1: build once, then prove | bits leaf / L1 / 2-to-1 |
|---|---|---|---|---|
| 64×64 | 28 ms (256 cols, 116 KB) | 123–126 ms, then 232–279 ms | 586–604 ms, then 1.15–1.25 s | 102.7 / 102.4 / 102.0 |
| 256×256 | 61–84 ms (450 cols, 186 KB) | 167–177 ms, then 302–332 ms | 659–707 ms, then 1.18–1.32 s | 102.8 / 102.4 / 102.0 |

The per-claim path measured back to back: leaf 11–28 ms, layer 1 0.20–0.44 s and 2-to-1 1.8–2.0 s, both
including their builds. So per leaf in a binary tree (one layer-1 and about one 2-to-1 proof), the cost drops
from about 2.3 s to about 1.6 s, and the checker has one fixed verifier per shape. The leaf itself costs about
2.5× more, which is small against the recursion.

## Tests and tools

- `tests/agg.rs` (test parameters): leaf → layer 1 → 2-to-1, each verified against its statement; a recursive
  proof against another statement and an aggregate with swapped statements are rejected; a leaf presented with
  another claim's public values does not recurse; `J101` and `U101` clear 101 bits for the CLI's leaf shapes.
- `umis_agg::recursive_shape` gives the security calculator a recursive proof's shape (tallest table, opened
  codewords, degree; constraint and LogUp counts are upper estimates).
- Bench: `PARAMS=j101|u101 cargo run --release -p umis-agg --example agg_bench -- 64,1,4,2 256,1,8,3`.

## Open

- ~~Per-claim periodic columns~~ (INV-LIFE-FS-2): solved for recursion by the digest leaf (above). The
  per-claim `leaf`/`layer1`/`aggregate` functions remain for comparison.
- **Statement for chaining (A3):** the digest binds `E` as a whole; chaining slots and binding to state roots
  needs separate digests (previous inner board, halo, `X_out`) and a way to fix the strip-halo duplicates (spec
  §4c "Open for A3").
- **Protocol integration:** an aggregator role, an aggregation deadline after `L`, an aggregate record checked by the
  beacon chain, and leaves bound to the beacon's statements (today the leaf statement is the claim digest).
- **GPU leaves:** the GPU prover's kernels are EF4; aggregation leaves need an EF5 port to use it.
- **P3R bug worked around** in `LifeAir::eval`: `RecursiveAir::declares_interactions` builds its layout without
  `num_periodic_columns`.
