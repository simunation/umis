//! Genesis boards and display channels per rule (never used by the proofs).
//!
//! A genesis board is a function of the genesis seed, the params and the zone's global position,
//! so patterns line up across zones.

use crate::{RuleId, StencilParams};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use umis_core::{Hash, keccak256_concat};
use umis_stencil::Grid;
use umis_stencil::lbm::{Lbm, U};

fn rng(seed: &Hash, p: &StencilParams, point: [i32; 3]) -> ChaCha20Rng {
    let tag = keccak256_concat(&[
        seed.as_bytes(),
        b"stencil2d-genesis",
        p.rule.name().as_bytes(),
        &point[0].to_be_bytes(),
        &point[1].to_be_bytes(),
        &p.n.to_be_bytes(),
    ]);
    ChaCha20Rng::from_seed(tag.0)
}

/// The genesis board of the zone at `point`.
pub fn genesis_board(seed: &Hash, p: &StencilParams, point: [i32; 3]) -> Grid {
    let n = p.n();
    let mut r = rng(seed, p, point);
    let (gx0, gy0) = (point[0] as i64 * n as i64, point[1] as i64 * n as i64);
    let world_h = (p.genesis_region[1].max(1) as i64) * n as i64;
    let world_w = (p.genesis_region[0].max(1) as i64) * n as i64;
    match p.rule {
        RuleId::Life => {
            let mut g = Grid::filled(n, n, &[0]);
            for c in g.data.iter_mut() {
                *c = (r.next_u32() % 100 < 30) as u32;
            }
            g
        }
        RuleId::Heat { wall, .. } => {
            // A few hot discs on a cool background (pattern 1: one disc per zone, centred).
            let mut g = Grid::filled(n, n, &[wall]);
            let discs: Vec<(i64, i64, i64)> = if p.pattern == 1 {
                vec![(n as i64 / 2, n as i64 / 2, n as i64 / 4)]
            } else {
                (0..3)
                    .map(|_| {
                        (
                            (r.next_u32() as usize % n) as i64,
                            (r.next_u32() as usize % n) as i64,
                            3 + (r.next_u32() % 5) as i64,
                        )
                    })
                    .collect()
            };
            for y in 0..n {
                for x in 0..n {
                    if discs.iter().any(|(cx, cy, rad)| (x as i64 - cx).pow(2) + (y as i64 - cy).pow(2) <= rad * rad) {
                        g.cell_mut(x, y)[0] = 60_000;
                    }
                }
            }
            g
        }
        RuleId::Lbm { .. } => {
            // A shear layer across the whole world: the middle band flows right, the rest
            // left, with a small transverse wave that rolls up into Kelvin–Helmholtz vortices
            // (pattern 0 adds a little random density noise).
            let mut g = Grid::filled(n, n, &Lbm::equilibrium(U, (0, 0)));
            let u0 = U / 10; // 0.1 lattice units (Mach ≈ 0.17)
            for y in 0..n {
                for x in 0..n {
                    let (gx, gy) = (gx0 + x as i64, gy0 + y as i64);
                    let band = gy > world_h / 4 && gy < 3 * world_h / 4;
                    let ux = if band { u0 } else { -u0 };
                    let phase = 2.0 * std::f64::consts::PI * gx as f64 / world_w.max(1) as f64 * 2.0;
                    let uy = (phase.sin() * (U as f64) / 50.0) as i64;
                    let noise = if p.pattern == 0 { (r.next_u32() % 3) as i64 - 1 } else { 0 };
                    g.cell_mut(x, y).copy_from_slice(&Lbm::equilibrium(U + noise, (ux, uy)));
                }
            }
            g
        }
    }
}

/// Display channels of a board.
pub fn render_channels(rule: &RuleId, g: &Grid) -> (u32, Vec<f32>) {
    match rule {
        RuleId::Life => (1, g.data.iter().map(|v| *v as f32).collect()),
        RuleId::Heat { .. } => (1, g.data.iter().map(|v| *v as f32 / 65535.0).collect()),
        RuleId::Lbm { .. } => {
            let mut out = Vec::with_capacity(3 * g.w * g.h);
            for c in g.data.chunks_exact(9) {
                let (rho, mx, my) = Lbm::moments(c);
                out.extend([(rho - U) as f32 / U as f32, mx as f32 / U as f32, my as f32 / U as f32]);
            }
            (3, out)
        }
    }
}
