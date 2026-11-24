//! Toy rule 2: explicit heat diffusion in fixed point (5-point Laplacian, ROADMAP §0 S2).
//!
//! One `bits`-bit value per cell (a temperature in `[0, 2^bits)`), radius 1. With
//! `α = a / 2^s` (`4a ≤ 2^s`, the stability limit of the explicit scheme),
//!
//! ```text
//! N  = (2^s − 4a)·u + a·(u_N + u_S + u_W + u_E)      (a convex combination times 2^s)
//! u' = ⌊N / 2^s⌋,  i.e.  N = u'·2^s + rem,  0 ≤ rem < 2^s
//! ```
//!
//! The rounding is proved with bit decompositions: `rem` (`s` bits) and `u'` (`bits` bits),
//! and every generation-0 value (`bits` bits). All integers stay below `2^(bits+s) ≪ p`, so
//! the field equation is the integer equation and `u'` is unique (RULE-2). Degree 2
//! (booleanity of the bits). The range checks dominate the columns: the reason for lookups
//! later (S5).

use crate::{Rule, Spec, Val};
use p3_field::PrimeCharacteristicRing;

#[derive(Clone, Copy, Debug)]
pub struct Heat {
    /// Bits per temperature.
    pub bits: usize,
    /// `α = a / 2^s`.
    pub a: u32,
    pub s: usize,
    /// Temperature of missing neighbours (a cold or hot wall).
    pub wall: u32,
    /// Range checks by lookups ([`crate::batch`]) instead of bit columns: the only helper is
    /// `rem`; `u`, `u'` and `rem` are range requests.
    pub lookups: bool,
}

impl Heat {
    pub fn new(bits: usize, a: u32, s: usize, wall: u32) -> Self {
        assert!(bits + s <= 28 && 4 * a as u64 <= 1 << s && (wall as u64) < 1 << bits, "heat parameters");
        Self { bits, a, s, wall, lookups: false }
    }

    /// The same rule with lookup range checks (`bits ≤ 16`).
    pub fn with_lookups(mut self) -> Self {
        assert!(self.bits <= 16 && self.s <= 16);
        self.lookups = true;
        self
    }

    fn n_of(&self, nb: &[&[u32]]) -> u64 {
        let (u, sum) = (nb[4][0] as u64, (nb[1][0] + nb[3][0] + nb[5][0] + nb[7][0]) as u64);
        ((1u64 << self.s) - 4 * self.a as u64) * u + self.a as u64 * sum
    }
}

fn bits_of(x: u64, n: usize) -> impl Iterator<Item = Val> {
    (0..n).map(move |j| Val::from_bool(x >> j & 1 == 1))
}

/// `Σ 2^j·b_j` and the booleanity of each `b_j`.
fn recompose<E: PrimeCharacteristicRing + Clone>(bits: &[E], out: &mut Vec<E>) -> E {
    for b in bits {
        out.push(b.clone() * (b.clone() - E::ONE));
    }
    bits.iter().rev().fold(E::ZERO, |acc, b| acc.double() + b.clone())
}

impl Rule for Heat {
    fn spec(&self) -> Spec {
        if self.lookups {
            return Spec {
                values: 1,
                bits: self.bits,
                radius: 1,
                aux0: 0,
                aux: 1,
                state_constraints: 0,
                step_constraints: 1,
                state_ranges: 1,
                step_ranges: 2,
            };
        }
        Spec {
            values: 1,
            bits: self.bits,
            radius: 1,
            aux0: self.bits,
            aux: self.bits + self.s,
            state_constraints: self.bits + 1,
            step_constraints: self.bits + self.s + 2,
            state_ranges: 0,
            step_ranges: 0,
        }
    }
    fn boundary(&self) -> Vec<u32> {
        vec![self.wall]
    }
    fn step(&self, nb: &[&[u32]]) -> (Vec<u32>, Vec<Val>) {
        let n = self.n_of(nb);
        let (next, rem) = (n >> self.s, n & ((1 << self.s) - 1));
        if self.lookups {
            return (vec![next as u32], vec![Val::from_u64(rem)]);
        }
        (vec![next as u32], bits_of(next, self.bits).chain(bits_of(rem, self.s)).collect())
    }
    fn state_aux(&self, cell: &[u32]) -> Vec<Val> {
        if self.lookups {
            return Vec::new();
        }
        bits_of(cell[0] as u64, self.bits).collect()
    }
    fn eval_state<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        cell: &[E],
        aux0: &[E],
        out: &mut Vec<E>,
        ranges: &mut Vec<(E, usize)>,
    ) {
        if self.lookups {
            ranges.push((cell[0].clone(), self.bits));
            return;
        }
        let v = recompose(aux0, out);
        out.push(cell[0].clone() - v);
    }
    fn eval_step<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        nb: &[&[E]],
        next: &[E],
        aux: &[E],
        out: &mut Vec<E>,
        ranges: &mut Vec<(E, usize)>,
    ) {
        let rem = if self.lookups {
            ranges.push((next[0].clone(), self.bits));
            ranges.push((aux[0].clone(), self.s));
            aux[0].clone()
        } else {
            let (vb, rb) = aux.split_at(self.bits);
            let v = recompose(vb, out);
            let rem = recompose(rb, out);
            out.push(next[0].clone() - v);
            rem
        };
        let sum = nb[1][0].clone() + nb[3][0].clone() + nb[5][0].clone() + nb[7][0].clone();
        let n = nb[4][0].clone() * E::from_u64((1u64 << self.s) - 4 * self.a as u64) + sum * E::from_u64(self.a as u64);
        out.push(n - next[0].clone() * E::from_u64(1u64 << self.s) - rem);
    }
}
