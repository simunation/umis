//! A claim-independent life2d AIR for recursion (ROADMAP A2/A3, INV-LIFE-DIG-*).
//!
//! [`LifeAir`](super::LifeAir) binds its claim through periodic columns (packed generation 0,
//! packed `X_out`, the absent mask), so its verifier circuit is a different circuit for every
//! slot (INV-LIFE-FS-2). [`LifeDigestAir`] proves the same `k` masked generations with
//! periodic columns that depend on the shape only, and binds the claim through public values
//! (INV-LIFE-DIG-1):
//!
//! - `public[0..8]` `d_prev`: the Poseidon2 [`board_root`] of the previous board (the inner
//!   generation-0 cells);
//! - `public[8..16]` `d_halo`: the digest of every other generation-0 cell (the halo, the strip
//!   copies and the padding; [`halo_digest`]);
//! - `public[16..24]` `d_next`: the [`board_root`] of `X_out` (the inner generation-`k` cells);
//! - `public[24..32]`: the eight `absent` flags. The mask of each cell is a linear combination
//!   of shape-only band indicators with these flags (INV-LIFE-DIG-3).
//!
//! `d_prev` and `d_next` absorb the same board positions (a board row's cells at the same
//! trace row and slot), so consecutive slots of a zone chain: `d_next(t) = d_prev(t + 1)` iff
//! the boards are equal (INV-LIFE-DIG-5). With one strip per zone (`strips = 1`) the halo
//! digest covers only the 8 neighbour regions and the padding, so a checker can recompute it
//! from the neighbours' face payloads without any state.
//!
//! # Sponge layout (columns after the life columns, per digest in the order above)
//!
//! - `cap_in[8]`: the capacity entering the row's first permutation; `0` on row 0, and the
//!   previous row's last permutation output `[8..16]` on every other row.
//! - per permutation `m < P`: `4 × 16` full-round outputs, `20` partial-round S-box outputs,
//!   `4 × 16` full-round outputs (the layout of `p3-poseidon2-air` without its input columns;
//!   S-box degree 3, no S-box registers). Its input is `[limbs[8m..8m+8], capacity]`, the
//!   capacity being `cap_in` for `m = 0` and permutation `m − 1`'s output `[8..16]` otherwise
//!   (overwrite-mode sponge, as `p3_symmetric::PaddingFreeSponge`).
//! - The digest is the last row's last permutation output `[0..8]` (INV-LIFE-DIG-2).

use super::{Cells, LIMB_BITS, Shape, Val, eval_gen0_boolean, eval_generations, ext_value, pack};
use crate::PORTS;
use crate::board::Board;
use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_koala_bear::{
    GenericPoseidon2LinearLayersKoalaBear as Layers, KOALABEAR_POSEIDON2_RC_16_EXTERNAL_FINAL as RC_FINAL,
    KOALABEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL as RC_INITIAL, KOALABEAR_POSEIDON2_RC_16_INTERNAL as RC_PARTIAL,
};
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use p3_maybe_rayon::prelude::*;
use p3_poseidon2::GenericPoseidon2LinearLayers;
use std::borrow::Cow;
use umis_task::topology::port_dir;

const WIDTH: usize = 16;
pub const RATE: usize = 8;
pub const DIGEST: usize = 8;
const CAP: usize = WIDTH - RATE;
const HALF_FULL: usize = 4;
const PARTIAL: usize = 20;
/// Columns of one permutation: every full-round output and every partial-round S-box output.
pub const PERM_COLS: usize = 2 * HALF_FULL * WIDTH + PARTIAL;
/// Constraints of one permutation (one per column).
const PERM_CONSTRAINTS: usize = PERM_COLS;
/// Offset of the `absent` flags in the public values.
pub const ABSENT: usize = 3 * DIGEST;
pub const N_PUBLIC: usize = ABSENT + 8;
/// Band indicators per stacked-row offset: `L₀ L₁ L₂ C₀ C₂ R₀ R₁ R₂` (see [`DigestShape::bands`]).
const BANDS: usize = 8;

/// The three digests of a claim, in public-value order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Prev = 0,
    Halo = 1,
    Next = 2,
}
pub const REGIONS: [Region; 3] = [Region::Prev, Region::Halo, Region::Next];

/// The digest layout of a life shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DigestShape {
    pub life: Shape,
}

impl DigestShape {
    /// Panics unless `k ≤ rpr`: `d_prev` reads generation 0 `k` rows down, at most one trace
    /// row ahead.
    pub fn new(life: Shape) -> Self {
        assert!(Self::supports(&life), "the digest AIR needs gens ≤ rows_per_trace_row");
        Self { life }
    }

    pub fn supports(life: &Shape) -> bool {
        life.k <= life.rpr
    }

    /// Cells absorbed per trace row: `rpr·w` for the boards, `rpr·Ws` for the halo.
    fn cells(&self, g: Region) -> usize {
        let s = &self.life;
        match g {
            Region::Halo => s.rpr * s.ws,
            _ => s.rpr * s.w,
        }
    }
    /// Permutations per trace row of digest `g` (30 cells per limb, 8 limbs per permutation).
    pub fn perms(&self, g: Region) -> usize {
        self.cells(g).div_ceil(LIMB_BITS).div_ceil(RATE)
    }
    fn region_cols(&self, g: Region) -> usize {
        CAP + self.perms(g) * PERM_COLS
    }
    /// First column of digest `g` (its `cap_in`).
    fn base(&self, g: Region) -> usize {
        self.life.width() + REGIONS[..g as usize].iter().map(|r| self.region_cols(*r)).sum::<usize>()
    }
    fn perm(&self, g: Region, m: usize) -> usize {
        self.base(g) + CAP + m * PERM_COLS
    }
    pub fn width(&self) -> usize {
        self.base(Region::Next) + self.region_cols(Region::Next)
    }

    // periodic columns: sel[i], sel0[i], then BANDS indicators per offset o ∈ [1, k + rpr)
    fn p_sel(&self, i: usize) -> usize {
        i
    }
    fn p_sel0(&self, i: usize) -> usize {
        self.life.rpr + i
    }
    fn p_band(&self, o: usize, b: usize) -> usize {
        2 * self.life.rpr + (o - 1) * BANDS + b
    }
    pub fn num_periodic(&self) -> usize {
        self.p_band(self.life.k + self.life.rpr, 0)
    }

    pub fn num_constraints(&self) -> usize {
        let s = &self.life;
        let cells: usize = (1..=s.k).map(|t| s.cone(t)).sum();
        let perms: usize = REGIONS.iter().map(|g| self.perms(*g)).sum();
        s.rpr * (s.ws + 2 * cells) + perms * PERM_CONSTRAINTS + REGIONS.len() * 3 * CAP
    }

    /// Whether stacked row `rpr·r + i + k` is an inner row: slot `i` of generation `k` at trace
    /// row `r` (INV-LIFE-OUT-1), and the generation-0 row that `d_prev` reads there.
    fn sel(&self, r: usize, i: usize) -> bool {
        let s = &self.life;
        s.locate(s.rpr * r + i + s.k).is_some_and(|(_, yy)| s.row_band(yy) == 1)
    }

    /// Whether stacked row `rpr·r + i` (slot `i` of generation 0 at trace row `r`) is inner.
    fn sel0(&self, r: usize, i: usize) -> bool {
        let s = &self.life;
        s.locate(s.rpr * r + i).is_some_and(|(_, yy)| s.row_band(yy) == 1)
    }

    /// The band indicators of stacked row `y`: `L_b = [strip 0 ∧ row band b]`,
    /// `C_b = [row band b]` (b ∈ {0, 2}), `R_b = [last strip ∧ row band b]`.
    fn bands(&self, y: usize) -> [bool; BANDS] {
        let s = &self.life;
        let Some((st, yy)) = s.locate(y) else { return [false; BANDS] };
        let rb = s.row_band(yy);
        let (first, last) = (st == 0, st + 1 == s.strips);
        [
            first && rb == 0,
            first && rb == 1,
            first && rb == 2,
            rb == 0,
            rb == 2,
            last && rb == 0,
            last && rb == 1,
            last && rb == 2,
        ]
    }

    /// The shape-only periodic columns.
    pub fn periodic(&self) -> Vec<Vec<Val>> {
        let s = &self.life;
        let mut per = vec![vec![Val::ZERO; s.height]; self.num_periodic()];
        for r in 0..s.height {
            for i in 0..s.rpr {
                per[self.p_sel(i)][r] = Val::from_bool(self.sel(r, i));
                per[self.p_sel0(i)][r] = Val::from_bool(self.sel0(r, i));
            }
            for o in 1..s.k + s.rpr {
                for (b, v) in self.bands(s.rpr * r + o).into_iter().enumerate() {
                    per[self.p_band(o, b)][r] = Val::from_bool(v);
                }
            }
        }
        per
    }

    /// The cells digest `g` absorbs at trace row `r` (`cur`, and `next` for `d_prev`), in
    /// absorption order: the native counterpart of the AIR's limbs (INV-LIFE-DIG-1).
    fn row_cells<E: PrimeCharacteristicRing + From<V>, V: Copy>(
        &self,
        g: Region,
        cells: &Cells<'_, V>,
        sel: &dyn Fn(usize) -> E,
        sel0: &dyn Fn(usize) -> E,
    ) -> Vec<E> {
        let s = &self.life;
        let mut out = Vec::with_capacity(self.cells(g));
        for i in 0..s.rpr {
            match g {
                Region::Prev => out.extend((0..s.w).map(|x| sel(i) * cells.get::<E>(0, i + s.k, s.k + x))),
                Region::Next => out.extend((0..s.w).map(|x| sel(i) * cells.get::<E>(s.k, i, s.k + x))),
                Region::Halo => out.extend((0..s.ws).map(|x| {
                    let c = cells.get::<E>(0, i, x);
                    if (s.k..s.k + s.w).contains(&x) { (E::ONE - sel0(i)) * c } else { c }
                })),
            }
        }
        out
    }

    /// The port index of extended-board band `(row band, column band)`.
    fn port(by: usize, bx: usize) -> usize {
        PORTS
            .iter()
            .position(|p| {
                let d = port_dir(*p).expect("port");
                (d[1] + 1) as usize == by && (d[0] + 1) as usize == bx
            })
            .expect("every non-centre band is a port")
    }
}

/// Packs cells 30 per limb and zero-pads to whole permutations.
fn limbs<E: PrimeCharacteristicRing + Clone>(cells: &[E], perms: usize) -> Vec<E> {
    let mut l: Vec<E> = cells.chunks(LIMB_BITS).map(|c| pack::<E>(&mut c.iter().cloned())).collect();
    l.resize(perms * RATE, E::ZERO);
    l
}

/// The permutation of `input` with every round's committed values in `cols` (`PERM_COLS`);
/// returns the output state. The same rounds as `Poseidon2KoalaBear<16>`.
fn permute_into(input: [Val; WIDTH], cols: &mut [Val]) -> [Val; WIDTH] {
    let mut st = input;
    Layers::external_linear_layer(&mut st);
    let mut c = 0;
    let full = |st: &mut [Val; WIDTH], rc: &[Val; WIDTH], cols: &mut [Val], c: &mut usize| {
        for (x, r) in st.iter_mut().zip(rc) {
            *x = (*x + *r).cube();
        }
        Layers::external_linear_layer(st);
        cols[*c..*c + WIDTH].copy_from_slice(st);
        *c += WIDTH;
    };
    for rc in &RC_INITIAL {
        full(&mut st, rc, cols, &mut c);
    }
    for rc in &RC_PARTIAL {
        st[0] = (st[0] + *rc).cube();
        cols[c] = st[0];
        c += 1;
        Layers::internal_linear_layer(&mut st);
    }
    for rc in &RC_FINAL {
        full(&mut st, rc, cols, &mut c);
    }
    st
}

/// Constraints of one permutation of `input` whose round values are `cols`; returns its
/// output (the last full-round columns).
fn eval_permutation<AB: AirBuilder<F = Val>>(
    builder: &mut AB,
    input: [AB::Expr; WIDTH],
    cols: &[AB::Var],
) -> [AB::Expr; WIDTH] {
    let mut st = input;
    Layers::external_linear_layer(&mut st);
    let mut c = 0;
    let full = |st: &mut [AB::Expr; WIDTH], rc: &[Val; WIDTH], builder: &mut AB, c: &mut usize| {
        for (x, r) in st.iter_mut().zip(rc) {
            *x = (x.clone() + AB::Expr::from(*r)).cube();
        }
        Layers::external_linear_layer(st);
        for (j, x) in st.iter_mut().enumerate() {
            builder.assert_eq(x.clone(), cols[*c + j]);
            *x = cols[*c + j].into();
        }
        *c += WIDTH;
    };
    for rc in &RC_INITIAL {
        full(&mut st, rc, builder, &mut c);
    }
    for rc in &RC_PARTIAL {
        let x = (st[0].clone() + AB::Expr::from(*rc)).cube();
        builder.assert_eq(x, cols[c]);
        st[0] = cols[c].into();
        c += 1;
        Layers::internal_linear_layer(&mut st);
    }
    for rc in &RC_FINAL {
        full(&mut st, rc, builder, &mut c);
    }
    st
}

/// Runs digest `g`'s sponge over `rows` (each row's limbs); fills `cols` (the region's
/// columns, `region_cols(g)` per row) when given.
fn sponge(d: &DigestShape, g: Region, rows: &[Vec<Val>], mut cols: Option<&mut [Val]>) -> [Val; DIGEST] {
    let per_row = d.region_cols(g);
    let mut st = [Val::ZERO; WIDTH];
    let mut scratch = vec![Val::ZERO; per_row];
    for (r, limbs) in rows.iter().enumerate() {
        let row: &mut [Val] = match cols.as_deref_mut() {
            Some(c) => &mut c[r * per_row..][..per_row],
            None => &mut scratch,
        };
        row[..CAP].copy_from_slice(&st[RATE..]);
        for m in 0..d.perms(g) {
            st[..RATE].copy_from_slice(&limbs[m * RATE..][..RATE]);
            let off = CAP + m * PERM_COLS;
            st = permute_into(st, &mut row[off..off + PERM_COLS]);
        }
    }
    st[..DIGEST].try_into().expect("digest")
}

/// A board's limbs per trace row as `d_prev`/`d_next` absorb them.
fn board_rows(d: &DigestShape, board: &Board) -> Vec<Vec<Val>> {
    let s = &d.life;
    (0..s.height)
        .map(|r| {
            let mut cells = Vec::with_capacity(d.cells(Region::Next));
            for i in 0..s.rpr {
                let row = s
                    .locate(s.rpr * r + i + s.k)
                    .filter(|_| d.sel(r, i))
                    .map(|(st, yy)| &board.cells[(yy - s.k) * s.n + st * s.w..][..s.w]);
                cells.extend((0..s.w).map(|x| Val::from_u8(row.map_or(0, |row| row[x]))));
            }
            limbs(&cells, d.perms(Region::Next))
        })
        .collect()
}

/// The Poseidon2 root of a zone board (`n × n`) under the digest layout: `d_prev` of a slot
/// that starts from `board`, and `d_next` of one that ends in it (P2 dual roots: the
/// Poseidon2 root next to the keccak state root; it depends on the zone's layout params).
pub fn board_root(d: &DigestShape, board: &Board) -> [Val; DIGEST] {
    sponge(d, Region::Next, &board_rows(d, board), None)
}

/// The digest of every generation-0 cell outside the zone board (`E`'s halo, strip copies
/// and padding).
pub fn halo_digest(d: &DigestShape, ext: &Board) -> [Val; DIGEST] {
    let s = &d.life;
    let rows: Vec<Vec<Val>> = (0..s.height)
        .map(|r| {
            let mut cells = Vec::with_capacity(d.cells(Region::Halo));
            for i in 0..s.rpr {
                let inner = d.sel0(r, i);
                cells.extend((0..s.ws).map(|x| {
                    let v = ext_value(s, ext, s.rpr * r + i, x);
                    Val::from_u8(if inner && (s.k..s.k + s.w).contains(&x) { 0 } else { v })
                }));
            }
            limbs(&cells, d.perms(Region::Halo))
        })
        .collect();
    sponge(d, Region::Halo, &rows, None)
}

/// The inner `n × n` board of an extended board.
pub fn inner(n: usize, k: usize, ext: &Board) -> Board {
    let mut b = Board::dead(n, n);
    for y in 0..n {
        b.cells[y * n..][..n].copy_from_slice(&ext.cells[(y + k) * ext.w + k..][..n]);
    }
    b
}

/// The public values of a claim: `d_prev`, `d_halo`, `d_next` and the `absent` flags.
pub fn public_values(d: &DigestShape, ext: &Board, absent: &[bool; 8], x_out: &Board) -> Vec<Val> {
    let s = &d.life;
    let prev = board_root(d, &inner(s.n, s.k, ext));
    let halo = halo_digest(d, ext);
    let next = board_root(d, x_out);
    prev.into_iter().chain(halo).chain(next).chain(absent.iter().map(|a| Val::from_bool(*a))).collect()
}

/// Every digest's limbs per row, read from a trace's own cells (`life` columns): exactly what
/// the AIR absorbs.
fn trace_rows(d: &DigestShape, g: Region, life: &RowMajorMatrix<Val>) -> Vec<Vec<Val>> {
    let s = &d.life;
    let w = life.width();
    (0..s.height)
        .into_par_iter()
        .map(|r| {
            let cur = &life.values[r * w..][..w];
            let next = &life.values[((r + 1) % s.height) * w..][..w];
            let cells = Cells::new(s, cur, next);
            let sel = |i: usize| Val::from_bool(d.sel(r, i));
            let sel0 = |i: usize| Val::from_bool(d.sel0(r, i));
            limbs(&d.row_cells::<Val, Val>(g, &cells, &sel, &sel0), d.perms(g))
        })
        .collect()
}

/// The digest AIR's trace: `life` (from [`super::generate_trace`], or any life trace) with the
/// sponge columns over its own cells.
pub fn extend_trace(d: &DigestShape, life: RowMajorMatrix<Val>) -> RowMajorMatrix<Val> {
    let (lw, w, h) = (d.life.width(), d.width(), d.life.height);
    assert_eq!(life.width(), lw);
    let blocks: Vec<Vec<Val>> = REGIONS
        .iter()
        .map(|g| {
            let mut b = Val::zero_vec(h * d.region_cols(*g));
            let _ = sponge(d, *g, &trace_rows(d, *g, &life), Some(&mut b));
            b
        })
        .collect();
    let mut v = Val::zero_vec(h * w);
    v.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
        row[..lw].copy_from_slice(&life.values[r * lw..][..lw]);
        for (g, b) in REGIONS.iter().zip(&blocks) {
            let rc = d.region_cols(*g);
            let o = d.base(*g);
            row[o..o + rc].copy_from_slice(&b[r * rc..][..rc]);
        }
    });
    RowMajorMatrix::new(v, w)
}

/// The claim-independent life AIR of a shape.
#[derive(Clone)]
pub struct LifeDigestAir {
    pub shape: DigestShape,
    periodic: Vec<Vec<Val>>,
}

impl LifeDigestAir {
    /// The constraint degree Plonky3 derives symbolically (periodic columns count as degree
    /// 1, so `sel · cell` sponge inputs make the first S-box degree 6).
    pub fn symbolic_degree(&self) -> usize {
        use p3_air::symbolic::{AirLayout, get_max_constraint_degree};
        let layout = AirLayout {
            main_width: self.shape.width(),
            num_public_values: N_PUBLIC,
            num_periodic_columns: self.shape.num_periodic(),
            ..Default::default()
        };
        get_max_constraint_degree::<Val, _>(self, layout, self.shape.life.height)
    }

    /// Panics unless [`DigestShape::supports`] the shape.
    pub fn new(life: Shape) -> Self {
        let shape = DigestShape::new(life);
        Self { shape, periodic: shape.periodic() }
    }
}

impl BaseAir<Val> for LifeDigestAir {
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
    fn max_constraint_degree(&self) -> Option<usize> {
        // A lower bound only: Plonky3 takes the larger of this and the symbolic degree (6).
        Some(3)
    }
}

impl<AB: AirBuilder<F = Val>> Air<AB> for LifeDigestAir {
    fn eval(&self, builder: &mut AB) {
        let d = self.shape;
        let s = d.life;
        let main = builder.main();
        let (cur, next) = (main.current_slice(), main.next_slice());
        let cells = Cells::new(&s, cur, next);
        let mut per: Vec<AB::Expr> = builder.periodic_values().iter().map(|x| (*x).into()).collect();
        if per.is_empty() {
            // See `LifeAir::eval`: Plonky3-recursion's interaction probe omits periodic columns.
            per = vec![AB::Expr::ZERO; d.num_periodic()];
        }
        let public: Vec<AB::Expr> = builder.public_values().iter().map(|x| (*x).into()).collect();
        let absent = |by: usize, bx: usize| public[ABSENT + DigestShape::port(by, bx)].clone();

        for i in 0..s.rpr {
            eval_gen0_boolean(builder, &cells, i);
        }
        // m(o, class) from the band indicators and the public flags (INV-LIFE-DIG-3).
        eval_generations(builder, &cells, |o, c| {
            let b = |j: usize| per[d.p_band(o, j)].clone();
            let (l0, l1, l2, c0, c2, r0, r1, r2) = (b(0), b(1), b(2), b(3), b(4), b(5), b(6), b(7));
            match c {
                1 => c0 * absent(0, 1) + c2 * absent(2, 1),
                0 => {
                    l0.clone() * absent(0, 0)
                        + l1 * absent(1, 0)
                        + l2.clone() * absent(2, 0)
                        + (c0 - l0) * absent(0, 1)
                        + (c2 - l2) * absent(2, 1)
                }
                _ => {
                    r0.clone() * absent(0, 2)
                        + r1 * absent(1, 2)
                        + r2.clone() * absent(2, 2)
                        + (c0 - r0) * absent(0, 1)
                        + (c2 - r2) * absent(2, 1)
                }
            }
        });

        // The three sponges (INV-LIFE-DIG-1, -2).
        let sel = |i: usize| per[d.p_sel(i)].clone();
        let sel0 = |i: usize| per[d.p_sel0(i)].clone();
        for g in REGIONS {
            let cells_g = d.row_cells::<AB::Expr, AB::Var>(g, &cells, &sel, &sel0);
            let limbs = limbs(&cells_g, d.perms(g));
            let cap_in = d.base(g);
            let mut cap: Vec<AB::Expr> = (0..CAP).map(|j| cur[cap_in + j].into()).collect();
            let mut st: [AB::Expr; WIDTH] = core::array::from_fn(|_| AB::Expr::ZERO);
            for m in 0..d.perms(g) {
                let input: [AB::Expr; WIDTH] =
                    core::array::from_fn(
                        |j| if j < RATE { limbs[m * RATE + j].clone() } else { cap[j - RATE].clone() },
                    );
                st = eval_permutation(builder, input, &cur[d.perm(g, m)..d.perm(g, m) + PERM_COLS]);
                cap = st[RATE..].to_vec();
            }
            let out = g as usize * DIGEST;
            for j in 0..CAP {
                builder.when_first_row().assert_zero(cur[cap_in + j]);
                builder.when_transition().assert_eq(next[cap_in + j], st[RATE + j].clone());
                builder.when_last_row().assert_eq(st[j].clone(), public[out + j].clone());
            }
        }
    }
}

/// Whether band `(by, bx)` is absent according to `absent`: the native counterpart of the
/// AIR's mask, for tests.
#[cfg(test)]
pub(crate) fn mask_native(d: &DigestShape, absent: &[bool; 8], y: usize, c: usize) -> bool {
    let s = &d.life;
    match s.locate(y) {
        Some((st, yy)) => super::band_absent(absent, s.row_band(yy), s.class_band(st, c)),
        None => false,
    }
}

#[cfg(test)]
#[path = "digest_tests.rs"]
mod tests;

#[cfg(test)]
mod perm_tests {
    use super::*;
    use p3_symmetric::Permutation;

    #[test]
    fn permutation_columns_match_the_native_permutation() {
        let perm = p3_koala_bear::default_koalabear_poseidon2_16();
        let input: [Val; WIDTH] = core::array::from_fn(|i| Val::from_u32(7 * i as u32 + 1));
        let mut cols = [Val::ZERO; PERM_COLS];
        assert_eq!(permute_into(input, &mut cols), perm.permute(input));
    }

    #[test]
    fn the_band_mask_equals_the_periodic_mask() {
        // Every (offset, class) of every row on several layouts, every single-port absence.
        for (n, k, strips, rpr) in [(64, 1, 4, 2), (64, 3, 4, 3), (32, 2, 1, 2), (256, 1, 8, 3)] {
            let mut p = crate::Life2dParams::default_for([1, 1, 0], 4);
            (p.n, p.gens, p.strips, p.rows_per_trace_row) = (n, k, strips, rpr);
            let d = DigestShape::new(Shape::of(&p));
            let s = d.life;
            for bits in 0..256u32 {
                let absent: [bool; 8] = core::array::from_fn(|i| bits >> i & 1 == 1);
                for r in 0..s.height {
                    for o in 1..s.k + s.rpr {
                        let y = s.rpr * r + o;
                        let bands = d.bands(y);
                        for c in 0..3 {
                            let a = |by, bx| absent[DigestShape::port(by, bx)] as i32;
                            let b = |j: usize| bands[j] as i32;
                            let m = match c {
                                1 => b(3) * a(0, 1) + b(4) * a(2, 1),
                                0 => {
                                    b(0) * a(0, 0)
                                        + b(1) * a(1, 0)
                                        + b(2) * a(2, 0)
                                        + (b(3) - b(0)) * a(0, 1)
                                        + (b(4) - b(2)) * a(2, 1)
                                }
                                _ => {
                                    b(5) * a(0, 2)
                                        + b(6) * a(1, 2)
                                        + b(7) * a(2, 2)
                                        + (b(3) - b(5)) * a(0, 1)
                                        + (b(4) - b(7)) * a(2, 1)
                                }
                            };
                            assert_eq!(m == 1, mask_native(&d, &absent, y, c), "{n},{k},{strips},{rpr} y {y} c {c}");
                            assert!(m == 0 || m == 1);
                        }
                    }
                }
            }
        }
    }
}
