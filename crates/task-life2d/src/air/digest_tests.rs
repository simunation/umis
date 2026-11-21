//! End-to-end tests of the digest AIR: honest proofs verify against the verifier's public
//! values, and the **release-mode verifier** rejects false claims (INV-LIFE-DIG-*). In debug
//! builds the prover's constraint checker may panic first; that counts as a rejection here,
//! the meaningful run is `--release`.

use super::*;
use crate::Life2dParams;
use crate::air::generate_trace;
use crate::board::run_extended_masked;
use p3_field::Field;
use p3_uni_stark::{prove, verify};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use umis_stark::config::{FriPreset, koala_bear};

fn shape(n: u32, k: u32, strips: u32, rpr: u32) -> (Life2dParams, Shape) {
    let mut p = Life2dParams::default_for([1, 1, 0], 4);
    (p.n, p.gens, p.strips, p.rows_per_trace_row) = (n, k, strips, rpr);
    (p.clone(), Shape::of(&p))
}

fn board(rng: &mut ChaCha8Rng, e: usize, pct: u32) -> Board {
    let mut b = Board::dead(e, e);
    for c in b.cells.iter_mut() {
        *c = (rng.next_u32() % 100 < pct) as u8;
    }
    b
}

/// Proves `trace` against `public` and verifies the proof against `claim` (the verifier's
/// public values). `true` when the verifier accepts.
fn accepted(air: &LifeDigestAir, trace: RowMajorMatrix<Val>, public: &[Val], claim: &[Val]) -> bool {
    let cfg = koala_bear::config(FriPreset::Test);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let proof = prove(&cfg, air, trace, public).map_err(|e| format!("{e:?}"))?;
        verify(&cfg, air, &proof, claim).map_err(|e| format!("{e:?}"))
    }));
    matches!(r, Ok(Ok(())))
}

fn honest(s: &Shape, ext: &Board, absent: &[bool; 8]) -> (LifeDigestAir, RowMajorMatrix<Val>, Vec<Val>, Board) {
    let air = LifeDigestAir::new(*s);
    let (life, x_out) = generate_trace(s, ext, absent);
    let trace = extend_trace(&air.shape, life, ext, &x_out);
    let public = public_values(&air.shape, ext, absent, &x_out);
    (air, trace, public, x_out)
}

#[test]
fn honest_digest_proofs_verify() {
    let mut rng = ChaCha8Rng::seed_from_u64(3);
    for (n, k, strips, rpr) in [(16, 2, 1, 2), (16, 2, 2, 3), (16, 3, 4, 4), (24, 4, 3, 2), (64, 1, 4, 2)] {
        let (p, s) = shape(n, k, strips, rpr);
        let e = p.n() + 2 * p.k();
        for round in 0..3 {
            let ext = board(&mut rng, e, 30);
            let absent: [bool; 8] = core::array::from_fn(|_| round > 0 && rng.next_u32() % 3 == 0);
            let (air, trace, public, x_out) = honest(&s, &ext, &absent);
            assert_eq!(x_out, run_extended_masked(&ext, p.k(), &crate::absent_mask(&p, &absent)));
            assert_eq!(trace.width(), air.shape.width());
            assert!(accepted(&air, trace, &public, &public), "{n},{k},{strips},{rpr} round {round}");
        }
    }
}

#[test]
fn one_air_serves_every_claim_of_a_shape() {
    // The AIR (and its periodic columns) depends on the shape only (INV-LIFE-DIG-4).
    let (p, s) = shape(16, 2, 2, 3);
    let e = p.n() + 2 * p.k();
    let mut rng = ChaCha8Rng::seed_from_u64(9);
    let (a1, b1) = (board(&mut rng, e, 30), board(&mut rng, e, 40));
    let (air1, ..) = honest(&s, &a1, &[false; 8]);
    let (air2, trace2, public2, _) = honest(&s, &b1, &[true, false, false, true, false, false, false, false]);
    assert_eq!(air1.periodic, air2.periodic);
    assert!(accepted(&air1, trace2, &public2, &public2));
}

#[test]
fn a_false_output_is_rejected() {
    // The prover commits to the sponge of the false claim (E, X_out') with one output cell
    // flipped in the trace too: only the Conway constraints stand (INV-LIFE-DIG-1).
    let (p, s) = shape(16, 2, 2, 3);
    let mut rng = ChaCha8Rng::seed_from_u64(11);
    let ext = board(&mut rng, p.n() + 2 * p.k(), 30);
    let air = LifeDigestAir::new(s);
    let (mut life, x_out) = generate_trace(&s, &ext, &[false; 8]);
    let mut lie = x_out.clone();
    lie.cells[5 * s.n + 3] ^= 1;
    // Output cell (row 5, column 3) of strip 0: stacked row k + 5, generation k column k + 3.
    let y = s.k + 5;
    let (r, i) = ((y - s.k) / s.rpr, (y - s.k) % s.rpr);
    let w = life.width();
    let c = s.col(s.k, i, s.k + 3);
    life.values[r * w + c] = Val::ONE - life.values[r * w + c];
    let claim = public_values(&air.shape, &ext, &[false; 8], &lie);
    let trace = extend_trace(&air.shape, life, &ext, &lie);
    assert!(!accepted(&air, trace, &claim, &claim));
    // The honest trace against the false claim's digest.
    let (_, honest_trace, _, _) = honest(&s, &ext, &[false; 8]);
    assert!(!accepted(&air, honest_trace, &claim, &claim));
}

#[test]
fn generation_zero_must_match_the_digest() {
    // A gen-0 cell switched on in a dead padding row keeps every later generation valid (a
    // lone live cell dies and wakes nobody); only the sponge absorption catches it.
    let (p, s) = shape(16, 2, 2, 3);
    assert!(s.rows() > s.strips * s.y + 4 * s.k, "the layout has padding rows");
    let mut rng = ChaCha8Rng::seed_from_u64(13);
    let ext = board(&mut rng, p.n() + 2 * p.k(), 30);
    let (air, mut trace, public, x_out) = honest(&s, &ext, &[false; 8]);
    let y = s.strips * s.y + 2 * s.k + 1;
    let (r, i) = (y / s.rpr, y % s.rpr);
    let w = trace.width();
    let x = s.ws / 2;
    trace.values[r * w + s.col0(i, x)] = Val::ONE;
    // Generation 1 around it stays dead, but its f changes (9 → 6 at the cell, 9 → 4 next
    // to it): update the inverse witnesses so that the Conway constraints hold.
    for yy in y - 1..=y + 1 {
        for xx in x - 1..=x + 1 {
            let (r1, i1) = ((yy - 1) / s.rpr, (yy - 1) % s.rpr);
            let f = if (yy, xx) == (y, x) { 6 } else { 4 };
            trace.values[r1 * w + s.col(1, i1, xx) + 1] = Val::from_u32(f).inverse();
        }
    }
    let forged = responge_row(&air.shape, &trace, r);
    // Sponge left as the honest claim's: the absorption of row r fails.
    assert!(!accepted(&air, trace, &public, &public));
    // Row r re-absorbed from the tampered cells, row r + 1 restarting from the honest
    // capacity: the chain constraint fails (INV-LIFE-DIG-2).
    assert!(!accepted(&air, forged, &public, &public));
    assert_eq!(x_out, generate_trace(&s, &ext, &[false; 8]).1);
}

/// `trace` with row `r`'s permutations recomputed from its own cells (and its honest
/// `cap_in`), every other row unchanged.
fn responge_row(d: &DigestShape, trace: &RowMajorMatrix<Val>, r: usize) -> RowMajorMatrix<Val> {
    let s = &d.life;
    let w = trace.width();
    let mut t = trace.clone();
    let row = &mut t.values[r * w..(r + 1) * w];
    let g0: Vec<u8> = (0..s.rpr * s.ws).map(|c| (row[c] == Val::ONE) as u8).collect();
    let out: Vec<u8> = (0..s.rpr)
        .flat_map(|i| (0..s.w).map(move |x| (i, x)))
        .map(|(i, x)| if d.sel(r, i) { (row[s.col(s.k, i, s.k + x)] == Val::ONE) as u8 } else { 0 })
        .collect();
    let mut limbs: Vec<Val> =
        g0.chunks(LIMB_BITS).chain(out.chunks(LIMB_BITS)).map(|c| super::super::pack_limb(c.iter().copied())).collect();
    limbs.resize(d.perms() * RATE, Val::ZERO);
    let mut st = [Val::ZERO; WIDTH];
    st[RATE..].copy_from_slice(&row[d.cap_in()..d.cap_in() + WIDTH - RATE]);
    for m in 0..d.perms() {
        st[..RATE].copy_from_slice(&limbs[m * RATE..][..RATE]);
        st = permute_into(st, &mut row[d.perm(m)..d.perm(m) + PERM_COLS]);
    }
    t
}

#[test]
fn lying_about_absent_regions_is_rejected() {
    // A corner zone: the prover evolves its absent halo regions freely and claims the
    // resulting board with the verifier's (true) absent flags in its public values.
    let (p, s) = shape(16, 4, 2, 2);
    let mut absent = [false; 8];
    for (i, port) in crate::PORTS.iter().enumerate() {
        let d = port_dir(*port).expect("port");
        absent[i] = d[0] < 0 || d[1] < 0;
    }
    assert_eq!(absent.iter().filter(|a| **a).count(), 5);
    let air = LifeDigestAir::new(s);
    let mut rng = ChaCha8Rng::seed_from_u64(17);
    let mut caught = 0;
    for _ in 0..6 {
        let ext = board(&mut rng, p.n() + 2 * p.k(), 35);
        let (life, x_free) = generate_trace(&s, &ext, &[false; 8]);
        if x_free == generate_trace(&s, &ext, &absent).1 {
            continue;
        }
        caught += 1;
        let claim = public_values(&air.shape, &ext, &absent, &x_free);
        let trace = extend_trace(&air.shape, life, &ext, &x_free);
        assert!(!accepted(&air, trace, &claim, &claim));
        // Sanity: the same unmasked run with the lie in the public flags would verify.
        let (life, _) = generate_trace(&s, &ext, &[false; 8]);
        let lie = public_values(&air.shape, &ext, &[false; 8], &x_free);
        let trace = extend_trace(&air.shape, life, &ext, &x_free);
        assert!(accepted(&air, trace, &lie, &lie));
        assert_ne!(lie, claim);
    }
    assert!(caught > 0, "some board exercises the mask");
}

#[test]
fn tampered_sponge_columns_are_rejected() {
    let (p, s) = shape(16, 2, 2, 3);
    let mut rng = ChaCha8Rng::seed_from_u64(19);
    let ext = board(&mut rng, p.n() + 2 * p.k(), 30);
    let (air, trace, public, _) = honest(&s, &ext, &[false; 8]);
    let d = air.shape;
    let w = trace.width();
    for (row, col) in [(0, d.cap_in()), (5, d.cap_in() + 3), (7, d.perm(0) + 40), (d.life.height - 1, w - 1)] {
        let mut t = trace.clone();
        t.values[row * w + col] += Val::ONE;
        assert!(!accepted(&air, t, &public, &public), "row {row} col {col}");
    }
    // A wrong digest in the public values.
    let mut bad = public.clone();
    bad[2] += Val::ONE;
    assert!(!accepted(&air, trace.clone(), &bad, &bad));
    assert!(accepted(&air, trace, &public, &public));
}
