//! Gray–Scott (toy 4): the steady state is a fixed point, a catalyst seed spreads, honest
//! batch proofs verify, and the **release-mode verifier** rejects each staged division moved
//! out of its range (INV-STENCIL-RULE-2).

use crate::batch::StencilBatch;
use crate::batch::statement;
use crate::grayscott::{BITS, GrayScott, U};
use crate::{Grid, Rule, Val};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use umis_stark::config::FriPreset;

fn accepted<R: Rule + Clone>(b: &StencilBatch<R>, traces: Vec<RowMajorMatrix<Val>>, claim: &[Val]) -> bool {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let proof = b.prove(&traces, claim)?;
        b.verify(&proof, claim)
    }));
    matches!(r, Ok(Ok(())))
}

/// The steady state with a square of `u = v = U/2` in the middle.
fn seeded(w: usize) -> Grid {
    let gs = GrayScott::default();
    let mut g = Grid::filled(w, w, &gs.boundary());
    let c = w / 2;
    for y in c - 2..c + 2 {
        for x in c - 2..c + 2 {
            g.cell_mut(x, y).copy_from_slice(&[U as u32 / 2, U as u32 / 2]);
        }
    }
    g
}

#[test]
fn the_steady_state_is_fixed_and_a_seed_spreads() {
    let gs = GrayScott::default();
    let rest = Grid::filled(12, 12, &gs.boundary());
    assert_eq!(rest.evolve(&gs, 1), Grid::filled(10, 10, &gs.boundary()));
    let mut g = seeded(40);
    let covered = |g: &Grid| g.data.chunks(2).filter(|c| c[1] > 50).count();
    let before = covered(&g);
    for _ in 0..200 {
        assert!(gs.provable(&g));
        let next = g.evolve(&gs, 1);
        // keep the frame: steady state outside
        let mut e = Grid::filled(40, 40, &gs.boundary());
        for y in 0..38 {
            for x in 0..38 {
                e.cell_mut(x + 1, y + 1).copy_from_slice(next.cell(x, y));
            }
        }
        g = e;
    }
    assert!(covered(&g) > before, "the catalyst spreads: {} → {}", before, covered(&g));
    assert!(g.data.iter().all(|&x| (x as i64) < 1 << BITS));
}

#[test]
fn honest_gray_scott_batch_proofs_verify() {
    let gs = GrayScott::default();
    for (n, k, rpr) in [(8, 1, 2), (12, 2, 2)] {
        let b = StencilBatch::new(gs, n, k, rpr, FriPreset::Test).unwrap();
        let ext = seeded(b.shape.we);
        let (t, public, next) = b.generate(&ext);
        assert_eq!(next, ext.evolve(&gs, k));
        assert_eq!(public, statement(&b.shape, &ext, &next));
        assert!(accepted(&b, t, &public), "{n},{k},{rpr}");
    }
}

/// The batch leaf of an 8×8 seeded board, the cells trace, its row and the column of the
/// generation-1 cell at slot 1, zone column 4 (inside the seed).
fn forge_site() -> (StencilBatch<GrayScott>, RowMajorMatrix<Val>, usize, usize) {
    let b = StencilBatch::new(GrayScott::default(), 8, 1, 2, FriPreset::Test).unwrap();
    let ext = seeded(b.shape.we);
    let (t, ..) = b.generate(&ext);
    let c = b.cells_only(&t[0]);
    let s = b.shape;
    (b, c, 2, s.col(1, 1, s.halo() + 4, 0))
}

fn get(c: &RowMajorMatrix<Val>, r: usize, col: usize) -> i64 {
    crate::air::to_u32(c.values[r * c.width() + col]) as i64
}

#[test]
fn the_seed_site_is_reactive() {
    // The forgeries below need a cell with v > 0 and R > 0.
    let (_, c, r, col) = forge_site();
    assert!(get(&c, r, col + 2) > 0 && get(&c, r, col + 4) > 0, "w and R at the site");
}

#[test]
fn v_squared_cannot_shift_into_its_remainder() {
    // (w − 1, r1 + 2^13): v² = w·2^13 + r1 still holds, r1 leaves its 13 bits. The forged w
    // changes R, so R, r2 and both next values follow it; the claim follows the forgery.
    let (b, mut c, r, col) = forge_site();
    let gs = GrayScott::default();
    let w_ = c.width();
    let at = |j: usize| r * w_ + col + 2 + j;
    let (w, r1) = (get(&c, r, col + 2), get(&c, r, col + 3));
    let u = {
        // u at generation 0, same slot and column: recompute from the cell's own equation is
        // not needed, R's equation gives it: u·w = R·2^13 + r2
        let (rr, r2) = (get(&c, r, col + 4), get(&c, r, col + 5));
        (rr * (1 << 13) + r2) / w
    };
    let (w2, r1b) = (w - 1, r1 + (1 << 13));
    let (rr2, r22) = ((u * w2) >> 13, (u * w2) & ((1 << 13) - 1));
    let d_r = rr2 - get(&c, r, col + 4);
    // Nu drops by 2^s·dR, Nv rises by it: u' and v' move by ∓dR with the same remainders
    c.values[at(0)] = Val::from_i64(w2);
    c.values[at(1)] = Val::from_i64(r1b);
    c.values[at(2)] = Val::from_i64(rr2);
    c.values[at(3)] = Val::from_i64(r22);
    c.values[r * w_ + col] -= Val::from_i64(d_r);
    c.values[r * w_ + col + 1] += Val::from_i64(d_r);
    let _ = gs;
    let (forged, claim) = b.assemble(&c);
    assert!(!accepted(&b, forged, &claim));
}

#[test]
fn the_update_cannot_round_the_other_way() {
    // u' + 1 with ru − 2^s: the equation holds, ru leaves its range.
    let (b, mut c, r, col) = forge_site();
    let gs = GrayScott::default();
    let w_ = c.width();
    c.values[r * w_ + col] += Val::ONE;
    c.values[r * w_ + col + 2 + 4] -= Val::from_u32(1 << gs.s);
    let (forged, claim) = b.assemble(&c);
    assert!(!accepted(&b, forged, &claim));
}

#[test]
fn out_of_range_states_are_not_provable() {
    let gs = GrayScott::default();
    let mut g = Grid::filled(10, 10, &gs.boundary());
    g.cell_mut(5, 5)[1] = 1 << BITS;
    assert!(!gs.provable(&g));
    let b = StencilBatch::new(gs, 8, 1, 2, FriPreset::Test).unwrap();
    let (t, public, _) = b.generate(&g);
    assert!(!accepted(&b, t, &public));
}

#[test]
fn gray_scott_clears_the_target_at_p100() {
    use umis_stark::security::{TARGET_BITS, proven_udr_bits};
    let b = StencilBatch::new(GrayScott::default(), 32, 1, 2, FriPreset::P100).unwrap();
    let ext = seeded(b.shape.we);
    let (t, public, _) = b.generate(&ext);
    let proof = b.prove(&t, &public).unwrap();
    b.verify(&proof, &public).unwrap();
    let bits = proven_udr_bits(FriPreset::P100, &StencilBatch::<GrayScott>::security_shape(&proof));
    assert!(bits >= TARGET_BITS, "{bits:.1}");
}
