//! Toy rule 3: a lattice Boltzmann fluid (D2Q9, BGK collision, He–Luo incompressible
//! equilibrium) in exact fixed point.
//!
//! Each cell holds the 9 post-collision distributions `g_i` (16-bit integers, unit
//! `U = 2^10` = density 1). One generation is stream + collide:
//!
//! ```text
//! f_i   = g_i(x − c_i)                                  (pull streaming, radius 1)
//! ρ     = Σ f_i,  m = Σ c_i f_i,  s_i = c_i·m,  |m|² = m_x² + m_y²
//! E_i   = 2ρU + 6U·s_i + 9·s_i² − 3·|m|²                 (= 72U·f_eq_i / W_i, W_i = 36 w_i)
//! for i = 1..8 (rounded to nearest):
//! q_i   = ⌊(W_i·E_i + 2^12) / 2^13⌋      (W_i·E_i + 2^12 = q_i·2^13 + r1_i,  r1_i < 2^13)
//! feq_i = ⌊(q_i + 4) / 9⌋                (q_i + 4 = 9·feq_i + r9_i,          r9_i < 9)
//!       ≈ W_i·E_i / 72U, the incompressible equilibrium w_i(ρ + 3s + 4.5s²/U − 1.5|m|²/U)
//! g'_i  = ⌊((2^t − a)·f_i + a·feq_i + 2^(t−1)) / 2^t⌋   (relaxation ω = a / 2^t, r2_i < 2^t)
//! g'_0  = ρ − Σ_{i≥1} g'_i                 (mass is conserved exactly)
//! ```
//!
//! Every quantity is range-checked by lookups, so each equation is an equation over the
//! integers (no wrap below `p`) and every result is unique (RULE-2): `f, g', feq, q` 16 bits,
//! `r1` 13, `r9 ∈ [0, 9)` (two checks), `r2` `t` bits, `ρ < 2^13`, `|m_x|, |m_y| < 512`. The last
//! two encode the physical regime (density about 1, velocity below 0.5 lattice units): a state
//! outside it cannot be proved, which is a statement about the simulation, not a hole. Worst-case
//! magnitudes: `|W·E| < 2^29`, `|q·2^13 + r1| < 2^29`, `|N2| < 2^21`, all `≪ p ≈ 2^31`.
//!
//! The rule's own degree is 2 (`s_i²`, `|m|²`); 25 constraints and 60 range requests per cell.
//! Missing neighbours are fluid at rest ([`Lbm::equilibrium`] of density 1), an exact fixed
//! point of the rule, evolving like any cell.

use crate::{Rule, Spec, Val};
use p3_field::PrimeCharacteristicRing;

/// Lattice velocities `c_i` (x right, y down) and `W_i = 36·w_i`.
pub const C: [(i64, i64); 9] = [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1), (-1, -1), (1, -1)];
pub const W: [i64; 9] = [16, 4, 4, 4, 4, 1, 1, 1, 1];
/// Density 1 in fixed point.
pub const U: i64 = 1 << 10;
const SHIFT1: usize = 13; // 72U = 9·2^13
const M_MAX: i64 = 512; // |m_x|, |m_y| < 512
const RHO_BITS: usize = 13;

#[derive(Clone, Copy, Debug)]
pub struct Lbm {
    /// `ω = a / 2^t` (stable for `0 < ω < 2`).
    pub a: i64,
    pub t: usize,
}

impl Default for Lbm {
    /// `ω = 3/2` (τ = 2/3, kinematic viscosity (τ − 1/2)/3 ≈ 0.056).
    fn default() -> Self {
        Self { a: 24, t: 4 }
    }
}

/// `(feq_i, q_i, r1_i, r9_i)` for `i ≥ 1`.
fn feq(rho: i64, m: (i64, i64), i: usize) -> (i64, i64, i64, i64) {
    let s = C[i].0 * m.0 + C[i].1 * m.1;
    let e = 2 * rho * U + 6 * U * s + 9 * s * s - 3 * (m.0 * m.0 + m.1 * m.1);
    let n1 = W[i] * e + (1 << (SHIFT1 - 1));
    let (q, r1) = (n1.div_euclid(1 << SHIFT1), n1.rem_euclid(1 << SHIFT1));
    let (f, r9) = ((q + 4).div_euclid(9), (q + 4).rem_euclid(9));
    (f, q, r1, r9)
}

/// The neighbourhood slot (row-major 3×3) streaming direction `i` pulls from: `x − c_i`.
fn source(i: usize) -> usize {
    let (cx, cy) = C[i];
    ((1 - cy) * 3 + (1 - cx)) as usize
}

impl Lbm {
    pub fn new(a: i64, t: usize) -> Self {
        assert!(a > 0 && a < 2 << t && (1..=8).contains(&t), "0 < ω < 2");
        Self { a, t }
    }

    /// The equilibrium of density `rho` and momentum `m` (in fixed point), rounded as the rule
    /// rounds it, with `f_0` closing the mass exactly.
    pub fn equilibrium(rho: i64, m: (i64, i64)) -> [u32; 9] {
        let mut f = [0u32; 9];
        for i in 1..9 {
            f[i] = feq(rho, m, i).0 as u32;
        }
        f[0] = (rho - f[1..].iter().map(|x| *x as i64).sum::<i64>()) as u32;
        f
    }

    /// Density and momentum of a cell.
    pub fn moments(g: &[u32]) -> (i64, i64, i64) {
        let rho = g.iter().map(|x| *x as i64).sum();
        let mx = (0..9).map(|i| C[i].0 * g[i] as i64).sum();
        let my = (0..9).map(|i| C[i].1 * g[i] as i64).sum();
        (rho, mx, my)
    }
}

impl Rule for Lbm {
    fn spec(&self) -> Spec {
        Spec {
            values: 9,
            bits: 16,
            radius: 1,
            aux0: 0,
            aux: 5 * 8,
            state_constraints: 0,
            step_constraints: 3 * 8 + 1,
            state_ranges: 9,
            step_ranges: 7 * 8 + 4,
        }
    }

    fn boundary(&self) -> Vec<u32> {
        Lbm::equilibrium(U, (0, 0)).to_vec()
    }

    fn step(&self, nb: &[&[u32]]) -> (Vec<u32>, Vec<Val>) {
        let f: [i64; 9] = core::array::from_fn(|i| nb[source(i)][i] as i64);
        let rho: i64 = f.iter().sum();
        let mx: i64 = (0..9).map(|i| C[i].0 * f[i]).sum();
        let my: i64 = (0..9).map(|i| C[i].1 * f[i]).sum();
        let (mut next, mut aux) = (vec![0u32; 9], Vec::with_capacity(40));
        let mut rest = rho;
        for i in 1..9 {
            let (fe, q, r1, r9) = feq(rho, (mx, my), i);
            let n2 = ((1 << self.t) - self.a) * f[i] + self.a * fe + (1 << (self.t - 1));
            let (g, r2) = (n2.div_euclid(1 << self.t), n2.rem_euclid(1 << self.t));
            rest -= g;
            // Outside the physical regime values leave their ranges and the slot cannot be
            // proved; the native value is still deterministic.
            next[i] = g.clamp(0, u32::MAX as i64) as u32;
            aux.extend([q, r1, r9, fe, r2].map(Val::from_i64));
        }
        next[0] = rest.clamp(0, u32::MAX as i64) as u32;
        (next, aux)
    }

    fn state_aux(&self, _: &[u32]) -> Vec<Val> {
        Vec::new()
    }

    fn eval_state<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        cell: &[E],
        _: &[E],
        _: &mut Vec<E>,
        ranges: &mut Vec<(E, usize)>,
    ) {
        ranges.extend(cell.iter().map(|g| (g.clone(), 16)));
    }

    fn eval_step<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        nb: &[&[E]],
        next: &[E],
        aux: &[E],
        out: &mut Vec<E>,
        ranges: &mut Vec<(E, usize)>,
    ) {
        let k = |x: i64| E::from_i64(x);
        let f: Vec<E> = (0..9).map(|i| nb[source(i)][i].clone()).collect();
        let rho = f.iter().fold(E::ZERO, |a, x| a + x.clone());
        let mx = (0..9).fold(E::ZERO, |a, i| a + f[i].clone() * k(C[i].0));
        let my = (0..9).fold(E::ZERO, |a, i| a + f[i].clone() * k(C[i].1));
        ranges.push((rho.clone(), RHO_BITS));
        ranges.push((mx.clone() + k(M_MAX), 10));
        ranges.push((my.clone() + k(M_MAX), 10));
        let msq = mx.clone() * mx.clone() + my.clone() * my.clone();
        let mut rest = rho.clone();
        for i in 1..9 {
            let a = &aux[5 * (i - 1)..5 * i];
            let (q, r1, r9, feq, r2) = (&a[0], &a[1], &a[2], &a[3], &a[4]);
            let s = mx.clone() * k(C[i].0) + my.clone() * k(C[i].1);
            let e = rho.clone() * k(2 * U) + s.clone() * k(6 * U) + s.clone() * s * k(9) - msq.clone() * k(3);
            out.push(e * k(W[i]) + k(1 << (SHIFT1 - 1)) - q.clone() * k(1 << SHIFT1) - r1.clone());
            out.push(q.clone() + k(4) - feq.clone() * k(9) - r9.clone());
            let n2 = f[i].clone() * k((1 << self.t) - self.a) + feq.clone() * k(self.a) + k(1 << (self.t - 1));
            out.push(n2 - next[i].clone() * k(1 << self.t) - r2.clone());
            rest -= next[i].clone();
            ranges.push((q.clone(), 16));
            ranges.push((r1.clone(), SHIFT1));
            ranges.push((r9.clone(), 4));
            ranges.push((k(8) - r9.clone(), 4));
            ranges.push((feq.clone(), 16));
            ranges.push((r2.clone(), self.t));
            ranges.push((next[i].clone(), 16));
        }
        out.push(next[0].clone() - rest);
        ranges.push((next[0].clone(), 16));
    }
}
