//! The integer stencil: coefficients, one substep, face extraction.
//!
//! Offsets `k`: 0 = the cell itself, then `+x, −x, +y, −y, +z, −z` (offset
//! `k` points along face port `k − 1`).
//!
//! With `Δt = S·dt_tick / k_sub` and `2^s` the fixed-point one:
//! - diffusion weight on each neighbour: `⌊2^s·Δt·ν⌋`;
//! - advection weight on the upwind neighbour of axis `a`: `⌊2^s·Δt·|Ū_a|⌋`
//!   (velocity units 1024 = 1 cell/s);
//! - relaxation `⌊2^s·Δt·λ⌋` is taken from the self weight;
//! - self weight: `2^s` minus all of the above, so `Σ_k w_k = 2^s − relax ≤ 2^s`.
//!
//! Where a strong current would make the self weight negative (CFL > 1),
//! the advection weights of that cell are scaled down to fit (a flux limiter),
//! keeping every weight non-negative. Forcing: `F_v = ⌊2^s·Δt·λ·Ū / 256⌋`,
//! `F_c = ⌊2^s·Δt·src / 256⌋`, and `b = tide·F` (tide 256 = 1.0).

use crate::field::{GenesisWater, SRC_MAX, TIDE_AMP, TIDE_ONE, VEL_ONE};
use crate::{Cell, FIELDS, Q_BOUND, Water3dParams};
use umis_task::fx::mul_div_floor;

pub const OFFSETS: [[i64; 3]; 7] = [[0, 0, 0], [1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coeffs {
    /// `Ū` per cell.
    pub ubar: Vec<[i64; 3]>,
    /// Velocity weights `w_k(x)` per cell.
    pub wv: Vec<[i64; 7]>,
    /// Dye weights per cell.
    pub wc: Vec<[i64; 7]>,
    /// Velocity forcing shape `F_v(x)`.
    pub fv: Vec<[i64; 3]>,
    /// Dye forcing shape `F_c(x)`.
    pub fc: Vec<i64>,
}

/// `Δt = dt_num / dt_den` in seconds.
fn dt(p: &Water3dParams) -> (i64, i64) {
    (p.slot_ticks as i64 * p.dt_tick.num as i64, p.k_sub as i64 * p.dt_tick.den as i64)
}

/// `⌊2^s · Δt · r⌋` for a rational coefficient `r`.
fn scaled(p: &Water3dParams, r: crate::Ratio) -> i64 {
    let (dn, dd) = dt(p);
    mul_div_floor((1i64 << p.shift) * dn, r.num as i64, dd * r.den as i64)
}

/// Diffusion-number bound (`6·W_diff + W_relax ≤ 2^s / 2` for both value
/// kinds) and the boundedness of dye and velocity under the forcing.
pub fn check_coefficient_bounds(p: &Water3dParams) -> Result<(), String> {
    let one = 1i64 << p.shift;
    for (name, diff, relax) in [("velocity", p.nu, p.lambda), ("dye", p.kappa, p.mu)] {
        let total = 6 * scaled(p, diff) + scaled(p, relax);
        if total > one / 2 {
            return Err(format!("{name}: diffusion number too large ({total} > {})", one / 2));
        }
        if scaled(p, relax) == 0 {
            return Err(format!("{name}: relaxation rounds to zero at this shift"));
        }
    }
    // Steady dye ≤ tide_max · src_max / μ must stay below the bound.
    let dye_max = (TIDE_ONE + TIDE_AMP) * SRC_MAX * p.mu.den as i64 / (TIDE_ONE * p.mu.num as i64);
    if dye_max >= Q_BOUND {
        return Err(format!("dye could reach {dye_max} ≥ 2^15; raise mu"));
    }
    Ok(())
}

impl Coeffs {
    pub fn compute(p: &Water3dParams, gw: &GenesisWater) -> Self {
        let n = p.n() as i64;
        let one = 1i64 << p.shift;
        let (dn, dd) = dt(p);
        let w_diff_v = scaled(p, p.nu);
        let w_relax_v = scaled(p, p.lambda);
        let w_diff_c = scaled(p, p.kappa);
        let w_relax_c = scaled(p, p.mu);
        let origin = [p.point[0] as i64 * n, p.point[1] as i64 * n, p.point[2] as i64 * n];
        let cells = p.cells();
        let mut c = Coeffs {
            ubar: Vec::with_capacity(cells),
            wv: Vec::with_capacity(cells),
            wc: Vec::with_capacity(cells),
            fv: Vec::with_capacity(cells),
            fc: Vec::with_capacity(cells),
        };
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let g = [origin[0] + x, origin[1] + y, origin[2] + z];
                    let u = gw.velocity(g);
                    // Advection weights on the upwind neighbours.
                    let mut adv = [0i64; 3];
                    for a in 0..3 {
                        adv[a] = mul_div_floor(one * dn, u[a].abs(), dd * VEL_ONE);
                    }
                    let weights = |w_diff: i64, w_relax: i64| -> [i64; 7] {
                        let avail = one - 6 * w_diff - w_relax;
                        let sum: i64 = adv.iter().sum();
                        let adv = if sum > avail { adv.map(|w| w * avail / sum) } else { adv };
                        let mut w = [w_diff; 7];
                        for a in 0..3 {
                            // Ū_a > 0 → upwind is −e_a (offset 2a+2); Ū_a < 0 → +e_a (2a+1).
                            let k = if u[a] > 0 { 2 * a + 2 } else { 2 * a + 1 };
                            w[k] += adv[a];
                        }
                        w[0] = one - w_relax - w[1..].iter().sum::<i64>();
                        debug_assert!(w.iter().all(|v| *v >= 0));
                        w
                    };
                    c.wv.push(weights(w_diff_v, w_relax_v));
                    c.wc.push(weights(w_diff_c, w_relax_c));
                    c.fv.push(u.map(|ua| {
                        mul_div_floor(one * dn * p.lambda.num as i64, ua, dd * p.lambda.den as i64 * TIDE_ONE)
                    }));
                    c.fc.push(mul_div_floor(one * dn, gw.source(g), dd * TIDE_ONE));
                    c.ubar.push(u);
                }
            }
        }
        c
    }
}

#[inline]
pub fn idx(n: usize, x: usize, y: usize, z: usize) -> usize {
    x + n * (y + n * z)
}

/// Position of cell `(x, y, z)` in the face of direction `d` (0..6):
/// x-faces are indexed by `y + n·z`, y-faces by `x + n·z`, z-faces by `x + n·y`.
#[inline]
pub fn face_index(n: usize, d: usize, x: usize, y: usize, z: usize) -> usize {
    match d / 2 {
        0 => y + n * z,
        1 => x + n * z,
        _ => x + n * y,
    }
}

/// Cells of the boundary layer facing direction `d`, in face order.
pub fn face_cells(p: &Water3dParams, cells: &[Cell], d: usize) -> Vec<Cell> {
    let n = p.n();
    let layer = if d % 2 == 0 { n - 1 } else { 0 };
    let mut out = vec![[0; FIELDS]; n * n];
    for b in 0..n {
        for a in 0..n {
            let (x, y, z) = match d / 2 {
                0 => (layer, a, b),
                1 => (a, layer, b),
                _ => (a, b, layer),
            };
            out[a + n * b] = cells[idx(n, x, y, z)];
        }
    }
    out
}

/// The value at offset `k` of cell `(x, y, z)`: inside the zone, or the halo.
#[inline]
fn neighbour(n: usize, cur: &[Cell], halos: &[Vec<Cell>; 6], x: usize, y: usize, z: usize, k: usize) -> Cell {
    let o = OFFSETS[k];
    let (nx, ny, nz) = (x as i64 + o[0], y as i64 + o[1], z as i64 + o[2]);
    let ni = n as i64;
    if (0..ni).contains(&nx) && (0..ni).contains(&ny) && (0..ni).contains(&nz) {
        cur[idx(n, nx as usize, ny as usize, nz as usize)]
    } else {
        halos[k - 1][face_index(n, k - 1, x, y, z)]
    }
}

/// The numerator `Σ_k w_k·q(x+k) + b` of one value; also used by the AIR
/// witness generator.
#[inline]
pub fn numerator(w: &[i64; 7], vals: &[i64; 7], b: i64) -> i64 {
    let mut acc = b;
    for k in 0..7 {
        acc += w[k] * vals[k];
    }
    acc
}

/// One substep over every cell: `next[x][f] = ⌊num / 2^s⌋`.
pub fn substep(p: &Water3dParams, c: &Coeffs, tide: i64, cur: &[Cell], halos: &[Vec<Cell>; 6], next: &mut [Cell]) {
    let n = p.n();
    let s = p.shift;
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let i = idx(n, x, y, z);
                let mut nb = [[0i32; FIELDS]; 7];
                for (k, slot) in nb.iter_mut().enumerate() {
                    *slot = if k == 0 { cur[i] } else { neighbour(n, cur, halos, x, y, z, k) };
                }
                let mut out = [0i32; FIELDS];
                for f in 0..FIELDS {
                    let vals: [i64; 7] = std::array::from_fn(|k| nb[k][f] as i64);
                    let (w, b) = if f < 3 { (&c.wv[i], tide * c.fv[i][f]) } else { (&c.wc[i], tide * c.fc[i]) };
                    out[f] = (numerator(w, &vals, b) >> s) as i32;
                }
                next[i] = out;
            }
        }
    }
}
