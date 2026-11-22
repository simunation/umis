# Stencil framework: specification and invariants (`crates/umis-stencil`)

The generic local-rule AIR (ROADMAP §0 S1). A rule declares its cell and update; the framework proves `k`
generations of an `n × n` zone from its extended board `E` (`we = n + 2·k·r`) and binds the claim by three
canonical Poseidon2 digests. Native only. Cite the `INV-STENCIL-*` ids in commits and tests. The intuition behind
degree and costs is in `docs/simulation-fit.md`.

## 1. What a proof claims

Public values `[d_prev, d_halo, d_next]` (3 × 8 KoalaBear elements):

- `d_prev = root(zone board before)`, `d_next = root(zone board after k generations)`,
  `d_halo = halo_digest(E)`;
- `next = E.evolve(rule, k)`: `k` applications of the rule, each shrinking the board by `r` on every side
  (`Grid::evolve`, the reference).

Missing neighbours: the caller fills their part of `E` with `Rule::boundary()`, which then evolves like any cell
(ghost cells). There are no masks or absent flags in the AIR.

## 2. Rule obligations (`Rule`, INV-STENCIL-RULE-*)

- **RULE-1:** `eval_state` forces every generation-0 value into `[0, 2^bits)` (life: booleanity). The digests
  pack values, so this is what makes a limb fix its values. Applied to every generation-0 cell, including
  padding (`a_padding_cell_must_stay_valid`).
- **RULE-2:** `eval_step` determines the next values uniquely and in range given valid inputs, and `step`'s
  output and helpers satisfy it. Applied to every cone cell of every generation on every row.
- `Spec::state_constraints` / `step_constraints` are the exact constraint counts.

## 3. Layout (INV-STENCIL-LAYOUT-*)

- **LAYOUT-1:** stacked torus of `rpr·height` rows. Generation `t` at trace row `ρ`, slot `i`, is stacked row
  `rpr·ρ + i + t·r`, so every constraint is a two-row constraint. This needs `rpr ≥ 2r` and `k·r ≤ rpr`
  (`Shape::new`).
- **LAYOUT-2 (light cone):** generation `t` is computed on columns `[t·r, we − t·r)`, and generation `k` on the
  zone's `n` columns. The `k·r` halo rows keep the padding and the torus wrap out of the zone's cone, so padding
  cells only need to be valid.
- **LAYOUT-3:** `Shape::new` rejects layouts that violate these bounds.

## 4. Digests (INV-STENCIL-DIG-*, INV-STENCIL-ROOT-*)

- **ROOT-1:** `root(board)` is a Poseidon2 sponge (width 16, rate 8, overwrite) over the board rows in order.
  Each row's `n·values` values are packed `30/bits` per limb and zero-padded to whole permutations. It is
  independent of the layout (`roots_do_not_depend_on_the_layout`, rpr 2–5).
- **ROOT-2:** `halo_digest(E)` is the same over every row of `E`, with the zone's cells set to 0.
- **DIG-1 (limbs):** per trace row, slot and digest, the row's limbs are committed columns constrained to the
  packing of the cells (`digest_columns_are_bound_to_the_cells`):
  - `d_prev` reads generation 0 at slot `i + k·r`, so its rows line up with `d_next`, which reads generation
    `k` at slot `i`;
  - `d_halo` reads generation 0 at slot `i`, with zone cells times `1 − inner0`.
- **DIG-2 (multiplexer):** the sponge state after a slot is `gate·perm(limbs, state) + (1 − gate)·state`. The
  gate is a shape-only periodic column: zone row for `d_prev`/`d_next`, row of `E` for `d_halo`. Rows outside
  a digest are skipped, not absorbed as zeros, so DIG equals ROOT
  (`the_sponge_state_is_the_multiplexed_permutation`; `honest_proofs_verify` checks that the trace's sponges
  end in the native digests).
- **DIG-3 (chain):** the state is 0 on the first row, each row starts from the previous row's last slot, and
  the last row's final state `[0..8]` is the public digest (`the_sponge_chain_cannot_restart`,
  `tampered_digest_columns_are_rejected`).
- **DIG-4 (chaining slots):** `d_next(t) = d_prev(t + 1)` exactly when slot `t + 1` starts from slot `t`'s board
  (`consecutive_slots_chain`).

## 5. Degree and parameters

- **DEG-1:** the AIR's symbolic degree (Plonky3 counts periodic columns as degree 1) is `max(3, rule degree)`.
  3 is the Poseidon2 S-box; heat diffusion's own degree is 2.
  Permutation inputs are committed limbs, and gates multiply only columns (`the_degree_is_the_rules_degree`:
  life is 3). Security inputs use `StencilAir::security_shape()`, which takes the symbolic degree.
- **PARAM-1:** `MIN_HEIGHT = 64`. Heights 2^3–2^5 give 93.8–100.5 proven bits at P100; 2^6 and up clear 101
  (`p100_clears_the_target_for_every_shape`).

## 6. Measured (life rule, P100, CPU, load about 6)

| zone | width × height | trace | prove | verify | proof |
|---|---|---|---|---|---|
| 64×64, k = 1, rpr 2 | 1438 × 64 | 1.7 ms | 53 ms | 49 ms | 803 KB |
| 256×256, k = 1, rpr 2 | 3514 × 256 | 10.6 ms | 101 ms | 115 ms | 1.9 MB |
| 256×256, k = 1, rpr 4 | 6980 × 128 | 11.3 ms | 188 ms | 225 ms | 3.8 MB |

That is about 5× the hand-tuned life prover (18.6 ms, 245 KB). About two thirds of the width is the three
in-trace sponges (about 2 permutations per board row and digest at n = 256). Prefer small `rpr`. The planned
fix is a separate hash table linked by a lookup bus (ROADMAP §0 S5).

## 6b. Toy 2: heat diffusion (`heat.rs`, S2)

16-bit temperatures, α = a/2^s (`4a ≤ 2^s`), `N = (2^s − 4a)·u + a·Σ₄ neighbours`, `u' = ⌊N/2^s⌋` proved as
`N = u'·2^s + rem`. Bit decompositions cover `rem` (s bits), `u'` (16 bits) and every generation-0 value
(16 bits). All integers stay below `2^(16+s) ≪ p`, so `u'` is unique. Rule degree 2; the AIR's is 3
(DEG-1). Missing neighbours are a fixed wall temperature.

Tests (each mutation-checked): `wrong_rounding_is_rejected` (rounding equation),
`rounding_bits_must_be_bits` (booleanity: `rem − 2^s` as a non-boolean "bit"),
`out_of_range_values_are_rejected` (a halo value of `2^16 + 5`: generation-0 recomposition), and P100 ≥ 101
bits.

| zone (P100, CPU) | width | prove | verify | proof |
|---|---|---|---|---|
| 64×64, k = 1, rpr 2 | 12,864 | 378 ms | 406 ms | 6.9 MB |
| 128×128 | 25,216 | 726 ms | 864 ms | 13.4 MB |
| 256×256 | 49,920 | 1.55 s | 1.72 s | 26.6 MB |

At 64×64 about 60 % of the columns are the in-trace sponges (16-bit values pack one per limb: 8 permutations
per board row and digest) and about 35 % are range-check bits (37 per computed cell). Both are what lookups and a
separate hash table remove (ROADMAP §0 S5).

## 6c. The batch leaf with lookups (`batch.rs`, S5a; INV-STENCIL-BATCH-*)

One batch STARK with up to three AIRs linked by LogUp buses (p3-lookup). Bus keys and counts use only main and
preprocessed columns: rc.2's LogUp trace builder cannot see periodic columns.

- **Cells AIR:**
  - the generations and the rule's constraints;
  - committed limbs of every digest row, bound to the cells (BATCH-1);
  - each 8-limb block sent on `stencil-hash` as `(region, seq, block)`, with count = a preprocessed gate. `seq`
    is preprocessed: board row × P + m, the canonical absorption order;
  - range requests looked up on `stencil-range`.
- **Hash AIR:** one Poseidon2 permutation per row, the three sponges in canonical order.
  - Each active row is a table entry `(region, seq, block)` with multiplicity 1 (preprocessed).
  - Zero capacity at a region's first row; chaining `(next.active − next.first)·(next.in − out) = 0` (BATCH-2).
  - The region's last row outputs its public digest.
- **Range AIR** (only if the rule requests ranges): the table `0..2^16` with a multiplicity column.
- **BATCH-3 (range requests):** `(x, b)` with `2 ≤ b ≤ 16` looks up `x` and, when `b < 16`, `x·2^(16−b)`. Since
  `x < 2^16`, the product is `< 2^30 < p` and cannot wrap. `b = 1` would wrap (`65025·2^15 − p < 2^16`,
  `the_one_bit_range_trick_would_wrap`), so it is a booleanity constraint.
- The public values equal the canonical `root` / `halo_digest` (`honest_batch_proofs_verify_for_both_toys`).
- Security: `StencilBatch::security_shape(proof)` reads the opened codewords from the proof (with upper estimates
  for constraints and LogUp). P100 clears 101 bits (`batch_leaves_clear_the_target_at_p100`).

Tests (each mutation-checked, two-sided for the buses):

| test | the only check that rejects it |
|---|---|
| `out_of_range_values_are_rejected`, `wrong_rounding_is_rejected` (all digests re-derived from the tampered cells) | range lookups |
| `limbs_are_bound_to_the_cells` (run a's cells, run b's limbs and hash table) | limb constraints |
| `the_hash_table_must_absorb_the_cells_blocks` (run b's hash table) | hash bus |
| `the_hash_chain_cannot_restart` | hash chaining |
| `cells_and_hash_blocks_must_agree` (tampered limb, block, public digest) | bus, last-row output |

Measured (P100, CPU):

| | uni (bits, in-trace hash) | batch (lookups, hash table) |
|---|---|---|
| heat 64×64 | 374 ms prove, 417 ms verify, 6.9 MB | 83 ms, 25 ms, 1.4 MB |
| heat 256×256 | 1.58 s, 1.70 s, 26.6 MB | 254 ms, 96 ms, 4.5 MB |
| life 64×64 | 53 ms, 53 ms, 803 KB | 9.3 ms, 8.0 ms, 410 KB |
| life 256×256 | 114 ms, 118 ms, 1.9 MB | 28 ms, 24 ms, 1.1 MB |

For heat, the fixed 2^16-row range table dominates small zones. A byte table plus a split (more helper columns)
trades that cost; worth it if the zones stay small. Trace generation of the batch leaf is still sequential per
row (`assemble`, 73 ms at 256×256).

## 6d. Toy 3: lattice Boltzmann (`lbm.rs`)

D2Q9 BGK with the He–Luo incompressible equilibrium, in exact integer arithmetic (unit `U = 2^10`, 16-bit
distributions). Pull streaming plus collision. Directions 1–8 use nearest rounding, proved as `q·d + r = N` with
lookup range checks on every quantity, so each equation holds over the integers (`|terms| < 2^29 ≪ p`).
`g'_0 = ρ − Σ g'_i` conserves mass exactly, and rest is an exact fixed point. The checks `ρ < 2^13` and
`|m| < 512` state the physical regime (`Lbm::provable` checks a slot natively). The rule's degree is 2, with 25
constraints and 60 range requests per cell.

Forgeries (each mutation-checked):
- `the_equilibrium_division_cannot_move_nine_into_the_remainder`: `feq − 1` with `r9 + 9`, stopped only by
  `8 − r9 ≥ 0`;
- `the_collision_cannot_round_up`: stopped only by `r2`'s range.

Range table choice (INV-STENCIL-BATCH-3): `Pairs` (`(v, b)` rows, one lookup per request) when a slot makes
≥ 2^15 narrow requests (LBM), otherwise `Single` (the 2^16 column, two lookups per narrow request; heat). Each
form has its own mutation-checked attack tests (`both_range_tables_are_covered_by_the_attack_tests`). Lookup
degree budget = the preset's maximum degree (P100: 5; LBM proofs 35 % smaller than with 3).

## 6e. The zone task (`crates/task-stencil`, INV-STENCIL-TASK-*)

`stencil2d@1`, layer `stencil`: an `n × n` zone of any rule (`RuleId::{Life, Heat, Lbm}`). Its protocol
semantics are life2d's, generalised:
- generation-tagged payloads of `k·r`-wide edges and corners;
- hold and birth, so committed states are globally exact;
- strict inputs.

Missing neighbours are the rule's boundary cells.
- **TASK-1 (native):** roots, canonical states (values in range), `outputs_root`, inputs against InputRefs, the
  decision recomputed, and every header field. Hold and birth slots are checked natively only (proof `[0]`).
- **TASK-2 (STARK):** for an advance, the verifier builds `E` itself and verifies the batch leaf against
  `public_values(E, next.grid)`.

Tests:
- `tests/global.rs`: lockstep zones equal one monolithic world (3 rules, k = 1, 2), and the LBM demo stays
  provable for 400 generations;
- `tests/proofs.rs`: real slots prove and verify; a false board with re-derived roots is rejected
  (mutation-checked); hold slots are native.

SimNet: `stencil-{life,heat,lbm}` (mock), `stencil-{life,lbm}-stark`, `stencil-lbm-junk`.

## 7. Required negative tests (`src/tests.rs`, `--release`; each mutation-checked)

| test | the only check that rejects it |
|---|---|
| `false_claims_are_rejected` (false output with recomputed digests; false prev, false halo) | rule constraints, digests |
| `a_padding_cell_must_stay_valid` | RULE-1 (`eval_state`) |
| `digest_columns_are_bound_to_the_cells` | DIG-1 limb constraints |
| `the_sponge_state_is_the_multiplexed_permutation` | DIG-2 multiplexer |
| `the_sponge_chain_cannot_restart` | DIG-3 transition |
| `tampered_digest_columns_are_rejected` | DIG-2/3 incl. the first-row zero |
