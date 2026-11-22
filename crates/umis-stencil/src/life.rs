//! Toy rule 1: Conway's Game of Life (B3/S23) on the stencil framework.
//!
//! One bit per cell, radius 1, one helper column (an inverse witness). With `c` the live
//! neighbour count and `a` the cell, `f = (c − 3)(c − 3 + a)` is a non-negative integer below
//! 31, zero exactly when the cell is alive next. The constraints `a'·f = 0` and
//! `f·w = 1 − a'` force `a' = [f = 0]` (degree 3). Missing neighbours are dead cells that
//! then evolve by the rule (the framework's ghost-cell boundary).

use crate::{Rule, Spec, Val};
use p3_field::{Field, PrimeCharacteristicRing};

#[derive(Clone, Copy, Debug, Default)]
pub struct Life;

fn f_of(nb: &[&[u32]]) -> u32 {
    let c: u32 = nb.iter().enumerate().filter(|(k, _)| *k != 4).map(|(_, x)| x[0]).sum();
    let a = nb[4][0] as i64;
    let c = c as i64;
    ((c - 3) * (c - 3 + a)) as u32
}

impl Rule for Life {
    fn spec(&self) -> Spec {
        Spec {
            values: 1,
            bits: 1,
            radius: 1,
            aux0: 0,
            aux: 1,
            state_constraints: 1,
            step_constraints: 2,
            state_ranges: 0,
            step_ranges: 0,
        }
    }
    fn boundary(&self) -> Vec<u32> {
        vec![0]
    }
    fn step(&self, nb: &[&[u32]]) -> (Vec<u32>, Vec<Val>) {
        let f = f_of(nb);
        let w = if f == 0 { Val::ZERO } else { Val::from_u32(f).inverse() };
        (vec![(f == 0) as u32], vec![w])
    }
    fn state_aux(&self, _: &[u32]) -> Vec<Val> {
        Vec::new()
    }
    fn eval_state<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        cell: &[E],
        _: &[E],
        out: &mut Vec<E>,
        _: &mut Vec<(E, usize)>,
    ) {
        out.push(cell[0].clone() * (cell[0].clone() - E::ONE));
    }
    fn eval_step<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        nb: &[&[E]],
        next: &[E],
        aux: &[E],
        out: &mut Vec<E>,
        _: &mut Vec<(E, usize)>,
    ) {
        let three = E::from_u64(3);
        let c = nb.iter().enumerate().filter(|(k, _)| *k != 4).fold(E::ZERO, |s, (_, x)| s + x[0].clone());
        let f = (c.clone() - three.clone()) * (c - three + nb[4][0].clone());
        out.push(next[0].clone() * f.clone());
        out.push(f * aux[0].clone() - (E::ONE - next[0].clone()));
    }
}
