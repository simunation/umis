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

- **DEG-1:** the AIR's symbolic degree (Plonky3 counts periodic columns as degree 1) equals the rule's degree.
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

## 7. Required negative tests (`src/tests.rs`, `--release`; each mutation-checked)

| test | the only check that rejects it |
|---|---|
| `false_claims_are_rejected` (false output with recomputed digests; false prev, false halo) | rule constraints, digests |
| `a_padding_cell_must_stay_valid` | RULE-1 (`eval_state`) |
| `digest_columns_are_bound_to_the_cells` | DIG-1 limb constraints |
| `the_sponge_state_is_the_multiplexed_permutation` | DIG-2 multiplexer |
| `the_sponge_chain_cannot_restart` | DIG-3 transition |
| `tampered_digest_columns_are_rejected` | DIG-2/3 incl. the first-row zero |
