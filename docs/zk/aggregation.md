# Recursive aggregation (spike, 2026-11-20)

`crates/umis-agg` (native only) aggregates life2d leaf proofs with Plonky3-recursion (P3R), pinned to commit
`3a69ba61f713c8400b14eb3d8f0e52caa13512c7` (unaudited upstream; approved by the owner on 2026-11-20). Plonky3 is on
`=0.9.0-rc.2`, the version P3R pins.

## What exists

- `FriPreset::R100` (umis-stark): the leaf preset P3R can verify in-circuit. Blowup 4, arity 8, 128 queries, 16-bit
  query and per-round commit grinding, **no** OOD, batch or lookup grinding (INV-PARAM-5). Poseidon2 Merkle trees
  (`koala_bear` config; the Keccak config is not recursion-friendly).
- `umis_agg::AggConfig`: the umis KoalaBear/Poseidon2 config plus the FRI verifier parameters P3R needs
  (`FriRecursionConfig`).
- `leaf` (one advancing slot as a uni-STARK), `layer1` (a recursive proof verifying one leaf), `aggregate` (2-to-1
  over two recursive proofs), `verify_recursive` (native check), `recursive_shape` (security-calculator shape of a
  recursive proof).
- Tests (`tests/agg.rs`, test preset): leaf → layer 1 → 2-to-1, all verified; a leaf presented with another claim's
  public values does not recurse.
- Bench: `cargo run --release -p umis-agg --example agg_bench -- 16,2,1,2 64,1,4,2 256,1,8,3`.

## Measured (R100, 16 cores, load about 5)

| zone | leaf | layer 1 | 2-to-1 | verify (either) |
|---|---|---|---|---|
| 16×16 | 7.0 ms, 132 KB | 180 ms, 518 KB | 1.26 s, 617 KB | 13–15 ms |
| 64×64 | 9.5 ms, 113 KB | 143 ms, 532 KB | 1.28 s, 625 KB | 13–16 ms |
| 256×256 | 16.8 ms, 243 KB | 350 ms, 540 KB | 1.37 s, 619 KB | 14–16 ms |

Recursion cost does not grow with the zone; a beacon slot's worth of leaves (tens) folds into one ~620 KB proof
with a tree of depth 4–6, about 6–8 s of 2-to-1 work per level sequentially (parallel across a level).

## Security: the blocker

Leaf, at the default 256×256 shape: 101.8 proven bits (unique decoding). The **recursive proofs** are batch STARKs
over the circuit tables, verified in-circuit by the next layer, so they cannot grind at the batch step either:

| recursive proof | tables | tallest | opened codewords | proven (UDR, R100) |
|---|---|---|---|---|
| layer 1 | 5 | 2^14 | 1654 | **96.3** |
| 2-to-1 | 5 | 2^17 | 1654 | **93.3** |

The batch-combination term `123 − log2|LDE| − log2 B` binds; Johnson is worse without batch grinding, more blowup is
worse (bigger LDE). An aggregate is only as strong as its weakest layer, so this pipeline is at about 93 bits. Only
the outermost proof (verified natively by the beacon chain) may grind (`*_cross` APIs with an output config).

Ways to 100 bits, for the owner to choose:
1. **EF5 for the recursive layers** (`QuinticTrinomialExtensionField`; P3R has quintic tests): about 31 more bits on
   every field-size term; leaves may stay EF4. Cost: bigger opened values, P3R's "field extensions are not fully
   parametrizable" caveat.
2. **Batch grinding in P3R's in-circuit FRI verifier** (one Poseidon2 permutation check, like the query PoW it
   already supports): a small upstream change, but it means patching unaudited code or waiting for upstream.
3. Smaller tables (table packing) only reach about 96 bits: not enough.

Also open: the life AIR's per-claim periodic columns become circuit constants, so every slot builds its own
layer-1 circuit (INV-LIFE-FS-2); the claim data has to move into committed columns or public values before a
prepared, reusable layer-1 circuit is possible. P3R bug worked around in `LifeAir::eval`: its interaction probe
(`RecursiveAir::declares_interactions`) omits `num_periodic_columns` from the layout.
