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
    let trace = extend_trace(&air.shape, life);
    let public = public_values(&air.shape, ext, absent, &x_out);
    (air, trace, public, x_out)
}

/// The digests the trace's own sponge columns end in (last row, last permutation).
fn trace_digests(d: &DigestShape, t: &RowMajorMatrix<Val>) -> Vec<Val> {
    let w = t.width();
    let last = &t.values[(d.life.height - 1) * w..][..w];
    REGIONS
        .iter()
        .flat_map(|g| {
            let o = d.perm(*g, d.perms(*g) - 1) + PERM_COLS - WIDTH;
            last[o..o + DIGEST].to_vec()
        })
        .collect()
}

#[test]
fn honest_digest_proofs_verify() {
    let mut rng = ChaCha8Rng::seed_from_u64(3);
    for (n, k, strips, rpr) in
        [(16, 2, 1, 2), (16, 2, 2, 3), (16, 3, 4, 4), (24, 2, 3, 2), (64, 1, 4, 2), (32, 2, 1, 5)]
    {
        let (p, s) = shape(n, k, strips, rpr);
        let e = p.n() + 2 * p.k();
        for round in 0..3 {
            let ext = board(&mut rng, e, 30);
            let absent: [bool; 8] = core::array::from_fn(|_| round > 0 && rng.next_u32() % 3 == 0);
            let (air, trace, public, x_out) = honest(&s, &ext, &absent);
            assert_eq!(x_out, run_extended_masked(&ext, p.k(), &crate::absent_mask(&p, &absent)));
            assert_eq!(trace.width(), air.shape.width());
            // The trace's sponges end in the natively computed digests.
            assert_eq!(trace_digests(&air.shape, &trace), public[..ABSENT].to_vec());
            assert!(accepted(&air, trace, &public, &public), "{n},{k},{strips},{rpr} round {round}");
        }
    }
}

#[test]
fn consecutive_slots_chain() {
    // Slot t + 1 starts from slot t's output (any halo): d_next(t) = d_prev(t + 1), and
    // both are the board root (INV-LIFE-DIG-5). A different board breaks the link.
    let mut rng = ChaCha8Rng::seed_from_u64(5);
    for (n, k, strips, rpr) in [(16, 2, 1, 2), (32, 1, 1, 3), (16, 2, 2, 3)] {
        let (p, s) = shape(n, k, strips, rpr);
        let e = p.n() + 2 * p.k();
        let d = DigestShape::new(s);
        let mut ext = board(&mut rng, e, 35);
        let mut prev_next: Option<Vec<Val>> = None;
        for _ in 0..3 {
            let (_, _, public, x_out) = honest(&s, &ext, &[false; 8]);
            assert_eq!(public[..DIGEST], board_root(&d, &inner(s.n, s.k, &ext)));
            assert_eq!(public[2 * DIGEST..3 * DIGEST], board_root(&d, &x_out));
            if let Some(pn) = &prev_next {
                assert_eq!(pn[..], public[..DIGEST], "{n},{k},{strips},{rpr}");
            }
            prev_next = Some(public[2 * DIGEST..3 * DIGEST].to_vec());
            // next slot: fresh halo, inner = this output
            let mut next = board(&mut rng, e, 35);
            for y in 0..s.n {
                next.cells[(y + s.k) * e + s.k..][..s.n].copy_from_slice(&x_out.cells[y * s.n..][..s.n]);
            }
            ext = next;
        }
        let mut other = inner(s.n, s.k, &ext);
        other.cells[3] ^= 1;
        assert_ne!(board_root(&d, &other), board_root(&d, &inner(s.n, s.k, &ext)));
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
    // The trace's output cell is flipped too, so its sponge ends in the false claim's d_next:
    // only the Conway constraints stand (INV-LIFE-DIG-1).
    let (p, s) = shape(16, 2, 2, 3);
    let mut rng = ChaCha8Rng::seed_from_u64(11);
    let ext = board(&mut rng, p.n() + 2 * p.k(), 30);
    let air = LifeDigestAir::new(s);
    let (mut life, x_out) = generate_trace(&s, &ext, &[false; 8]);
    let mut lie = x_out.clone();
    lie.cells[5 * s.n + 3] ^= 1;
    // Output cell (row 5, column 3) of strip 0: stacked row k + 5, generation k column k + 3.
    let (r, i) = (5 / s.rpr, 5 % s.rpr);
    let w = life.width();
    let c = s.col(s.k, i, s.k + 3);
    life.values[r * w + c] = Val::ONE - life.values[r * w + c];
    let claim = public_values(&air.shape, &ext, &[false; 8], &lie);
    let trace = extend_trace(&air.shape, life);
    assert_eq!(trace_digests(&air.shape, &trace), claim[..ABSENT].to_vec(), "the sponge matches the lie");
    assert!(!accepted(&air, trace, &claim, &claim));
    // The honest trace against the false claim's digest.
    let (_, honest_trace, _, _) = honest(&s, &ext, &[false; 8]);
    assert!(!accepted(&air, honest_trace, &claim, &claim));
}

#[test]
fn a_false_previous_board_is_rejected() {
    // The honest run of E against a claim whose previous board differs in one cell.
    let (p, s) = shape(16, 2, 1, 2);
    let mut rng = ChaCha8Rng::seed_from_u64(12);
    let ext = board(&mut rng, p.n() + 2 * p.k(), 30);
    let (air, trace, public, x_out) = honest(&s, &ext, &[false; 8]);
    let mut ext2 = ext.clone();
    ext2.cells[(2 + s.k) * ext.w + 7 + s.k] ^= 1;
    let claim = public_values(&air.shape, &ext2, &[false; 8], &x_out);
    assert_eq!(claim[DIGEST..], public[DIGEST..], "only d_prev differs");
    assert!(!accepted(&air, trace, &claim, &claim));
}

#[test]
fn generation_zero_must_match_the_digest() {
    // A gen-0 cell switched on in a dead padding row keeps every later generation valid (a
    // lone live cell dies and wakes nobody); only the halo sponge catches it.
    let (p, s) = shape(16, 2, 2, 3);
    assert!(s.rows() > s.strips * s.y + 4 * s.k, "the layout has padding rows");
    let mut rng = ChaCha8Rng::seed_from_u64(13);
    let ext = board(&mut rng, p.n() + 2 * p.k(), 30);
    let (air, mut trace, public, _) = honest(&s, &ext, &[false; 8]);
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
}

/// `trace` with row `r`'s halo permutations recomputed from its own cells (and its honest
/// `cap_in`), every other row unchanged.
fn responge_row(d: &DigestShape, trace: &RowMajorMatrix<Val>, r: usize) -> RowMajorMatrix<Val> {
    let s = &d.life;
    let (w, lw) = (trace.width(), s.width());
    let mut t = trace.clone();
    let cur: Vec<Val> = t.values[r * w..][..lw].to_vec();
    let next: Vec<Val> = t.values[((r + 1) % s.height) * w..][..lw].to_vec();
    let cells = Cells::new(s, &cur, &next);
    let sel = |i: usize| Val::from_bool(d.sel(r, i));
    let sel0 = |i: usize| Val::from_bool(d.sel0(r, i));
    let g = Region::Halo;
    let limbs = limbs(&d.row_cells::<Val, Val>(g, &cells, &sel, &sel0), d.perms(g));
    let row = &mut t.values[r * w..(r + 1) * w];
    let mut st = [Val::ZERO; WIDTH];
    st[RATE..].copy_from_slice(&row[d.base(g)..d.base(g) + CAP]);
    for m in 0..d.perms(g) {
        st[..RATE].copy_from_slice(&limbs[m * RATE..][..RATE]);
        st = permute_into(st, &mut row[d.perm(g, m)..d.perm(g, m) + PERM_COLS]);
    }
    t
}

#[test]
fn lying_about_absent_regions_is_rejected() {
    // A corner zone: the prover evolves its absent halo regions freely and claims the
    // resulting board with the verifier's (true) absent flags in its public values.
    let (p, s) = shape(16, 4, 2, 4);
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
        let trace = extend_trace(&air.shape, life);
        assert!(!accepted(&air, trace.clone(), &claim, &claim));
        // Sanity: the same unmasked run with the lie in the public flags would verify.
        let lie = public_values(&air.shape, &ext, &[false; 8], &x_free);
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
    for (row, col) in [
        (0, d.base(Region::Prev)),
        (5, d.base(Region::Halo) + 3),
        (7, d.perm(Region::Next, 0) + 40),
        (d.life.height - 1, w - 1),
    ] {
        let mut t = trace.clone();
        t.values[row * w + col] += Val::ONE;
        assert!(!accepted(&air, t, &public, &public), "row {row} col {col}");
    }
    // A wrong digest in the public values, in each of the three digests.
    for g in REGIONS {
        let mut bad = public.clone();
        bad[g as usize * DIGEST + 2] += Val::ONE;
        assert!(!accepted(&air, trace.clone(), &bad, &bad), "{g:?}");
    }
    assert!(accepted(&air, trace, &public, &public));
}

#[test]
fn with_one_strip_the_halo_digest_ignores_the_zone_board() {
    // strips = 1: d_halo depends only on E's halo (the neighbour regions), so a checker
    // without the zone's state rebuilds it from the input payloads. With several strips the
    // strip copies of the zone's own cells enter it too.
    let mut rng = ChaCha8Rng::seed_from_u64(23);
    for (strips, independent) in [(1, true), (2, false)] {
        let (p, s) = shape(16, 2, strips, 2);
        let d = DigestShape::new(s);
        let e = p.n() + 2 * p.k();
        let ext = board(&mut rng, e, 40);
        let mut blank = ext.clone();
        for y in 0..s.n {
            blank.cells[(y + s.k) * e + s.k..][..s.n].fill(0);
        }
        assert_eq!(halo_digest(&d, &ext) == halo_digest(&d, &blank), independent, "strips {strips}");
    }
}

#[test]
fn symbolic_degree_is_six_and_the_life_air_three() {
    // Periodic columns count as degree 1: `sel · cell` sponge inputs cubed by the S-box give
    // degree 6 (security inputs must use this, not the declared 3). The life AIR is degree 3.
    for (n, k, strips, rpr) in [(16, 2, 1, 2), (64, 1, 4, 2)] {
        let (_, s) = shape(n, k, strips, rpr);
        assert_eq!(LifeDigestAir::new(s).symbolic_degree(), 6);
    }
}
