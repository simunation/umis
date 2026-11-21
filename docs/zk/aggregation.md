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

## Tests and tools

- `tests/agg.rs` (test parameters): leaf → layer 1 → 2-to-1, each verified against its statement; a recursive
  proof against another statement and an aggregate with swapped statements are rejected; a leaf presented with
  another claim's public values does not recurse; `J101` and `U101` clear 101 bits for the CLI's leaf shapes.
- `umis_agg::recursive_shape` gives the security calculator a recursive proof's shape (tallest table, opened
  codewords, degree; constraint and LogUp counts are upper estimates).
- Bench: `PARAMS=j101|u101 cargo run --release -p umis-agg --example agg_bench -- 64,1,4,2 256,1,8,3`.

## Open

- **Per-claim periodic columns** (INV-LIFE-FS-2): the life AIR's claim data are circuit constants, so every slot
  needs its own layer-1 owner and circuit. Move them into committed columns or public values to reuse one prepared
  circuit per zone shape.
- **Protocol integration:** an aggregator role, an aggregation deadline after `L`, an aggregate record checked by the
  beacon chain, and leaves bound to the beacon's statements (today the leaf statement is the claim digest).
- **GPU leaves:** the GPU prover's kernels are EF4; aggregation leaves need an EF5 port to use it.
- **P3R bug worked around** in `LifeAir::eval`: `RecursiveAir::declares_interactions` builds its layout without
  `num_periodic_columns`.
