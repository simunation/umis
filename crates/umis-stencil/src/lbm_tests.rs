//! The lattice Boltzmann toy (D2Q9): physics sanity, honest batch proofs, and the
//! **release-mode verifier** rejecting wrong roundings that the equations alone would allow.

use crate::air::public_values;
use crate::batch::StencilBatch;
use crate::lbm::{C, Lbm, U};
use crate::{Grid, Rule, Val};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use umis_stark::config::FriPreset;

fn rest(w: usize) -> Grid {
    Grid::filled(w, w, &Lbm::default().boundary())
}

/// Rest fluid with a density bump of `amp` (fixed point) in a centred disc of radius 2.
fn pulse(w: usize, amp: i64) -> Grid {
    let mut g = rest(w);
    let c = w as i64 / 2;
    for y in 0..w {
        for x in 0..w {
            let (dx, dy) = (x as i64 - c, y as i64 - c);
            if dx * dx + dy * dy <= 4 {
                g.cell_mut(x, y).copy_from_slice(&Lbm::equilibrium(U + amp, (0, 0)));
            }
        }
    }
    g
}

fn accepted<R: Rule + Clone>(b: &StencilBatch<R>, traces: Vec<RowMajorMatrix<Val>>, claim: &[Val]) -> bool {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let proof = b.prove(&traces, claim)?;
        b.verify(&proof, claim)
    }));
    matches!(r, Ok(Ok(())))
}

#[test]
fn rest_is_a_fixed_point_and_a_pulse_spreads_symmetrically() {
    let lbm = Lbm::default();
    assert_eq!(rest(10).evolve(&lbm, 3), rest(4));
    // mass is conserved exactly in the interior of a pulse (away from the shrinking edge)
    let g0 = pulse(21, 200);
    let g1 = g0.evolve(&lbm, 1);
    let mass = |g: &Grid, lo: usize, hi: usize| -> i64 {
        (lo..hi).flat_map(|y| (lo..hi).map(move |x| (x, y))).map(|(x, y)| Lbm::moments(g.cell(x, y)).0).sum()
    };
    // one generation moves mass at most one cell: the 15×15 core of g0 (inside a rest ring) equals
    // the 17×17 window of g1 shifted by the shrink, both surrounded by rest fluid
    assert_eq!(mass(&g0, 2, 19), mass(&g1, 1, 18));
    let g = pulse(17, 200).evolve(&lbm, 4);
    let rho = |x: usize, y: usize| Lbm::moments(g.cell(x, y)).0;
    let c = g.w / 2;
    assert_eq!(rho(c + 2, c), rho(c - 2, c));
    assert_eq!(rho(c, c + 2), rho(c, c - 2));
    assert_eq!(rho(c + 2, c), rho(c, c + 2));
    // the bump moved outwards: the centre lost density, the ring gained
    let rest_rho = Lbm::moments(&lbm.boundary()).0;
    assert!(rho(c + 3, c) > rest_rho, "{} vs {rest_rho}", rho(c + 3, c));
    // momentum points outward on the ring
    let (_, mx, _) = Lbm::moments(g.cell(c + 2, c));
    assert!(mx > 0);
    assert_eq!(C.len(), 9);
}

#[test]
fn honest_lbm_batch_proofs_verify() {
    for (n, k, rpr) in [(8, 1, 2), (12, 2, 2)] {
        let b = StencilBatch::new(Lbm::default(), n, k, rpr, FriPreset::Test).unwrap();
        let ext = pulse(b.shape.we, 150);
        let (t, public, next) = b.generate(&ext);
        assert_eq!(next, ext.evolve(&Lbm::default(), k));
        assert_eq!(public, public_values(&b.shape, &ext, &next));
        assert!(accepted(&b, t, &public), "{n},{k},{rpr}");
    }
}

/// The batch leaf of an 8×8 pulse, and the column of helper `j` (`q, r1, r9, feq, r2`) of
/// direction `i` for the generation-1 cell at slot 1, zone column 3 of trace row 1.
fn forge_site() -> (StencilBatch<Lbm>, RowMajorMatrix<Val>, usize, usize) {
    let b = StencilBatch::new(Lbm::default(), 8, 1, 2, FriPreset::Test).unwrap();
    let ext = pulse(b.shape.we, 150);
    let (t, ..) = b.generate(&ext);
    let c = b.cells_only(&t[0]);
    let s = b.shape;
    let (x, i, r) = (s.halo() + 3, 1, 1);
    (b, c, r, s.col(1, i, x, 0))
}

#[test]
fn the_equilibrium_division_cannot_move_nine_into_the_remainder() {
    // q = 9·feq + r9 also holds with (feq − 1, r9 + 9): r9 + 9 < 16 passes a plain 4-bit check,
    // so only `8 − r9 ≥ 0` rejects it. The forged feq changes g'; the claim follows the forgery.
    let (b, mut c, r, col) = forge_site();
    let w = c.width();
    // helpers of direction i ≥ 1 start at column 9 + 5(i − 1); pick one with r9 ≤ 6
    let get = |c: &RowMajorMatrix<Val>, d: usize, j: usize| {
        crate::air::to_u32(c.values[r * w + col + 9 + 5 * (d - 1) + j]) as i64
    };
    let d = (1..9).find(|&d| get(&c, d, 2) + 9 < 16 && get(&c, d, 3) > 0).expect("a direction with r9 ≤ 6");
    let base = col + 9 + 5 * (d - 1);
    let (feq, r9, r2) = (get(&c, d, 3), get(&c, d, 2), get(&c, d, 4));
    let lbm = Lbm::default();
    let g_old = crate::air::to_u32(c.values[r * w + col + d]) as i64;
    // N2 drops by a; recompute g' and r2 for feq − 1
    let n2 = g_old * (1 << lbm.t) + r2 - lbm.a; // feq − 1 lowers N2 by a
    // g'_0 closes the mass: it absorbs the change of g'_d
    let (g, r2n) = (n2.div_euclid(1 << lbm.t), n2.rem_euclid(1 << lbm.t));
    c.values[r * w + base + 3] = Val::from_i64(feq - 1);
    c.values[r * w + base + 2] = Val::from_i64(r9 + 9);
    c.values[r * w + base + 4] = Val::from_i64(r2n);
    c.values[r * w + col + d] = Val::from_i64(g);
    c.values[r * w + col] += Val::from_i64(g_old - g);
    let (forged, claim) = b.assemble(&c);
    assert!(!accepted(&b, forged, &claim));
}

#[test]
fn the_collision_cannot_round_up() {
    // g' + 1 with r2 − 2^t: the equation holds, r2 leaves its range.
    let (b, mut c, r, col) = forge_site();
    let w = c.width();
    let d = 3usize;
    let lbm = Lbm::default();
    c.values[r * w + col + d] += Val::ONE;
    c.values[r * w + col] -= Val::ONE; // g'_0 keeps the mass
    c.values[r * w + col + 9 + 5 * (d - 1) + 4] -= Val::from_u32(1 << lbm.t);
    let (forged, claim) = b.assemble(&c);
    assert!(!accepted(&b, forged, &claim));
}

#[test]
fn lbm_clears_the_target_at_p100() {
    use umis_stark::security::{TARGET_BITS, proven_udr_bits};
    let b = StencilBatch::new(Lbm::default(), 8, 1, 2, FriPreset::P100).unwrap();
    let ext = pulse(b.shape.we, 150);
    let (t, public, _) = b.generate(&ext);
    let proof = b.prove(&t, &public).unwrap();
    b.verify(&proof, &public).unwrap();
    let bits = proven_udr_bits(FriPreset::P100, &StencilBatch::<Lbm>::security_shape(&proof));
    assert!(bits >= TARGET_BITS, "{bits:.1}");
}
