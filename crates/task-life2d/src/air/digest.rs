//! A claim-independent life2d AIR for recursion (ROADMAP A2, INV-LIFE-DIG-*).
//!
//! [`LifeAir`](super::LifeAir) binds its claim through periodic columns (packed generation 0,
//! packed `X_out`, the absent mask), so its verifier circuit is a different circuit for every
//! slot (INV-LIFE-FS-2). [`LifeDigestAir`] proves the same `k` masked generations with
//! periodic columns that depend on the shape only, and binds the claim through public values:
//!
//! - `public[0..8]`: a Poseidon2 sponge digest of every trace row's packed generation-0 cells
//!   and packed, `sel`-gated generation-`k` inner cells ([`digest`], INV-LIFE-DIG-1). The
//!   sponge runs inside the trace: each row carries `P` width-16 permutations absorbing that
//!   row's limbs, chained row to row through the capacity (INV-LIFE-DIG-2).
//! - `public[8..16]`: the eight `absent` flags. The mask of each cell is a linear combination
//!   of shape-only band indicators with these flags (INV-LIFE-DIG-3).
//!
//! The verifier computes both from the claim (`E`, `X_out`, `absent`), as for [`LifeAir`].
//!
//! # Sponge layout (columns after the life columns)
//!
//! - `cap_in[8]`: the capacity entering the row's first permutation; `0` on row 0, and the
//!   previous row's last permutation output `[8..16]` on every other row.
//! - per permutation `m < P`: `4 × 16` full-round outputs, `20` partial-round S-box outputs,
//!   `4 × 16` full-round outputs (the layout of `p3-poseidon2-air` without its input columns;
//!   S-box degree 3, no S-box registers). Its input is `[limbs[8m..8m+8], capacity]`, the
//!   capacity being `cap_in` for `m = 0` and permutation `m − 1`'s output `[8..16]` otherwise
//!   (overwrite-mode sponge, as `p3_symmetric::PaddingFreeSponge`).
//! - The digest is the last row's last permutation output `[0..8]`.

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
const HALF_FULL: usize = 4;
const PARTIAL: usize = 20;
/// Columns of one permutation: every full-round output and every partial-round S-box output.
pub const PERM_COLS: usize = 2 * HALF_FULL * WIDTH + PARTIAL;
/// Constraints of one permutation (one per column).
const PERM_CONSTRAINTS: usize = PERM_COLS;
pub const N_PUBLIC: usize = DIGEST + 8;
/// Band indicators per stacked-row offset: `L₀ L₁ L₂ C₀ C₂ R₀ R₁ R₂` (see [`DigestShape::bands`]).
const BANDS: usize = 8;

/// The digest layout of a life shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DigestShape {
    pub life: Shape,
}

impl DigestShape {
    pub fn new(life: Shape) -> Self {
        Self { life }
    }

    /// Packed limbs per trace row: generation 0 (`rpr·Ws` cells), then the `sel`-gated
    /// generation-`k` inner cells (`rpr·w`), 30 cells per limb, in column order.
    pub fn limbs0(&self) -> usize {
        (self.life.rpr * self.life.ws).div_ceil(LIMB_BITS)
    }
    pub fn limbs_out(&self) -> usize {
        (self.life.rpr * self.life.w).div_ceil(LIMB_BITS)
    }
    /// Permutations per trace row.
    pub fn perms(&self) -> usize {
        (self.limbs0() + self.limbs_out()).div_ceil(RATE)
    }

    fn cap_in(&self) -> usize {
        self.life.width()
    }
    fn perm(&self, m: usize) -> usize {
        self.cap_in() + (WIDTH - RATE) + m * PERM_COLS
    }
    pub fn width(&self) -> usize {
        self.perm(self.perms())
    }

    // periodic columns: sel[i], then BANDS indicators per offset o ∈ [1, k + rpr)
    fn p_sel(&self, i: usize) -> usize {
        i
    }
    fn p_band(&self, o: usize, b: usize) -> usize {
        self.life.rpr + (o - 1) * BANDS + b
    }
    pub fn num_periodic(&self) -> usize {
        self.p_band(self.life.k + self.life.rpr, 0)
    }

    pub fn num_constraints(&self) -> usize {
        let s = &self.life;
        let cells: usize = (1..=s.k).map(|t| s.cone(t)).sum();
        s.rpr * (s.ws + 2 * cells) + self.perms() * PERM_CONSTRAINTS + 3 * (WIDTH - RATE)
    }

    /// Whether slot `i` of generation `k` at trace row `r` is an inner row (INV-LIFE-OUT-1).
    fn sel(&self, r: usize, i: usize) -> bool {
        let s = &self.life;
        s.locate(s.rpr * r + i + s.k).is_some_and(|(_, yy)| s.row_band(yy) == 1)
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
            }
            for o in 1..s.k + s.rpr {
                for (b, v) in self.bands(s.rpr * r + o).into_iter().enumerate() {
                    per[self.p_band(o, b)][r] = Val::from_bool(v);
                }
            }
        }
        per
    }

    /// One trace row's limbs, zero-padded to `perms() · RATE`.
    fn row_limbs(&self, ext: &Board, x_out: &Board, r: usize) -> Vec<Val> {
        let s = &self.life;
        let g0 =
            (0..s.rpr).flat_map(|i| (0..s.ws).map(move |x| (i, x))).map(|(i, x)| ext_value(s, ext, s.rpr * r + i, x));
        let out: Vec<u8> = (0..s.rpr)
            .flat_map(|i| {
                let row = s
                    .locate(s.rpr * r + i + s.k)
                    .filter(|_| self.sel(r, i))
                    .map(|(st, yy)| &x_out.cells[(yy - s.k) * s.n + st * s.w..][..s.w]);
                (0..s.w).map(move |x| row.map_or(0, |row| row[x]))
            })
            .collect();
        let g0: Vec<u8> = g0.collect();
        let mut limbs: Vec<Val> =
            g0.chunks(LIMB_BITS).chain(out.chunks(LIMB_BITS)).map(|c| super::pack_limb(c.iter().copied())).collect();
        limbs.resize(self.perms() * RATE, Val::ZERO);
        limbs
    }

    /// The port index of extended-board band `(row band, column band)`, if a port has it.
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

/// The sponge digest of a claim (INV-LIFE-DIG-1) and, if `cols` is given, every row's sponge
/// columns (`width() − life width` per row, row-major).
fn sponge(d: &DigestShape, ext: &Board, x_out: &Board, mut cols: Option<&mut [Val]>) -> [Val; DIGEST] {
    let per_row = d.width() - d.cap_in();
    let mut st = [Val::ZERO; WIDTH];
    let mut scratch = vec![Val::ZERO; per_row];
    for r in 0..d.life.height {
        let limbs = d.row_limbs(ext, x_out, r);
        let row: &mut [Val] = match cols.as_deref_mut() {
            Some(c) => &mut c[r * per_row..][..per_row],
            None => &mut scratch,
        };
        row[..WIDTH - RATE].copy_from_slice(&st[RATE..]);
        for m in 0..d.perms() {
            st[..RATE].copy_from_slice(&limbs[m * RATE..][..RATE]);
            let off = WIDTH - RATE + m * PERM_COLS;
            st = permute_into(st, &mut row[off..off + PERM_COLS]);
        }
    }
    st[..DIGEST].try_into().expect("digest")
}

/// The public values of a claim: its digest and the `absent` flags.
pub fn public_values(d: &DigestShape, ext: &Board, absent: &[bool; 8], x_out: &Board) -> Vec<Val> {
    let digest = sponge(d, ext, x_out, None);
    digest.into_iter().chain(absent.iter().map(|a| Val::from_bool(*a))).collect()
}

/// The digest AIR's trace: `life` (from [`super::generate_trace`]) with the sponge columns.
pub fn extend_trace(d: &DigestShape, life: RowMajorMatrix<Val>, ext: &Board, x_out: &Board) -> RowMajorMatrix<Val> {
    let (lw, w, h) = (d.cap_in(), d.width(), d.life.height);
    assert_eq!(life.width(), lw);
    let mut sp = Val::zero_vec(h * (w - lw));
    let _ = sponge(d, ext, x_out, Some(&mut sp));
    let mut v = Val::zero_vec(h * w);
    v.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
        row[..lw].copy_from_slice(&life.values[r * lw..][..lw]);
        row[lw..].copy_from_slice(&sp[r * (w - lw)..][..w - lw]);
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
        let absent = |by: usize, bx: usize| public[DIGEST + DigestShape::port(by, bx)].clone();

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

        // The row's limbs (INV-LIFE-DIG-1): generation 0 in column order, then the inner
        // generation-k cells of each slot, gated by sel.
        let g0 = (0..s.rpr).flat_map(|i| (0..s.ws).map(move |x| (i, x)));
        let g0: Vec<AB::Expr> = g0.map(|(i, x)| cells.get(0, i, x)).collect();
        let out: Vec<AB::Expr> = (0..s.rpr)
            .flat_map(|i| (0..s.w).map(move |x| (i, x)))
            .map(|(i, x)| per[d.p_sel(i)].clone() * cells.get::<AB::Expr>(s.k, i, s.k + x))
            .collect();
        let mut limbs: Vec<AB::Expr> = g0
            .chunks(LIMB_BITS)
            .chain(out.chunks(LIMB_BITS))
            .map(|c| pack::<AB::Expr>(&mut c.iter().cloned()))
            .collect();
        limbs.resize(d.perms() * RATE, AB::Expr::ZERO);

        // The sponge (INV-LIFE-DIG-2).
        let cap_in = d.cap_in();
        let mut cap: Vec<AB::Expr> = (0..WIDTH - RATE).map(|j| cur[cap_in + j].into()).collect();
        let mut st: [AB::Expr; WIDTH] = core::array::from_fn(|_| AB::Expr::ZERO);
        for m in 0..d.perms() {
            let input: [AB::Expr; WIDTH] =
                core::array::from_fn(|j| if j < RATE { limbs[m * RATE + j].clone() } else { cap[j - RATE].clone() });
            st = eval_permutation(builder, input, &cur[d.perm(m)..d.perm(m) + PERM_COLS]);
            cap = st[RATE..].to_vec();
        }
        for j in 0..WIDTH - RATE {
            builder.when_first_row().assert_zero(cur[cap_in + j]);
            builder.when_transition().assert_eq(next[cap_in + j], st[RATE + j].clone());
            builder.when_last_row().assert_eq(st[j].clone(), public[j].clone());
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
