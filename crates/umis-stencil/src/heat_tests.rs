//! The heat-diffusion toy (S2): reference evolution, honest proofs, degree 2, a physics sanity
//! check, and the **release-mode verifier** rejecting wrong rounding and out-of-range values.

use crate::air::{StencilAir, public_values};
use crate::heat::Heat;
use crate::{Grid, Rule, Val};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use p3_uni_stark::{prove, verify};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use umis_stark::config::{FriPreset, koala_bear};

fn heat() -> Heat {
    Heat::new(16, 3, 4, 1000) // α = 3/16, 16-bit temperatures, a 1000 wall
}

fn random(rng: &mut ChaCha8Rng, w: usize) -> Grid {
    let mut g = Grid::filled(w, w, &[0]);
    for c in g.data.iter_mut() {
        *c = rng.next_u32() % (1 << 16);
    }
    g
}

fn accepted<R: Rule + Clone>(air: &StencilAir<R>, trace: RowMajorMatrix<Val>, claim: &[Val]) -> bool {
    let cfg = koala_bear::config(FriPreset::Test);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let proof = prove(&cfg, air, trace, claim).map_err(|e| format!("{e:?}"))?;
        verify(&cfg, air, &proof, claim).map_err(|e| format!("{e:?}"))
    }));
    matches!(r, Ok(Ok(())))
}

fn cells<R: Rule + Clone>(air: &StencilAir<R>, t: &RowMajorMatrix<Val>) -> RowMajorMatrix<Val> {
    let (w, cw) = (t.width(), air.shape.cells_width());
    RowMajorMatrix::new(t.values.chunks(w).flat_map(|r| r[..cw].to_vec()).collect(), cw)
}

#[test]
fn heat_follows_the_reference_and_verifies() {
    let mut rng = ChaCha8Rng::seed_from_u64(1);
    for (n, k, rpr) in [(8, 1, 2), (8, 2, 2), (12, 1, 3)] {
        let air = StencilAir::new(heat(), n, k, rpr).unwrap();
        assert_eq!(air.symbolic_degree(), 3, "max(rule degree 2, Poseidon2 S-box 3)");
        let ext = random(&mut rng, air.shape.we);
        let (trace, next) = air.generate_trace(&ext);
        assert_eq!(next, ext.evolve(&heat(), k));
        let public = public_values(&air.shape, &ext, &next);
        assert!(accepted(&air, trace, &public), "{n},{k},{rpr}");
    }
}

#[test]
fn diffusion_smooths_and_stays_in_range() {
    // A convex combination: no new maximum or minimum, and a hot spot spreads.
    let mut g = Grid::filled(12, 12, &[1000]);
    g.cell_mut(6, 6)[0] = 60_000;
    let out = g.evolve(&heat(), 3);
    let (lo, hi) = (*out.data.iter().min().unwrap(), *out.data.iter().max().unwrap());
    assert!(lo >= 1000 && hi < 60_000 && out.cell(3, 3)[0] > 1000, "{lo} {hi}");
}

#[test]
fn wrong_rounding_is_rejected() {
    // Round one cell up instead of down: u' + 1 with its bits, rem − 2^s cannot be written
    // in s bits; the prover keeps rem's honest bits, so only the rounding equation fails.
    let mut rng = ChaCha8Rng::seed_from_u64(2);
    let h = heat();
    let air = StencilAir::new(h, 8, 1, 2).unwrap();
    let s = air.shape;
    let ext = random(&mut rng, s.we);
    let (trace, next) = air.generate_trace(&ext);
    let mut c = cells(&air, &trace);
    let w = c.width();
    let (x, i, r) = (s.halo() + 3, 1, 1); // a zone cell of generation 1
    let col = s.col(1, i, x, 0);
    let v = crate::air::to_u32(c.values[r * w + col]) + 1;
    c.values[r * w + col] = Val::from_u32(v);
    for j in 0..h.bits {
        c.values[r * w + col + 1 + j] = Val::from_bool(v >> j & 1 == 1);
    }
    let y = s.rpr * r + i + 1 - s.halo();
    let mut lie = next.clone();
    lie.cell_mut(x - s.halo(), y)[0] = v;
    let forged = air.extend_trace(c);
    let claim = public_values(&s, &ext, &lie);
    assert!(!accepted(&air, forged, &claim));
}

#[test]
fn out_of_range_values_are_rejected() {
    // A claim whose halo holds 2^16 + 5: the trace is the honest run of that board (the
    // neighbours' rounding stays in range) and the digests match the claim; only the state
    // recomposition (16 bits) catches the invalid value.
    let h = heat();
    let air = StencilAir::new(h, 8, 1, 2).unwrap();
    let s = air.shape;
    let mut ext = Grid::filled(s.we, s.we, &[1000]);
    ext.cell_mut(4, 0)[0] = (1 << 16) + 5;
    let (trace, next) = air.generate_trace(&ext);
    let claim = public_values(&s, &ext, &next);
    assert!(!accepted(&air, trace, &claim));
}

#[test]
fn rounding_bits_must_be_bits() {
    // Round up (u' + 1) and write rem − 2^s with non-boolean "bits" (b_0 = rem − 2^s): the
    // rounding equation and the recomposition hold; only booleanity catches it.
    let mut rng = ChaCha8Rng::seed_from_u64(3);
    let h = heat();
    let air = StencilAir::new(h, 8, 1, 2).unwrap();
    let s = air.shape;
    let ext = random(&mut rng, s.we);
    let (trace, next) = air.generate_trace(&ext);
    let mut c = cells(&air, &trace);
    let w = c.width();
    let (x, i, r) = (s.halo() + 3, 1, 1);
    let col = s.col(1, i, x, 0);
    let v = crate::air::to_u32(c.values[r * w + col]) + 1;
    assert!(v < 1 << 16);
    c.values[r * w + col] = Val::from_u32(v);
    for j in 0..h.bits {
        c.values[r * w + col + 1 + j] = Val::from_bool(v >> j & 1 == 1);
    }
    let rem: u32 = (0..h.s).map(|j| crate::air::to_u32(c.values[r * w + col + 1 + h.bits + j]) << j).sum();
    c.values[r * w + col + 1 + h.bits] = Val::from_u32(rem) - Val::from_u32(1 << h.s);
    for j in 1..h.s {
        c.values[r * w + col + 1 + h.bits + j] = Val::ZERO;
    }
    let y = s.rpr * r + i + 1 - s.halo();
    let mut lie = next.clone();
    lie.cell_mut(x - s.halo(), y)[0] = v;
    let forged = air.extend_trace(c);
    let claim = public_values(&s, &ext, &lie);
    assert!(!accepted(&air, forged, &claim));
}

#[test]
fn heat_clears_the_target_at_p100() {
    use umis_stark::security::{TARGET_BITS, proven_udr_bits};
    for (n, k, rpr) in [(8, 1, 2), (64, 1, 2), (128, 2, 2)] {
        let sh = StencilAir::new(heat(), n, k, rpr).unwrap().security_shape();
        let bits = proven_udr_bits(FriPreset::P100, &sh);
        assert!(bits >= TARGET_BITS, "{n},{k},{rpr}: {bits:.1} bits");
    }
}
