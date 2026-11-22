//! A generic local-rule (stencil) AIR framework (ROADMAP §0 S1; native only).
//!
//! A simulation implements [`Rule`]: the values of one cell, how many bits each value has,
//! the neighbourhood radius, the native update and the same update as polynomial
//! constraints. The framework proves `k` generations of a zone (`n × n` cells) from its
//! extended board `E` (the zone plus a `k·r` halo from its neighbours) and binds the claim
//! by three Poseidon2 digests in the public values ([`air::StencilAir`]):
//!
//! - `d_prev`: the [`root`](air::root) of the zone's board before the slot;
//! - `d_halo`: the [`halo_digest`](air::halo_digest) of `E` outside the zone;
//! - `d_next`: the root of the zone's board after `k` generations.
//!
//! Roots are **canonical**: a sponge over the board rows in order, each row's values packed
//! and padded the same way, independent of the trace layout. So `d_next` of one slot equals
//! `d_prev` of the next, and any node can recompute a root from a state.
//!
//! Boundary condition: a missing neighbour contributes [`Rule::boundary`] cells to `E`,
//! which then evolve by the same rule (ghost cells); no masks enter the AIR, so the
//! constraint degree is `max(3, rule degree)` ([`air::StencilAir::symbolic_degree`]).

pub mod air;
pub mod heat;
#[cfg(test)]
mod heat_tests;
pub mod life;
pub mod poseidon;
#[cfg(test)]
mod tests;

use p3_field::PrimeCharacteristicRing;
pub use p3_koala_bear::KoalaBear as Val;

/// What a rule declares about its cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Spec {
    /// Values per cell.
    pub values: usize,
    /// Bits per value: every value is an integer in `[0, 2^bits)` (≤ 30).
    pub bits: usize,
    /// Neighbourhood radius `r` (the update reads `(2r+1)²` cells).
    pub radius: usize,
    /// Helper columns of a generation-0 cell (state validity, e.g. range-check bits).
    pub aux0: usize,
    /// Helper columns of each computed cell (e.g. inverses, quotients, range bits).
    pub aux: usize,
    /// Constraints per generation-0 cell ([`Rule::eval_state`]).
    pub state_constraints: usize,
    /// Constraints per computed cell ([`Rule::eval_step`]).
    pub step_constraints: usize,
}

/// A local update rule. `nb` is the neighbourhood in row-major order (`(2r+1)²` cells,
/// centre at index `(2r+1)·r + r`), each cell a slice of `values` entries.
///
/// Soundness obligations of an implementation (the framework relies on them):
/// - `eval_state` forces every value into `[0, 2^bits)` (the digests pack values);
/// - `eval_step` determines the next values uniquely and in `[0, 2^bits)` given valid
///   inputs, and is satisfied by `step`'s output and helpers.
pub trait Rule: Send + Sync + 'static {
    fn spec(&self) -> Spec;
    /// The cell value of a missing neighbour region.
    fn boundary(&self) -> Vec<u32>;
    /// Native update: the next values and the helper columns.
    fn step(&self, nb: &[&[u32]]) -> (Vec<u32>, Vec<Val>);
    /// Helper columns of a generation-0 cell.
    fn state_aux(&self, cell: &[u32]) -> Vec<Val>;
    /// Validity constraints of a generation-0 cell.
    fn eval_state<E: PrimeCharacteristicRing + Clone + From<Val>>(&self, cell: &[E], aux0: &[E], out: &mut Vec<E>);
    /// Update constraints: `next` (and `aux`) follow from `nb`.
    fn eval_step<E: PrimeCharacteristicRing + Clone + From<Val>>(
        &self,
        nb: &[&[E]],
        next: &[E],
        aux: &[E],
        out: &mut Vec<E>,
    );
}

/// A rectangular board of cells, `values` integers per cell, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub values: usize,
    pub data: Vec<u32>,
}

impl Grid {
    pub fn filled(w: usize, h: usize, cell: &[u32]) -> Self {
        Self { w, h, values: cell.len(), data: cell.iter().copied().cycle().take(w * h * cell.len()).collect() }
    }
    pub fn cell(&self, x: usize, y: usize) -> &[u32] {
        &self.data[(y * self.w + x) * self.values..][..self.values]
    }
    pub fn cell_mut(&mut self, x: usize, y: usize) -> &mut [u32] {
        &mut self.data[(y * self.w + x) * self.values..][..self.values]
    }
    pub fn row(&self, y: usize) -> &[u32] {
        &self.data[y * self.w * self.values..][..self.w * self.values]
    }

    /// The inner `n × n` board of an extended board with halo `h`.
    pub fn inner(&self, n: usize, halo: usize) -> Grid {
        let mut g = Grid::filled(n, n, &vec![0; self.values]);
        for y in 0..n {
            for x in 0..n {
                g.cell_mut(x, y).copy_from_slice(self.cell(x + halo, y + halo));
            }
        }
        g
    }

    /// `k` generations of `rule` on this extended board, each shrinking the board by the
    /// radius on every side (the reference the AIR is tested against).
    pub fn evolve<R: Rule>(&self, rule: &R, k: usize) -> Grid {
        let r = rule.spec().radius;
        let mut g = self.clone();
        for _ in 0..k {
            let (w, h) = (g.w - 2 * r, g.h - 2 * r);
            let mut next = Grid::filled(w, h, &vec![0; g.values]);
            for y in 0..h {
                for x in 0..w {
                    let nb: Vec<&[u32]> = (0..2 * r + 1)
                        .flat_map(|dy| (0..2 * r + 1).map(move |dx| (dx, dy)))
                        .map(|(dx, dy)| g.cell(x + dx, y + dy))
                        .collect();
                    let (v, _) = rule.step(&nb);
                    next.cell_mut(x, y).copy_from_slice(&v);
                }
            }
            g = next;
        }
        g
    }
}

/// Smallest trace height: shorter traces have too few LDE points per query for P100's
/// unique-decoding bound (2^5 gives 100.5 bits; INV-STENCIL-PARAM-1). Padding rows are free.
pub const MIN_HEIGHT: usize = 64;

/// The trace layout of a zone: `n × n` cells, `k` generations per slot, `rpr` stacked rows of
/// `E` per trace row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    pub spec: Spec,
    pub n: usize,
    pub k: usize,
    pub rpr: usize,
    /// Width (= height) of `E`: `n + 2·k·r`.
    pub we: usize,
    pub height: usize,
}

impl Shape {
    /// Needs `rpr ≥ 2r` (a cell reads at most one trace row ahead) and `k·r ≤ rpr`
    /// (`d_prev` reads generation 0 `k·r` stacked rows ahead), `bits ≤ 30`.
    pub fn new(spec: Spec, n: usize, k: usize, rpr: usize) -> Result<Self, String> {
        let r = spec.radius;
        if spec.bits == 0 || spec.bits > 30 || spec.values == 0 {
            return Err("values need 1..=30 bits".into());
        }
        if r == 0 || rpr < 2 * r || k == 0 || k * r > rpr || n == 0 {
            return Err(format!("layout needs r ≥ 1, k ≥ 1, rpr ≥ 2r and k·r ≤ rpr (r {r}, k {k}, rpr {rpr})"));
        }
        let we = n + 2 * k * r;
        let height = we.div_ceil(rpr).next_power_of_two().max(MIN_HEIGHT);
        Ok(Self { spec, n, k, rpr, we, height })
    }

    pub fn halo(&self) -> usize {
        self.k * self.spec.radius
    }
    /// Stacked rows on the torus.
    pub fn rows(&self) -> usize {
        self.rpr * self.height
    }
    /// First column and width of generation `t`'s cone.
    pub fn cone(&self, t: usize) -> (usize, usize) {
        let r = self.spec.radius;
        (t * r, self.we - 2 * t * r)
    }
    /// Columns per cell of generation `t`.
    pub fn cell_cols(&self, t: usize) -> usize {
        self.spec.values + if t == 0 { self.spec.aux0 } else { self.spec.aux }
    }
    /// First main column of generation `t`.
    pub fn base(&self, t: usize) -> usize {
        (0..t).map(|u| self.rpr * self.cone(u).1 * self.cell_cols(u)).sum()
    }
    /// Columns of the cell computation (all generations).
    pub fn cells_width(&self) -> usize {
        self.base(self.k + 1)
    }
    /// Column of value `j` (or helper `values + j`) of the generation-`t` cell at slot `i`,
    /// board column `x` (inside the cone).
    pub fn col(&self, t: usize, i: usize, x: usize, j: usize) -> usize {
        let (x0, w) = self.cone(t);
        debug_assert!(x >= x0 && x < x0 + w && i < self.rpr);
        self.base(t) + (i * w + x - x0) * self.cell_cols(t) + j
    }
    /// Values packed per limb.
    pub fn per_limb(&self) -> usize {
        30 / self.spec.bits
    }
}
