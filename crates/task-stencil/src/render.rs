//! Pictures of a stencil world (display only): life black and white, heat a colour ramp, the
//! lattice Boltzmann fluid its vorticity (blue clockwise, red anticlockwise) over its speed.

use crate::RuleId;
use std::io::Write;
use umis_stencil::Grid;
use umis_stencil::lbm::{Lbm, U};

/// The world board (`rx·n × ry·n`) of zones `(point, board)`; cells without a zone keep `fill`.
pub fn world(zones: &[([i32; 3], Grid)], n: usize, fill: &[u32]) -> Grid {
    let rx = zones.iter().map(|(p, _)| p[0] + 1).max().unwrap_or(1) as usize;
    let ry = zones.iter().map(|(p, _)| p[1] + 1).max().unwrap_or(1) as usize;
    let mut w = Grid::filled(rx * n, ry * n, fill);
    for (p, g) in zones {
        for y in 0..n {
            for x in 0..n {
                w.cell_mut(p[0] as usize * n + x, p[1] as usize * n + y).copy_from_slice(g.cell(x, y));
            }
        }
    }
    w
}

fn ramp(t: f32) -> [u8; 3] {
    // black → red → yellow → white
    let t = t.clamp(0.0, 1.0);
    let c = |a: f32| (a.clamp(0.0, 1.0) * 255.0) as u8;
    [c(3.0 * t), c(3.0 * t - 1.0), c(3.0 * t - 2.0)]
}

/// RGB pixels of a world board, `scale` pixels per cell.
pub fn rgb(rule: &RuleId, w: &Grid, scale: usize) -> (usize, usize, Vec<u8>) {
    let (iw, ih) = (w.w * scale, w.h * scale);
    let mut px = vec![0u8; iw * ih * 3];
    let colour: Vec<[u8; 3]> = match rule {
        RuleId::Life => w.data.iter().map(|v| if *v == 1 { [255, 255, 255] } else { [10, 10, 30] }).collect(),
        RuleId::Heat { wall, .. } => {
            let lo = *wall as f32;
            w.data.iter().map(|v| ramp(((*v as f32 - lo) / (60_000.0 - lo)).sqrt())).collect()
        }
        RuleId::Lbm { .. } => {
            let vel: Vec<(f32, f32)> = w
                .data
                .chunks_exact(9)
                .map(|c| {
                    let (rho, mx, my) = Lbm::moments(c);
                    let r = rho.max(1) as f32;
                    (mx as f32 / r, my as f32 / r)
                })
                .collect();
            let raw = |x: usize, y: usize| vel[y.min(w.h - 1) * w.w + x.min(w.w - 1)];
            // a 3×3 box average: fixed-point rounding makes single-cell curls noisy
            let at = |x: usize, y: usize| {
                let (mut sx, mut sy) = (0.0, 0.0);
                for dy in 0..3 {
                    for dx in 0..3 {
                        let v = raw((x + dx).saturating_sub(1), (y + dy).saturating_sub(1));
                        sx += v.0;
                        sy += v.1;
                    }
                }
                (sx / 9.0, sy / 9.0)
            };
            (0..w.h)
                .flat_map(|y| (0..w.w).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let (xm, ym) = (x.saturating_sub(1), y.saturating_sub(1));
                    let curl = (at(x + 1, y).1 - at(xm, y).1) - (at(x, y + 1).0 - at(x, ym).0);
                    let (ux, uy) = at(x, y);
                    let speed = (ux * ux + uy * uy).sqrt() * 8.0;
                    let k = (curl * 25.0).clamp(-1.0, 1.0);
                    let base = (speed * 60.0).min(60.0);
                    let c = |a: f32| a.clamp(0.0, 255.0) as u8;
                    if k >= 0.0 {
                        [c(base + 255.0 * k), c(base + 60.0 * k), c(base)]
                    } else {
                        [c(base), c(base - 60.0 * k), c(base - 255.0 * k)]
                    }
                })
                .collect()
        }
    };
    for y in 0..ih {
        for x in 0..iw {
            px[(y * iw + x) * 3..][..3].copy_from_slice(&colour[(y / scale) * w.w + x / scale]);
        }
    }
    (iw, ih, px)
}

/// Writes a binary PPM image.
pub fn write_ppm(path: &std::path::Path, w: usize, h: usize, rgb: &[u8]) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    write!(f, "P6\n{w} {h}\n255\n")?;
    f.write_all(rgb)
}

/// One line about a world board: life population, heat mean temperature, fluid mass and
/// kinetic energy.
pub fn summary(rule: &RuleId, w: &Grid) -> String {
    match rule {
        RuleId::Life => format!("population {}", w.data.iter().sum::<u32>()),
        RuleId::Heat { .. } => {
            let mean = w.data.iter().map(|v| *v as f64).sum::<f64>() / w.data.len() as f64;
            format!("mean temperature {mean:.0}")
        }
        RuleId::Lbm { .. } => {
            let (mut mass, mut ke) = (0i64, 0f64);
            for c in w.data.chunks_exact(9) {
                let (rho, mx, my) = Lbm::moments(c);
                mass += rho;
                ke += (mx * mx + my * my) as f64 / rho.max(1) as f64;
            }
            format!("mass {:.4} kinetic energy {:.3}", mass as f64 / (U as f64 * (w.w * w.h) as f64), ke / U as f64)
        }
    }
}
