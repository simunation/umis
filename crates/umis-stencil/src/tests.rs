//! Framework tests on the life rule: the trace equals the reference evolution, honest proofs
//! verify, the degree is the rule's, roots do not depend on the layout, consecutive slots
//! chain, and the **release-mode verifier** rejects false claims. In debug builds the prover's
//! constraint checker may panic first; that counts as a rejection (the meaningful run is
//! `--release`).

use crate::air::{N_PUBLIC, REGIONS, Region, StencilAir, halo_digest, public_values, root};
use crate::life::Life;
use crate::poseidon::{DIGEST, PERM_COLS, WIDTH};
use crate::{Grid, Val};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use p3_uni_stark::{prove, verify};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use umis_stark::config::{FriPreset, koala_bear};

const SHAPES: [(usize, usize, usize); 5] = [(8, 1, 2), (16, 2, 2), (16, 1, 3), (12, 2, 4), (16, 3, 3)];

fn random(rng: &mut ChaCha8Rng, w: usize, pct: u32) -> Grid {
    let mut g = Grid::filled(w, w, &[0]);
    for c in g.data.iter_mut() {
        *c = (rng.next_u32() % 100 < pct) as u32;
    }
    g
}

fn accepted(air: &StencilAir<Life>, trace: RowMajorMatrix<Val>, claim: &[Val]) -> bool {
    let cfg = koala_bear::config(FriPreset::Test);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let proof = prove(&cfg, air, trace, claim).map_err(|e| format!("{e:?}"))?;
        verify(&cfg, air, &proof, claim).map_err(|e| format!("{e:?}"))
    }));
    matches!(r, Ok(Ok(())))
}

/// The digests the trace's sponges end in.
fn trace_digests(air: &StencilAir<Life>, t: &RowMajorMatrix<Val>) -> Vec<Val> {
    let s = &air.shape;
    let w = t.width();
    let last = &t.values[(s.height - 1) * w..][..w];
    REGIONS.iter().flat_map(|g| last[s.out_col(*g, s.rpr - 1)..][..DIGEST].to_vec()).collect()
}

/// The cell columns of a full trace.
fn cells(air: &StencilAir<Life>, t: &RowMajorMatrix<Val>) -> RowMajorMatrix<Val> {
    let (w, cw) = (t.width(), air.shape.cells_width());
    RowMajorMatrix::new(t.values.chunks(w).flat_map(|r| r[..cw].to_vec()).collect(), cw)
}

#[test]
fn the_trace_follows_the_reference_evolution() {
    let mut rng = ChaCha8Rng::seed_from_u64(1);
    for (n, k, rpr) in SHAPES {
        let air = StencilAir::new(Life, n, k, rpr).unwrap();
        for _ in 0..4 {
            let ext = random(&mut rng, air.shape.we, 35);
            let (_, next) = air.generate_trace(&ext);
            assert_eq!(next, ext.evolve(&Life, k), "{n},{k},{rpr}");
        }
    }
}

#[test]
fn honest_proofs_verify() {
    let mut rng = ChaCha8Rng::seed_from_u64(2);
    for (n, k, rpr) in SHAPES {
        let air = StencilAir::new(Life, n, k, rpr).unwrap();
        let ext = random(&mut rng, air.shape.we, 35);
        let (trace, next) = air.generate_trace(&ext);
        let public = public_values(&air.shape, &ext, &next);
        assert_eq!(public.len(), N_PUBLIC);
        assert_eq!(trace_digests(&air, &trace), public, "{n},{k},{rpr}: sponges end in the canonical digests");
        assert!(accepted(&air, trace, &public), "{n},{k},{rpr}");
    }
}

#[test]
fn the_degree_is_the_rules_degree() {
    // INV-STENCIL-DEG-1: digest inputs are committed limbs, gates multiply only columns.
    for (n, k, rpr) in SHAPES {
        assert_eq!(StencilAir::new(Life, n, k, rpr).unwrap().symbolic_degree(), 3);
    }
}

#[test]
fn roots_do_not_depend_on_the_layout() {
    // INV-STENCIL-ROOT-1/2: the same claim gives the same public values for every rpr.
    let mut rng = ChaCha8Rng::seed_from_u64(3);
    let ext = random(&mut rng, 16 + 4, 40);
    let mut seen = None;
    for rpr in [2, 3, 4, 5] {
        let air = StencilAir::new(Life, 16, 2, rpr).unwrap();
        let (trace, next) = air.generate_trace(&ext);
        let public = public_values(&air.shape, &ext, &next);
        assert_eq!(trace_digests(&air, &trace), public, "rpr {rpr}");
        assert_eq!(*seen.get_or_insert(public.clone()), public, "rpr {rpr}");
    }
}

#[test]
fn consecutive_slots_chain() {
    // d_next(t) = d_prev(t + 1) when slot t + 1 starts from slot t's board (any halo).
    let mut rng = ChaCha8Rng::seed_from_u64(4);
    let air = StencilAir::new(Life, 16, 2, 2).unwrap();
    let (n, h, we) = (16, air.shape.halo(), air.shape.we);
    let mut ext = random(&mut rng, we, 35);
    let mut last: Option<Vec<Val>> = None;
    for _ in 0..3 {
        let (_, next) = air.generate_trace(&ext);
        let public = public_values(&air.shape, &ext, &next);
        if let Some(prev) = &last {
            assert_eq!(prev[..], public[..DIGEST]);
        }
        last = Some(public[2 * DIGEST..].to_vec());
        let mut e2 = random(&mut rng, we, 35);
        for y in 0..n {
            for x in 0..n {
                e2.cell_mut(x + h, y + h).copy_from_slice(next.cell(x, y));
            }
        }
        ext = e2;
    }
    let s = &air.shape;
    let b = ext.inner(n, h);
    let mut b2 = b.clone();
    b2.data[5] ^= 1;
    assert_ne!(root(s, &b), root(s, &b2));
    let mut e3 = ext.clone();
    e3.cell_mut(0, 0)[0] ^= 1;
    assert_ne!(halo_digest(s, &ext), halo_digest(s, &e3));
}

#[test]
fn false_claims_are_rejected() {
    let mut rng = ChaCha8Rng::seed_from_u64(5);
    let air = StencilAir::new(Life, 16, 2, 3).unwrap();
    let s = air.shape;
    let ext = random(&mut rng, s.we, 35);
    let (trace, next) = air.generate_trace(&ext);
    let honest = public_values(&s, &ext, &next);
    assert!(accepted(&air, trace.clone(), &honest));

    // A false output, with the output cell flipped in the trace too and the digests recomputed
    // over the tampered cells: only the rule's constraints stand.
    let mut lie = next.clone();
    lie.cell_mut(3, 5)[0] ^= 1;
    let mut c = cells(&air, &trace);
    let (r, i) = (5 / s.rpr, 5 % s.rpr);
    let col = s.col(s.k, i, 3 + s.halo(), 0);
    let cw = c.width();
    c.values[r * cw + col] = Val::ONE - c.values[r * cw + col];
    let forged = air.extend_trace(c);
    let claim = public_values(&s, &ext, &lie);
    assert_eq!(trace_digests(&air, &forged), claim, "the sponges match the lie");
    assert!(!accepted(&air, forged, &claim));
    // The honest trace against the false output, a false previous board, a false halo.
    assert!(!accepted(&air, trace.clone(), &claim));
    let mut e2 = ext.clone();
    e2.cell_mut(s.halo() + 2, s.halo() + 4)[0] ^= 1;
    let claim = public_values(&s, &e2, &next);
    assert_eq!(claim[DIGEST..], honest[DIGEST..], "only d_prev differs");
    assert!(!accepted(&air, trace.clone(), &claim));
    let mut e3 = ext.clone();
    e3.cell_mut(0, 1)[0] ^= 1;
    let claim = public_values(&s, &e3, &next);
    assert_eq!(claim[..DIGEST], honest[..DIGEST]);
    assert_ne!(claim[DIGEST..2 * DIGEST], honest[DIGEST..2 * DIGEST]);
    assert!(!accepted(&air, trace, &claim));
}

#[test]
fn a_padding_cell_must_stay_valid() {
    // A generation-0 cell set to 2 in a padding row (outside every digest and every zone
    // cone): only the rule's state constraint (booleanity) catches it.
    let air = StencilAir::new(Life, 8, 1, 2).unwrap();
    let s = air.shape;
    assert!(s.rows() > s.we + 2, "the layout has padding rows");
    let ext = Grid::filled(s.we, s.we, &[0]);
    let (trace, next) = air.generate_trace(&ext);
    let public = public_values(&s, &ext, &next);
    let mut c = cells(&air, &trace);
    let y = s.we + 1;
    let (r, i) = (y / s.rpr, y % s.rpr);
    let w = c.width();
    c.values[r * w + s.col(0, i, s.we / 2, 0)] = Val::from_u32(2);
    // Generation 1 around it stays dead, but f changes (9 → 3 at the cell: c = 0, a = 2; 9 → 1
    // next to it: c = 2, a = 0): update the inverse witnesses so that only the state
    // constraint is violated.
    let x = s.we / 2;
    let (c0, cone_w) = s.cone(1);
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (yy, xx) = ((y as i64 + dy) as usize, (x as i64 + dx) as usize);
            if xx < c0 || xx >= c0 + cone_w {
                continue;
            }
            let (r1, j) = ((yy - 1) / s.rpr, (yy - 1) % s.rpr);
            let f = if (dx, dy) == (0, 0) { 3 } else { 1 };
            c.values[r1 * w + s.col(1, j, xx, 1)] = p3_field::Field::inverse(&Val::from_u32(f));
        }
    }
    let forged = air.extend_trace(c);
    assert!(!accepted(&air, forged, &public));
}

#[test]
fn tampered_digest_columns_are_rejected() {
    let mut rng = ChaCha8Rng::seed_from_u64(6);
    let air = StencilAir::new(Life, 16, 2, 2).unwrap();
    let s = air.shape;
    let ext = random(&mut rng, s.we, 35);
    let (trace, next) = air.generate_trace(&ext);
    let public = public_values(&s, &ext, &next);
    let w = trace.width();
    for (row, col) in [
        (1, s.slot_base(Region::Prev, 0)),
        (3, s.perm_col(Region::Halo, 1, 0) + 17),
        (2, s.out_col(Region::Next, 0) + 9),
        (0, s.region_base(Region::Halo)),
        (s.height - 1, w - 1),
    ] {
        let mut t = trace.clone();
        t.values[row * w + col] += Val::ONE;
        assert!(!accepted(&air, t, &public), "row {row} col {col}");
    }
    let _ = (WIDTH, PERM_COLS);
}

/// Two runs of one shape whose boards differ in one zone cell (row 8, away from the torus wrap): `(air, a, b,
/// claim_b)` with `claim_b` the public values of run `b`.
fn two_runs() -> (StencilAir<Life>, RowMajorMatrix<Val>, RowMajorMatrix<Val>, Vec<Val>, Vec<Val>) {
    let mut rng = ChaCha8Rng::seed_from_u64(7);
    let air = StencilAir::new(Life, 16, 1, 2).unwrap();
    let s = air.shape;
    let ext = random(&mut rng, s.we, 35);
    let mut ext_b = ext.clone();
    ext_b.cell_mut(s.halo() + 5, s.halo() + 8)[0] ^= 1; // zone row 8 (trace rows 4–5)
    let (ta, na) = air.generate_trace(&ext);
    let (tb, nb) = air.generate_trace(&ext_b);
    let (pa, pb) = (public_values(&s, &ext, &na), public_values(&s, &ext_b, &nb));
    assert_ne!(pa[..DIGEST], pb[..DIGEST]);
    (air, ta, tb, pa, pb)
}

#[test]
fn digest_columns_are_bound_to_the_cells() {
    // Run a's cells with run b's digest columns (limbs, permutations, states): only the limb
    // constraints tie the sponges to the cells (INV-STENCIL-DIG-1).
    let (air, ta, tb, pa, pb) = two_runs();
    let (w, cw) = (ta.width(), air.shape.cells_width());
    let mut t = ta.clone();
    for r in 0..air.shape.height {
        t.values[r * w + cw..(r + 1) * w].copy_from_slice(&tb.values[r * w + cw..(r + 1) * w]);
    }
    assert!(!accepted(&air, t, &pb));
    assert!(accepted(&air, ta, &pa));
}

#[test]
fn the_sponge_state_is_the_multiplexed_permutation() {
    // Run a honestly, but each region's final state (last row, last slot) replaced by run b's
    // digest: only the multiplexer constraint stands (INV-STENCIL-DIG-2).
    let (air, ta, _, _, pb) = two_runs();
    let s = air.shape;
    let w = ta.width();
    let mut t = ta.clone();
    let last = (s.height - 1) * w;
    for g in REGIONS {
        let o = last + s.out_col(g, s.rpr - 1);
        t.values[o..o + DIGEST].copy_from_slice(&pb[g as usize * DIGEST..][..DIGEST]);
    }
    assert!(!accepted(&air, t, &pb));
}

#[test]
fn the_sponge_chain_cannot_restart() {
    // Rows ≥ r0 (past every difference between the runs) take run b's digest columns, whose
    // limbs equal run a's there: only the row-to-row chaining stands (INV-STENCIL-DIG-3).
    let (air, ta, tb, _, pb) = two_runs();
    let (w, cw) = (ta.width(), air.shape.cells_width());
    let r0 = 7;
    let mut t = ta.clone();
    for r in r0..air.shape.height {
        assert_eq!(ta.values[r * w..r * w + cw], tb.values[r * w..r * w + cw], "cells agree from row {r0}");
        t.values[r * w + cw..(r + 1) * w].copy_from_slice(&tb.values[r * w + cw..(r + 1) * w]);
    }
    assert!(!accepted(&air, t, &pb));
}

#[test]
fn p100_clears_the_target_for_every_shape() {
    // INV-STENCIL-PARAM-1: MIN_HEIGHT = 64 keeps every shape at ≥ 101 proven bits (P100, UDR).
    use umis_stark::security::{TARGET_BITS, proven_udr_bits};
    for (n, k, rpr) in [(8, 1, 2), (16, 2, 2), (64, 1, 2), (256, 1, 2), (256, 2, 2), (256, 3, 4)] {
        let sh = StencilAir::new(Life, n, k, rpr).unwrap().security_shape();
        let bits = proven_udr_bits(FriPreset::P100, &sh);
        assert!(bits >= TARGET_BITS, "{n},{k},{rpr}: {bits:.1} bits");
    }
}
