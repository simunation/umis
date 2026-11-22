//! The stencil AIR (INV-STENCIL-*; spec `docs/zk/stencil-spec.md`).
//!
//! # Layout
//!
//! `E` (`we × we`) is stacked on a vertical torus of `rpr · height` rows, `rpr` stacked rows
//! per trace row, padded with boundary cells. Generation `t` at trace row `r`, slot `i`, is
//! stacked row `rpr·r + i + t·r_n` (`r_n` the radius): each generation is shifted down by the
//! radius, so a generation-`t+1` cell reads generation-`t` slots `i..=i+2r_n` of the current
//! and the next trace row, and every constraint is a two-row constraint. Generation `t` is
//! computed on the columns `[t·r_n, we − t·r_n)` (the light cone); generation `k` on exactly
//! the zone's `n` columns. No constraint is gated: they hold on every row of the torus, and
//! the halo rows (`k·r_n` on each side) keep the padding and the wrap out of the zone's cone.
//!
//! # Digests (INV-STENCIL-DIG)
//!
//! Three sponges absorb **whole board rows** in order: `d_prev` the zone rows of generation
//! 0, read `k·r_n` stacked rows ahead so that they line up with `d_next`, the zone rows of
//! generation `k`; `d_halo` every row of `E` with the zone's cells zeroed. Per trace row and
//! slot each sponge has: committed limbs (the row's values packed `30/bits` per limb, so the
//! permutation inputs have degree 1), `P` permutations, and a 16-column state that is the
//! permutation output when the slot's row belongs to the region and the previous state
//! otherwise (a multiplexer on a shape-only periodic gate). Rows outside the region are
//! skipped rather than absorbed as zeros, so each digest equals the canonical native one
//! ([`root`], [`halo_digest`]) whatever the layout.

use crate::poseidon::{DIGEST, PERM_COLS, RATE, WIDTH, eval_permutation, permute, permute_into};
use crate::{Grid, Rule, Shape, Val};
use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::{PrimeCharacteristicRing, PrimeField32};
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use p3_maybe_rayon::prelude::*;
use std::borrow::Cow;

/// The three digests, in public-value order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Prev = 0,
    Halo = 1,
    Next = 2,
}
pub const REGIONS: [Region; 3] = [Region::Prev, Region::Halo, Region::Next];
pub const N_PUBLIC: usize = 3 * DIGEST;

impl Shape {
    /// Values a region absorbs per board row.
    fn row_values(&self, g: Region) -> usize {
        self.spec.values * if g == Region::Halo { self.we } else { self.n }
    }
    pub fn limbs(&self, g: Region) -> usize {
        self.row_values(g).div_ceil(self.per_limb())
    }
    pub fn perms(&self, g: Region) -> usize {
        self.limbs(g).div_ceil(RATE)
    }
    fn slot_cols(&self, g: Region) -> usize {
        self.limbs(g) + self.perms(g) * PERM_COLS + WIDTH
    }
    fn region_cols(&self, g: Region) -> usize {
        WIDTH + self.rpr * self.slot_cols(g)
    }
    pub(crate) fn region_base(&self, g: Region) -> usize {
        self.cells_width() + REGIONS[..g as usize].iter().map(|r| self.region_cols(*r)).sum::<usize>()
    }
    pub(crate) fn slot_base(&self, g: Region, i: usize) -> usize {
        self.region_base(g) + WIDTH + i * self.slot_cols(g)
    }
    pub(crate) fn perm_col(&self, g: Region, i: usize, m: usize) -> usize {
        self.slot_base(g, i) + self.limbs(g) + m * PERM_COLS
    }
    pub(crate) fn out_col(&self, g: Region, i: usize) -> usize {
        self.slot_base(g, i) + self.limbs(g) + self.perms(g) * PERM_COLS
    }
    pub fn width(&self) -> usize {
        self.region_base(Region::Next) + self.region_cols(Region::Next)
    }

    // periodic: zone[i] (sel of d_prev/d_next), in_e[i] (row of E), inner0[i] (zone row)
    fn p_zone(&self, i: usize) -> usize {
        i
    }
    fn p_in_e(&self, i: usize) -> usize {
        self.rpr + i
    }
    fn p_inner0(&self, i: usize) -> usize {
        2 * self.rpr + i
    }
    pub fn num_periodic(&self) -> usize {
        3 * self.rpr
    }

    /// Whether `d_prev`/`d_next` absorb at trace row `r`, slot `i`: stacked row
    /// `rpr·r + i + k·r_n` is a zone row (board row `rpr·r + i`).
    fn zone(&self, r: usize, i: usize) -> bool {
        self.rpr * r + i < self.n
    }
    /// Whether generation-0 slot `i` of trace row `r` is a row of `E`.
    fn in_e(&self, r: usize, i: usize) -> bool {
        self.rpr * r + i < self.we
    }
    /// Whether generation-0 slot `i` of trace row `r` is a zone row of `E`.
    fn inner0(&self, r: usize, i: usize) -> bool {
        (self.halo()..self.halo() + self.n).contains(&(self.rpr * r + i))
    }
    fn gate(&self, g: Region, r: usize, i: usize) -> bool {
        if g == Region::Halo { self.in_e(r, i) } else { self.zone(r, i) }
    }

    pub fn periodic(&self) -> Vec<Vec<Val>> {
        let mut per = vec![vec![Val::ZERO; self.height]; self.num_periodic()];
        for r in 0..self.height {
            for i in 0..self.rpr {
                per[self.p_zone(i)][r] = Val::from_bool(self.zone(r, i));
                per[self.p_in_e(i)][r] = Val::from_bool(self.in_e(r, i));
                per[self.p_inner0(i)][r] = Val::from_bool(self.inner0(r, i));
            }
        }
        per
    }

    pub fn num_constraints(&self) -> usize {
        let s = &self.spec;
        let state = self.rpr * self.we * s.state_constraints;
        let step: usize = (1..=self.k).map(|t| self.rpr * self.cone(t).1 * s.step_constraints).sum();
        let digests: usize = REGIONS
            .iter()
            .map(|g| self.rpr * (self.limbs(*g) + self.perms(*g) * PERM_COLS + WIDTH) + 2 * WIDTH + DIGEST)
            .sum();
        state + step + digests
    }
}

/// The cells of the current and next trace row.
struct Cells<'a, V> {
    s: &'a Shape,
    cur: &'a [V],
    next: &'a [V],
}

impl<V: Copy> Cells<'_, V> {
    /// Value `j` (or helper) of generation `t` at slot `slot` (`≥ rpr`: next trace row), column `x`.
    fn get<E: From<V>>(&self, t: usize, slot: usize, x: usize, j: usize) -> E {
        let (row, i) = if slot < self.s.rpr { (self.cur, slot) } else { (self.next, slot - self.s.rpr) };
        row[self.s.col(t, i, x, j)].into()
    }
}

/// A region's values at slot `i` (row order, cell-major), the zone cells of `d_halo`'s rows
/// multiplied by `1 − inner0`.
fn region_values<E: PrimeCharacteristicRing + Clone + From<V>, V: Copy>(
    s: &Shape,
    g: Region,
    cells: &Cells<'_, V>,
    i: usize,
    inner0: &E,
) -> Vec<E> {
    let (v, h) = (s.spec.values, s.halo());
    let mut out = Vec::with_capacity(s.row_values(g));
    match g {
        Region::Prev => (h..h + s.n).for_each(|x| (0..v).for_each(|j| out.push(cells.get(0, i + h, x, j)))),
        Region::Next => (h..h + s.n).for_each(|x| (0..v).for_each(|j| out.push(cells.get(s.k, i, x, j)))),
        Region::Halo => (0..s.we).for_each(|x| {
            (0..v).for_each(|j| {
                let c: E = cells.get(0, i, x, j);
                out.push(if (h..h + s.n).contains(&x) { (E::ONE - inner0.clone()) * c } else { c });
            })
        }),
    }
    out
}

/// Packs values `30/bits` per limb (`Σ v_m · 2^(bits·m)`), zero-padded to whole permutations.
fn pack<E: PrimeCharacteristicRing + Clone>(s: &Shape, vals: &[E], perms: usize) -> Vec<E> {
    let shift = E::from_u64(1u64 << s.spec.bits);
    let mut l: Vec<E> =
        vals.chunks(s.per_limb()).map(|c| c.iter().rev().fold(E::ZERO, |a, x| a * shift.clone() + x.clone())).collect();
    l.resize(perms * RATE, E::ZERO);
    l
}

fn pack_u32(s: &Shape, vals: &[u32], perms: usize) -> Vec<Val> {
    let v: Vec<Val> = vals.iter().map(|x| Val::from_u32(*x)).collect();
    pack(s, &v, perms)
}

/// Absorbs one row (its limbs, `P·8`) into `st`.
fn absorb(st: &mut [Val; WIDTH], limbs: &[Val]) {
    for block in limbs.chunks(RATE) {
        st[..RATE].copy_from_slice(block);
        *st = permute(*st);
    }
}

/// The canonical Poseidon2 root of a zone board (`n × n`): a sponge over its rows in order,
/// each row's `n·values` values packed `30/bits` per limb and zero-padded to whole
/// permutations (INV-STENCIL-ROOT-1). Independent of the trace layout (`rpr`, height).
pub fn root(s: &Shape, board: &Grid) -> [Val; DIGEST] {
    assert!(board.w == s.n && board.h == s.n && board.values == s.spec.values);
    let mut st = [Val::ZERO; WIDTH];
    for y in 0..s.n {
        absorb(&mut st, &pack_u32(s, board.row(y), s.perms(Region::Next)));
    }
    st[..DIGEST].try_into().expect("digest")
}

/// The canonical digest of `E` outside the zone: every row of `E` in order with the zone's
/// cells set to 0 (INV-STENCIL-ROOT-2). A checker without the zone's state recomputes it
/// from the neighbours' payloads and the boundary.
pub fn halo_digest(s: &Shape, ext: &Grid) -> [Val; DIGEST] {
    assert!(ext.w == s.we && ext.h == s.we);
    let (h, v) = (s.halo(), s.spec.values);
    let mut st = [Val::ZERO; WIDTH];
    for y in 0..s.we {
        let mut row = ext.row(y).to_vec();
        if (h..h + s.n).contains(&y) {
            row[h * v..(h + s.n) * v].fill(0);
        }
        absorb(&mut st, &pack_u32(s, &row, s.perms(Region::Halo)));
    }
    st[..DIGEST].try_into().expect("digest")
}

/// The public values of a claim: `[d_prev, d_halo, d_next]`.
pub fn public_values(s: &Shape, ext: &Grid, next: &Grid) -> Vec<Val> {
    let prev = root(s, &ext.inner(s.n, s.halo()));
    prev.into_iter().chain(halo_digest(s, ext)).chain(root(s, next)).collect()
}

/// The AIR of rule `R` on a zone shape. Depends on the shape only (INV-STENCIL-AIR-1).
#[derive(Clone)]
pub struct StencilAir<R> {
    pub rule: R,
    pub shape: Shape,
    periodic: Vec<Vec<Val>>,
}

impl<R: Rule + Clone> StencilAir<R> {
    pub fn new(rule: R, n: usize, k: usize, rpr: usize) -> Result<Self, String> {
        let shape = Shape::new(rule.spec(), n, k, rpr)?;
        Ok(Self { periodic: shape.periodic(), rule, shape })
    }

    /// The constraint degree Plonky3 derives (periodic columns count as degree 1). It must
    /// equal the rule's own degree (INV-STENCIL-DEG-1).
    pub fn symbolic_degree(&self) -> usize {
        use p3_air::symbolic::{AirLayout, get_max_constraint_degree};
        let layout = AirLayout {
            main_width: self.shape.width(),
            num_public_values: N_PUBLIC,
            num_periodic_columns: self.shape.num_periodic(),
            ..Default::default()
        };
        get_max_constraint_degree::<Val, _>(self, layout, self.shape.height)
    }

    /// The proof's shape for the security calculator (EF4 leaf, uni-STARK): main at `zeta` and
    /// `zeta·g`, the quotient chunks over EF4 at `zeta`, the symbolic degree.
    pub fn security_shape(&self) -> umis_stark::security::ProofShape {
        let d = self.symbolic_degree();
        let chunks = (d.max(2) - 1).next_power_of_two();
        umis_stark::security::ProofShape {
            log_height: self.shape.height.trailing_zeros() as usize,
            num_constraints: self.shape.num_constraints(),
            max_degree: d,
            batched_openings: 2 * self.shape.width() + chunks * 4,
            logup: None,
        }
    }

    /// The honest trace of `ext` (`we × we`) and the zone's board after `k` generations.
    pub fn generate_trace(&self, ext: &Grid) -> (RowMajorMatrix<Val>, Grid) {
        let s = &self.shape;
        let (rows, r, v) = (s.rows(), s.spec.radius, s.spec.values);
        assert!(ext.w == s.we && ext.h == s.we && ext.values == v);
        let boundary = self.rule.boundary();
        // gens[t][stacked row] = (values of the cone, helpers of the cone)
        let mut vals: Vec<Vec<Vec<u32>>> = Vec::with_capacity(s.k + 1);
        let mut auxs: Vec<Vec<Vec<Val>>> = Vec::with_capacity(s.k + 1);
        let g0: Vec<Vec<u32>> =
            (0..rows)
                .map(|y| {
                    if y < s.we {
                        ext.row(y).to_vec()
                    } else {
                        boundary.iter().copied().cycle().take(s.we * v).collect()
                    }
                })
                .collect();
        let a0: Vec<Vec<Val>> =
            g0.par_iter().map(|row| row.chunks(v).flat_map(|c| self.rule.state_aux(c)).collect()).collect();
        vals.push(g0);
        auxs.push(a0);
        for t in 1..=s.k {
            let (px, pw) = s.cone(t - 1);
            let (x0, w) = s.cone(t);
            let prev = &vals[t - 1];
            let (nv, na): (Vec<Vec<u32>>, Vec<Vec<Val>>) = (0..rows)
                .into_par_iter()
                .map(|y| {
                    let (mut rv, mut ra) = (Vec::with_capacity(w * v), Vec::with_capacity(w * s.spec.aux));
                    for x in x0..x0 + w {
                        let nb: Vec<&[u32]> = (0..2 * r + 1)
                            .flat_map(|dy| (0..2 * r + 1).map(move |dx| (dx, dy)))
                            .map(|(dx, dy)| {
                                let row = &prev[(y + rows + dy - r) % rows];
                                let xx = x + dx - r - px;
                                debug_assert!(xx < pw);
                                &row[xx * v..][..v]
                            })
                            .collect();
                        let (nx, aux) = self.rule.step(&nb);
                        rv.extend(nx);
                        ra.extend(aux);
                    }
                    (rv, ra)
                })
                .unzip();
            vals.push(nv);
            auxs.push(na);
        }
        let cw = s.cells_width();
        let mut life = Val::zero_vec(s.height * cw);
        life.par_chunks_mut(cw).enumerate().for_each(|(tr, row)| {
            for t in 0..=s.k {
                let (x0, w) = s.cone(t);
                let nav = s.cell_cols(t) - v;
                for i in 0..s.rpr {
                    let y = (s.rpr * tr + i + t * r) % rows;
                    for xi in 0..w {
                        let c = s.col(t, i, x0 + xi, 0);
                        for j in 0..v {
                            row[c + j] = Val::from_u32(vals[t][y][xi * v + j]);
                        }
                        row[c + v..c + v + nav].copy_from_slice(&auxs[t][y][xi * nav..(xi + 1) * nav]);
                    }
                }
            }
        });
        let mut next = Grid::filled(s.n, s.n, &vec![0; v]);
        let h = s.halo();
        for yy in 0..s.n {
            let y = yy + h;
            next.data[yy * s.n * v..(yy + 1) * s.n * v].copy_from_slice(&vals[s.k][y]);
        }
        (self.extend_trace(RowMajorMatrix::new(life, cw)), next)
    }

    /// Appends the digest columns computed from the trace's own cells.
    pub fn extend_trace(&self, cells: RowMajorMatrix<Val>) -> RowMajorMatrix<Val> {
        let s = &self.shape;
        let (cw, w, h) = (s.cells_width(), s.width(), s.height);
        assert_eq!(cells.width(), cw);
        let mut v = Val::zero_vec(h * w);
        for (r, row) in v.chunks_mut(w).enumerate() {
            row[..cw].copy_from_slice(&cells.values[r * cw..][..cw]);
        }
        for g in REGIONS {
            let mut st = [Val::ZERO; WIDTH];
            for r in 0..h {
                let next_r = (r + 1) % h;
                let (cur, nxt) = (v[r * w..r * w + cw].to_vec(), v[next_r * w..next_r * w + cw].to_vec());
                let row = &mut v[r * w..(r + 1) * w];
                let base = s.region_base(g);
                row[base..base + WIDTH].copy_from_slice(&st);
                let cells = Cells { s, cur: &cur, next: &nxt };
                for i in 0..s.rpr {
                    let inner0 = Val::from_bool(s.inner0(r, i));
                    let limbs = pack(s, &region_values::<Val, Val>(s, g, &cells, i, &inner0), s.perms(g));
                    let lb = s.slot_base(g, i);
                    row[lb..lb + s.limbs(g)].copy_from_slice(&limbs[..s.limbs(g)]);
                    let mut p = st;
                    for m in 0..s.perms(g) {
                        p[..RATE].copy_from_slice(&limbs[m * RATE..][..RATE]);
                        let c = s.perm_col(g, i, m);
                        p = permute_into(p, &mut row[c..c + PERM_COLS]);
                    }
                    if s.gate(g, r, i) {
                        st = p;
                    }
                    let o = s.out_col(g, i);
                    row[o..o + WIDTH].copy_from_slice(&st);
                }
            }
        }
        RowMajorMatrix::new(v, w)
    }
}

impl<R: Rule + Clone> BaseAir<Val> for StencilAir<R> {
    fn width(&self) -> usize {
        self.shape.width()
    }
    fn num_periodic_columns(&self) -> usize {
        self.shape.num_periodic()
    }
    fn periodic_columns(&self) -> Cow<'_, [Vec<Val>]> {
        Cow::Borrowed(&self.periodic)
    }
    fn num_public_values(&self) -> usize {
        N_PUBLIC
    }
    fn num_constraints(&self) -> Option<usize> {
        Some(self.shape.num_constraints())
    }
}

impl<AB: AirBuilder<F = Val>, R: Rule + Clone> Air<AB> for StencilAir<R> {
    fn eval(&self, builder: &mut AB) {
        let s = &self.shape;
        let (r, v) = (s.spec.radius, s.spec.values);
        let main = builder.main();
        let (cur, next) = (main.current_slice(), main.next_slice());
        let cells = Cells { s, cur, next };
        let mut per: Vec<AB::Expr> = builder.periodic_values().iter().map(|x| (*x).into()).collect();
        if per.is_empty() {
            // Plonky3-recursion's interaction probe (rev 3a69ba6) omits periodic columns; only
            // the constraint shape matters there.
            per = vec![AB::Expr::ZERO; s.num_periodic()];
        }
        let public: Vec<AB::Expr> = builder.public_values().iter().map(|x| (*x).into()).collect();
        let mut out: Vec<AB::Expr> = Vec::new();

        // Generation 0: valid cells (INV-STENCIL-RULE-1).
        for i in 0..s.rpr {
            for x in 0..s.we {
                let c: Vec<AB::Expr> = (0..v).map(|j| cells.get(0, i, x, j)).collect();
                let a: Vec<AB::Expr> = (0..s.spec.aux0).map(|j| cells.get(0, i, x, v + j)).collect();
                self.rule.eval_state(&c, &a, &mut out);
            }
        }
        // Generations 1..=k: the rule on every cone cell (INV-STENCIL-RULE-2).
        for t in 0..s.k {
            let (x0, w) = s.cone(t + 1);
            for i in 0..s.rpr {
                for x in x0..x0 + w {
                    let nb_vals: Vec<Vec<AB::Expr>> = (0..2 * r + 1)
                        .flat_map(|dy| (0..2 * r + 1).map(move |dx| (dx, dy)))
                        .map(|(dx, dy)| (0..v).map(|j| cells.get(t, i + dy, x + dx - r, j)).collect())
                        .collect();
                    let nb: Vec<&[AB::Expr]> = nb_vals.iter().map(|c| c.as_slice()).collect();
                    let nx: Vec<AB::Expr> = (0..v).map(|j| cells.get(t + 1, i, x, j)).collect();
                    let aux: Vec<AB::Expr> = (0..s.spec.aux).map(|j| cells.get(t + 1, i, x, v + j)).collect();
                    self.rule.eval_step(&nb, &nx, &aux, &mut out);
                }
            }
        }
        for c in out {
            builder.assert_zero(c);
        }

        // The digests (INV-STENCIL-DIG-1..3).
        for g in REGIONS {
            let base = s.region_base(g);
            let mut st: Vec<AB::Expr> = (0..WIDTH).map(|j| cur[base + j].into()).collect();
            for i in 0..s.rpr {
                let vals = region_values::<AB::Expr, AB::Var>(s, g, &cells, i, &per[s.p_inner0(i)]);
                let packed = pack(s, &vals, s.perms(g));
                let lb = s.slot_base(g, i);
                for (q, p) in packed.iter().take(s.limbs(g)).enumerate() {
                    builder.assert_eq(cur[lb + q], p.clone());
                }
                let limb = |q: usize| -> AB::Expr { if q < s.limbs(g) { cur[lb + q].into() } else { AB::Expr::ZERO } };
                let mut cap: Vec<AB::Expr> = st[RATE..].to_vec();
                let mut po: [AB::Expr; WIDTH] = core::array::from_fn(|_| AB::Expr::ZERO);
                for m in 0..s.perms(g) {
                    let input: [AB::Expr; WIDTH] =
                        core::array::from_fn(|j| if j < RATE { limb(m * RATE + j) } else { cap[j - RATE].clone() });
                    let c = s.perm_col(g, i, m);
                    po = eval_permutation(builder, input, &cur[c..c + PERM_COLS]);
                    cap = po[RATE..].to_vec();
                }
                let gate = per[if g == Region::Halo { s.p_in_e(i) } else { s.p_zone(i) }].clone();
                let o = s.out_col(g, i);
                for j in 0..WIDTH {
                    let mux = gate.clone() * po[j].clone() + (AB::Expr::ONE - gate.clone()) * st[j].clone();
                    builder.assert_eq(cur[o + j], mux);
                }
                st = (0..WIDTH).map(|j| cur[o + j].into()).collect();
            }
            for j in 0..WIDTH {
                builder.when_first_row().assert_zero(cur[base + j]);
                builder.when_transition().assert_eq(next[base + j], st[j].clone());
            }
            for j in 0..DIGEST {
                builder.when_last_row().assert_eq(st[j].clone(), public[g as usize * DIGEST + j].clone());
            }
        }
    }
}

/// `x` as a canonical `u32` (tests and rules).
pub fn to_u32(x: Val) -> u32 {
    x.as_canonical_u32()
}
