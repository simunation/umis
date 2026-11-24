//! The STARK trace shape of a zone (`air.rs` layout), outside the `stark` feature so that
//! `validate_params` can bound it (INV-LIFE-PARAM-1).

/// Smallest trace height: shorter traces have too few LDE points per query for the
/// unique-decoding bound of the P100 preset (INV-LIFE-PARAM-1). Padding rows are free
/// (INV-LIFE-CONE-3).
pub const MIN_HEIGHT: usize = 64;
/// Largest trace height, main width and constraint count a verifier accepts: the shape
/// envelope at which the P100 security bound is asserted (INV-LIFE-PARAM-1).
pub const MAX_HEIGHT: usize = 1 << 12;
pub const MAX_WIDTH: usize = 4096;
pub const MAX_CONSTRAINTS: usize = 8192;

use crate::Life2dParams;

/// Cells packed per periodic limb (a sum of ≤ 30 booleans stays below p, INV-LIFE-IN-1).
pub const LIMB_BITS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    pub n: usize,
    pub k: usize,
    pub strips: usize,
    pub rpr: usize,
    /// Inner columns per strip.
    pub w: usize,
    /// Columns per strip, halo included.
    pub ws: usize,
    /// Rows per strip, halo included.
    pub y: usize,
    /// Trace height.
    pub height: usize,
}

impl Shape {
    pub fn of(p: &Life2dParams) -> Self {
        let (n, k, strips, rpr) = (p.n(), p.k(), p.strips as usize, p.rows_per_trace_row as usize);
        let w = n / strips;
        let y = n + 2 * k;
        let height = (strips * y).div_ceil(rpr).next_power_of_two().max(MIN_HEIGHT);
        Self { n, k, strips, rpr, w, ws: w + 2 * k, y, height }
    }

    /// Torus rows.
    pub fn rows(&self) -> usize {
        self.rpr * self.height
    }

    /// Cone width of generation `t ≥ 1`.
    pub fn cone(&self, t: usize) -> usize {
        self.ws - 2 * t
    }

    /// First main column of generation `t ≥ 1` (generation 0 comes first).
    pub fn base(&self, t: usize) -> usize {
        self.rpr * self.ws + (1..t).map(|u| 2 * self.rpr * self.cone(u)).sum::<usize>()
    }

    pub fn width(&self) -> usize {
        self.base(self.k + 1)
    }

    /// Main column of generation-0 cell at slot `i`, strip column `x`.
    pub fn col0(&self, i: usize, x: usize) -> usize {
        i * self.ws + x
    }

    /// Main column of cell `a` (`+1`: its `v`) at generation `t ≥ 1`, slot `i`,
    /// strip column `x` (inside the cone).
    pub fn col(&self, t: usize, i: usize, x: usize) -> usize {
        debug_assert!(t >= 1 && x >= t && x < self.ws - t);
        self.base(t) + 2 * (i * self.cone(t) + x - t)
    }

    /// Packed limbs of a generation-0 row (`Ws` cells) and of an output row (`w`).
    pub(crate) fn limbs0(&self) -> usize {
        self.ws.div_ceil(LIMB_BITS)
    }
    pub(crate) fn limbs_out(&self) -> usize {
        self.w.div_ceil(LIMB_BITS)
    }

    /// Exact number of constraints `air::LifeAir::eval` asserts.
    pub fn num_constraints(&self) -> usize {
        let cells: usize = (1..=self.k).map(|t| self.cone(t)).sum();
        self.rpr * (self.ws + self.limbs0() + 2 * cells + self.limbs_out())
    }
}
