//! Genesis-derived fields, all integer and evaluated in global cell
//! coordinates, so they are continuous across zones and every zone can
//! compute its part locally:
//!
//! - the background current `Ū(x)`: the discrete curl of a vector potential
//!   `A` built from random vortex tubes, jets and a drift. With central
//!   differences the discrete divergence of a discrete curl is exactly zero;
//! - dye sources `src(x)`;
//! - the tide table `tide(tick)`.
//!
//! Units: velocity 1024 = one cell per physical second; tide 256 = 1.0.

use crate::Water3dParams;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use umis_core::{Hash, keccak256_concat};
use umis_task::fx::SIN64_MILLI;

pub const VEL_ONE: i64 = 1024;
pub const TIDE_ONE: i64 = 256;
/// Tide amplitude: ±0.375.
pub const TIDE_AMP: i64 = 96;
/// Largest dye source strength (dye units per second).
pub const SRC_MAX: i64 = 300;
/// Feature strength bounds of the vector potential.
pub const STRENGTH_MIN: i64 = 600;
pub const STRENGTH_MAX: i64 = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feature {
    /// `A_axis += S·(r² − d²)² / r⁴` within distance `r` of the axis line.
    Tube { axis: usize, c: [i64; 3], r: i64, s: i64 },
    /// Horizontal jet along `axis` (0 = x, 1 = y), centred on `c[1-axis]`
    /// with half-width `w`, vertical extent `h` around `c[2]`.
    Jet { axis: usize, c: [i64; 3], w: i64, h: i64, s: i64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DyeSource {
    pub c: [i64; 3],
    pub r: i64,
    pub q: i64,
}

/// Everything random about the genesis water, derived from the genesis seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenesisWater {
    pub features: Vec<Feature>,
    /// Uniform drift `(dx, dy)`: `A_z += dx·y − dy·x`.
    pub drift: [i64; 2],
    pub sources: Vec<DyeSource>,
    pub tide_period_ticks: i64,
    pub tide_phase: i64,
}

fn range(rng: &mut ChaCha20Rng, lo: i64, hi: i64) -> i64 {
    lo + (rng.next_u32() as i64) % (hi - lo + 1)
}

fn signed(rng: &mut ChaCha20Rng, lo: i64, hi: i64) -> i64 {
    let v = range(rng, lo, hi);
    if rng.next_u32() & 1 == 0 { v } else { -v }
}

impl GenesisWater {
    /// The RNG is `ChaCha20` seeded with `H(genesis.seed ‖ "water3d")`.
    pub fn generate(genesis_seed: &Hash, p: &Water3dParams) -> Self {
        let seed = keccak256_concat(&[&genesis_seed.0, b"water3d"]);
        let mut rng = ChaCha20Rng::from_seed(seed.0);
        let n = p.n_loc as i64;
        let dom = [p.domain[0] as i64 * n, p.domain[1] as i64 * n, p.domain[2] as i64 * n];
        let foc = [p.focus[0] as i64 * n, p.focus[1] as i64 * n, p.focus[2] as i64 * n];
        let mut features = Vec::new();
        for k in 0..p.vortices {
            let ext = if k % 2 == 0 { foc } else { dom };
            let c = [range(&mut rng, 0, ext[0] - 1), range(&mut rng, 0, ext[1] - 1), range(&mut rng, 0, ext[2] - 1)];
            if rng.next_u32() % 3 < 2 {
                // Mostly vertical tubes: horizontal gyres.
                let axis = if rng.next_u32() % 3 == 0 { (rng.next_u32() % 2) as usize } else { 2 };
                features.push(Feature::Tube {
                    axis,
                    c,
                    r: range(&mut rng, 6, 24),
                    s: signed(&mut rng, STRENGTH_MIN, STRENGTH_MAX),
                });
            } else {
                features.push(Feature::Jet {
                    axis: (rng.next_u32() % 2) as usize,
                    c,
                    w: range(&mut rng, 4, 12),
                    h: range(&mut rng, 4, 16),
                    s: signed(&mut rng, STRENGTH_MIN, STRENGTH_MAX),
                });
            }
        }
        let drift = [signed(&mut rng, 0, 50), signed(&mut rng, 0, 50)];
        let sources = (0..p.dye_sources)
            .map(|_| DyeSource {
                c: [range(&mut rng, 0, foc[0] - 1), range(&mut rng, 0, foc[1] - 1), range(&mut rng, 0, foc[2] - 1)],
                r: range(&mut rng, 3, 8),
                q: range(&mut rng, SRC_MAX / 3, SRC_MAX),
            })
            .collect();
        Self {
            features,
            drift,
            sources,
            tide_period_ticks: range(&mut rng, 400, 1600),
            tide_phase: range(&mut rng, 0, 63),
        }
    }

    /// Vector potential at a global cell coordinate.
    pub fn potential(&self, g: [i64; 3]) -> [i64; 3] {
        let mut a = [0i64; 3];
        for f in &self.features {
            match *f {
                Feature::Tube { axis, c, r, s } => {
                    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                    let d2 = (g[u] - c[u]).pow(2) + (g[v] - c[v]).pow(2);
                    let r2 = r * r;
                    if d2 < r2 {
                        let t = (r2 - d2) as i128;
                        a[axis] += (s as i128 * t * t / (r2 as i128 * r2 as i128)) as i64;
                    }
                }
                Feature::Jet { axis, c, w, h, s } => {
                    // A_z = ±S · smoothstep across the jet · vertical bump.
                    let across = 1 - axis;
                    let t = (g[across] - (c[across] - w)).clamp(0, 2 * w) as i128;
                    let ww = (2 * w) as i128;
                    let step_num = 3 * t * t * ww - 2 * t * t * t; // smoothstep · (2w)³
                    let dz2 = (g[2] - c[2]).pow(2);
                    let h2 = h * h;
                    if dz2 < h2 {
                        let bz = (h2 - dz2) as i128;
                        let num = s as i128 * step_num * bz * bz;
                        let den = ww * ww * ww * (h2 as i128) * (h2 as i128);
                        let sign = if axis == 0 { 1 } else { -1 };
                        a[2] += sign * (num / den) as i64;
                    }
                }
            }
        }
        a[2] += self.drift[0] * g[1] - self.drift[1] * g[0];
        a
    }

    /// `Ū = curl A` with central differences (factor 2 absorbed into units).
    pub fn velocity(&self, g: [i64; 3]) -> [i64; 3] {
        let at = |d: [i64; 3]| self.potential([g[0] + d[0], g[1] + d[1], g[2] + d[2]]);
        let (xp, xm, yp, ym, zp, zm) =
            (at([1, 0, 0]), at([-1, 0, 0]), at([0, 1, 0]), at([0, -1, 0]), at([0, 0, 1]), at([0, 0, -1]));
        [(yp[2] - ym[2]) - (zp[1] - zm[1]), (zp[0] - zm[0]) - (xp[2] - xm[2]), (xp[1] - xm[1]) - (yp[0] - ym[0])]
    }

    /// Dye source strength at a global cell.
    pub fn source(&self, g: [i64; 3]) -> i64 {
        let mut q = 0;
        for s in &self.sources {
            let d2 = (0..3).map(|i| (g[i] - s.c[i]).pow(2)).sum::<i64>();
            let r2 = s.r * s.r;
            if d2 < r2 {
                q += s.q * (r2 - d2).pow(2) / (r2 * r2);
            }
        }
        q.min(SRC_MAX)
    }

    /// `tide` at a tick: `256 + 96·sin(2π·(phase/64 + tick/period))`, from a
    /// fixed integer sine table.
    pub fn tide(&self, tick: u64) -> i64 {
        let idx = (self.tide_phase + (tick as i64 * 64) / self.tide_period_ticks).rem_euclid(64);
        TIDE_ONE + TIDE_AMP * SIN64_MILLI[idx as usize] as i64 / 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Water3dParams {
        Water3dParams::default_for([0, 0, 0], 4)
    }

    #[test]
    fn deterministic_generation() {
        let s = umis_core::keccak256(b"genesis");
        assert_eq!(GenesisWater::generate(&s, &params()), GenesisWater::generate(&s, &params()));
        assert_ne!(
            GenesisWater::generate(&s, &params()),
            GenesisWater::generate(&umis_core::keccak256(b"x"), &params())
        );
    }

    #[test]
    fn background_current_is_discretely_divergence_free() {
        let gw = GenesisWater::generate(&umis_core::keccak256(b"genesis"), &params());
        for x in 10..20 {
            for y in 10..20 {
                for z in 2..8 {
                    let v = |d: [i64; 3]| gw.velocity([x + d[0], y + d[1], z + d[2]]);
                    let div = (v([1, 0, 0])[0] - v([-1, 0, 0])[0])
                        + (v([0, 1, 0])[1] - v([0, -1, 0])[1])
                        + (v([0, 0, 1])[2] - v([0, 0, -1])[2]);
                    assert_eq!(div, 0, "at {x},{y},{z}");
                }
            }
        }
    }

    #[test]
    fn tide_range() {
        let gw = GenesisWater::generate(&umis_core::keccak256(b"genesis"), &params());
        for t in 0..5000 {
            let v = gw.tide(t);
            assert!((TIDE_ONE - TIDE_AMP..=TIDE_ONE + TIDE_AMP).contains(&v));
        }
    }
}
