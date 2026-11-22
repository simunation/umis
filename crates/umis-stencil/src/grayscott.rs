//! Toy rule 4: Gray–Scott reaction–diffusion in exact fixed point (two species, radius 1).
//!
//! Two 14-bit values per cell, `u` and `v`, in units of `U = 2^13` (so a value covers
//! `[0, 2)`). With the 5-point Laplacian `Δx = Σ4 x_nb − 4x`, diffusion `Du = du/2^s`,
//! `Dv = dv/2^s`, feed `F = f/2^s` and kill `k = kk/2^s`:
//!
//! ```text
//! u' = u + Du·Δu − u·v² + F·(1 − u)
//! v' = v + Dv·Δv + u·v² − (F + k)·v
//! ```
//!
//! The reaction `R = u·v²/U²` is computed in two exact stages, every intermediate below 2^29:
//!
//! ```text
//! v² = w·2^13 + r1,      0 ≤ r1 < 2^13, w < 2^15
//! u·w = R·2^13 + r2,     0 ≤ r2 < 2^13, R < 2^16
//! Nu = (2^s − 4du − f)·u + du·Σ4u + f·U − 2^s·R + 2^(s−1) = u'·2^s + ru,  0 ≤ ru < 2^s
//! Nv = (2^s − 4dv − f − kk)·v + dv·Σ4v + 2^s·R + 2^(s−1) = v'·2^s + rv,  0 ≤ rv < 2^s
//! ```
//!
//! (nearest rounding). Each equation's two sides are integers of magnitude below `p/2`, so the
//! field equation is the integer one and every helper and the next values are unique given
//! their ranges (RULE-2). `u' ≥ 0` needs `R ≤ u` roughly (true in the physical regime); a state
//! where a next value leaves `[0, 2^14)` cannot be proved ([`GrayScott::provable`]). Degree 2.
//! Range requests: `u, v` (14 bits) per generation-0 cell; `r1, r2` (13), `w` (15), `R` (16),
//! `ru, rv` (s) and `u', v'` (14) per computed cell.

use crate::{Grid, Rule, Spec, Val};
use p3_field::PrimeCharacteristicRing;

/// One in value units.
pub const U: i64 = 1 << 13;
/// Bits per value.
pub const BITS: usize = 14;
const SHIFT: usize = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GrayScott {
    pub du: i64,
    pub dv: i64,
    pub f: i64,
    pub kk: i64,
    /// Coefficients are `x / 2^s`.
    pub s: usize,
}

impl Default for GrayScott {
    /// `Du = 0.16`, `Dv = 0.08`, `F = 0.037`, `k = 0.06` (spots that split and spread).
    fn default() -> Self {
        Self::new(655, 328, 152, 246, 12)
    }
}

/// The helpers of one cell update: `(w, r1, R, r2, ru, rv)` and the next values.
struct Update {
    next: [i64; 2],
    aux: [i64; 6],
}

impl GrayScott {
    pub fn new(du: i64, dv: i64, f: i64, kk: i64, s: usize) -> Self {
        assert!((2..=13).contains(&s), "s in 2..=13");
        let one = 1i64 << s;
        assert!(du >= 0 && dv >= 0 && f >= 0 && kk >= 0, "non-negative coefficients");
        assert!(one - 4 * du - f >= 0 && one - 4 * dv - f - kk >= 0, "stable explicit scheme");
        Self { du, dv, f, kk, s }
    }

    fn update(&self, nb: &[&[u32]]) -> Update {
        let x = |c: usize, j: usize| nb[c][j] as i64;
        let (u, v) = (x(4, 0), x(4, 1));
        let su = x(1, 0) + x(3, 0) + x(5, 0) + x(7, 0);
        let sv = x(1, 1) + x(3, 1) + x(5, 1) + x(7, 1);
        let (w, r1) = ((v * v) >> SHIFT, (v * v) & ((1 << SHIFT) - 1));
        let (r, r2) = ((u * w) >> SHIFT, (u * w) & ((1 << SHIFT) - 1));
        let one = 1i64 << self.s;
        let half = one >> 1;
        let nu = (one - 4 * self.du - self.f) * u + self.du * su + self.f * U - one * r + half;
        let nv = (one - 4 * self.dv - self.f - self.kk) * v + self.dv * sv + one * r + half;
        let (u2, ru) = (nu.div_euclid(one), nu.rem_euclid(one));
        let (v2, rv) = (nv.div_euclid(one), nv.rem_euclid(one));
        Update { next: [u2, v2], aux: [w, r1, r, r2, ru, rv] }
    }

    /// Whether every cell update of the extended board `ext` stays in range (the proof then
    /// exists): next values in `[0, 2^14)`.
    pub fn provable(&self, ext: &Grid) -> bool {
        let lim = 1i64 << BITS;
        if ext.data.iter().any(|&x| x as i64 >= lim) {
            return false;
        }
        for y in 1..ext.h - 1 {
            for x in 1..ext.w - 1 {
                let nb: Vec<&[u32]> = (0..3)
                    .flat_map(|dy| (0..3).map(move |dx| (dx, dy)))
                    .map(|(dx, dy)| ext.cell(x + dx - 1, y + dy - 1))
                    .collect();
                if self.update(&nb).next.iter().any(|&n| !(0..lim).contains(&n)) {
                    return false;
                }
            }
        }
        true
    }
}

impl Rule for GrayScott {
    fn spec(&self) -> Spec {
        Spec {
            values: 2,
            bits: BITS,
            radius: 1,
            aux0: 0,
            aux: 6,
            state_constraints: 0,
            step_constraints: 4,
            state_ranges: 2,
            step_ranges: 8,
        }
    }
    /// The trivial steady state: all substrate, no catalyst.
    fn boundary(&self) -> Vec<u32> {
        vec![U as u32, 0]
    }
    fn step(&self, nb: &[&[u32]]) -> (Vec<u32>, Vec<Val>) {
        let up = self.update(nb);
        // Out-of-range next values are clamped natively; such a slot has no proof (`provable`).
        let lim = (1i64 << BITS) - 1;
        (up.next.iter().map(|&n| n.clamp(0, lim) as u32).collect(), up.aux.iter().map(|&a| Val::from_i64(a)).collect())
    }
    fn state_aux(&self, _cell: &[u32]) -> Vec<Val> {
        Vec::new()
    }
    fn eval_state<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        cell: &[E],
        _aux0: &[E],
        _out: &mut Vec<E>,
        ranges: &mut Vec<(E, usize)>,
    ) {
        ranges.push((cell[0].clone(), BITS));
        ranges.push((cell[1].clone(), BITS));
    }
    fn eval_step<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        nb: &[&[E]],
        next: &[E],
        aux: &[E],
        out: &mut Vec<E>,
        ranges: &mut Vec<(E, usize)>,
    ) {
        let c = |x: i64| E::from_i64(x);
        let (u, v) = (nb[4][0].clone(), nb[4][1].clone());
        let su = nb[1][0].clone() + nb[3][0].clone() + nb[5][0].clone() + nb[7][0].clone();
        let sv = nb[1][1].clone() + nb[3][1].clone() + nb[5][1].clone() + nb[7][1].clone();
        let [w, r1, r, r2, ru, rv] = core::array::from_fn(|j| aux[j].clone());
        let sh = c(1 << SHIFT);
        out.push(v.clone() * v.clone() - w.clone() * sh.clone() - r1.clone());
        out.push(u.clone() * w.clone() - r.clone() * sh - r2.clone());
        let one = 1i64 << self.s;
        let half = one >> 1;
        let nu = u * c(one - 4 * self.du - self.f) + su * c(self.du) + c(self.f * U + half) - r.clone() * c(one);
        out.push(nu - next[0].clone() * c(one) - ru.clone());
        let nv = v * c(one - 4 * self.dv - self.f - self.kk) + sv * c(self.dv) + r.clone() * c(one) + c(half);
        out.push(nv - next[1].clone() * c(one) - rv.clone());
        for (x, b) in [
            (r1, SHIFT),
            (w, 15),
            (r2, SHIFT),
            (r, 16),
            (ru, self.s),
            (rv, self.s),
            (next[0].clone(), BITS),
            (next[1].clone(), BITS),
        ] {
            ranges.push((x, b));
        }
    }
}
