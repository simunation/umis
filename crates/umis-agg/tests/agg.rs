//! Aggregation round trip on the fast test parameters (security numbers: `examples/agg_bench`).
use task_life2d::Life2dParams;
use task_life2d::board::Board;
use umis_agg::{AggParams, aggregate, config, layer1, leaf};

fn board(p: &Life2dParams, salt: u64) -> Board {
    let e = (p.n + 2 * p.gens) as usize;
    let mut b = Board::dead(e, e);
    for (i, c) in b.cells.iter_mut().enumerate() {
        *c = ((i as u64 ^ salt).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 5) as u8;
    }
    b
}

fn small() -> Life2dParams {
    let mut p = Life2dParams::default_for([1, 1, 0], 4);
    (p.n, p.gens, p.strips, p.rows_per_trace_row) = (16, 2, 1, 2);
    p
}

#[test]
fn leaves_recurse_and_aggregate() {
    let (p, lp) = (AggParams::TEST, small());
    let cfg = config(&p);
    let a = leaf(&cfg, &lp, 1, &board(&lp, 1), &[false; 8]);
    let b = leaf(&cfg, &lp, 2, &board(&lp, 2), &[false; 8]);
    let (ra, rb) = (layer1(&p, &a).unwrap(), layer1(&p, &b).unwrap());
    ra.verify().unwrap();
    rb.verify().unwrap();
    let agg = aggregate(&p, &ra, &rb).unwrap();
    agg.verify().unwrap();
    // Each recursive proof verifies only against its own statement.
    assert!(ra.verifier.verify(&ra.out.0, &rb.statement).is_err());
    let mut swapped = agg.statement.clone();
    swapped.rotate_left(ra.statement.len());
    assert!(agg.verifier.verify(&agg.out.0, &swapped).is_err());
}

/// A leaf proof presented with another claim's public values does not recurse.
#[test]
fn a_leaf_with_a_wrong_claim_does_not_recurse() {
    let (p, lp) = (AggParams::TEST, small());
    let cfg = config(&p);
    let mut a = leaf(&cfg, &lp, 1, &board(&lp, 1), &[false; 8]);
    a.public = leaf(&cfg, &lp, 7, &board(&lp, 1), &[false; 8]).public;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layer1(&p, &a)));
    assert!(!matches!(r, Ok(Ok(_))), "a mismatched claim must not produce a layer-1 proof");
}

/// The production parameters clear 101 proven bits for the leaf shapes the CLI uses.
#[test]
fn leaf_shapes_are_secure() {
    for (n, k, strips, rpr) in [(16, 2, 1, 2), (64, 1, 4, 2), (256, 1, 8, 3), (256, 8, 8, 3)] {
        let mut lp = Life2dParams::default_for([1, 1, 0], 4);
        (lp.n, lp.gens, lp.strips, lp.rows_per_trace_row) = (n, k, strips, rpr);
        let s = task_life2d::air::Shape::of(&lp);
        let shape = umis_stark::security::ProofShape {
            log_height: s.height.trailing_zeros() as usize,
            num_constraints: s.num_constraints(),
            max_degree: 3,
            batched_openings: 2 * s.width() + 10,
            logup: None,
        };
        for p in [AggParams::J101, AggParams::U101] {
            let bits = p.bits(&shape);
            assert!(bits >= 101.0, "{n}/{k}/{strips}/{rpr} {p:?}: {bits:.1}");
        }
    }
}

/// The claim-independent leaf (ROADMAP A2, INV-LIFE-DIG-4): one layer-1 owner and one 2-to-1
/// owner, each built once, prove every slot of a zone shape; every recursive proof verifies
/// only against its own statement.
#[test]
fn one_owner_per_shape_proves_many_slots() {
    use umis_agg::{AggOwner, Layer1Owner, digest_leaf};
    let (p, lp) = (AggParams::TEST, small());
    let cfg = config(&p);
    let mut corner = [false; 8];
    corner[0] = true;
    let leaves: Vec<_> = [(1, [false; 8]), (2, corner), (3, [false; 8])]
        .iter()
        .map(|(salt, absent)| digest_leaf(&cfg, &lp, &board(&lp, *salt), absent))
        .collect();
    assert_ne!(leaves[0].public, leaves[1].public);
    let l1 = Layer1Owner::new(&p, &leaves[0]).unwrap();
    let rs: Vec<_> = leaves.iter().map(|l| l1.prove(l).unwrap()).collect();
    for r in &rs {
        r.verify().unwrap();
    }
    assert!(rs[1].verifier.verify(&rs[1].out.0, &rs[0].statement).is_err());

    let agg = AggOwner::new(&p, &rs[0], &rs[1]).unwrap();
    let a01 = agg.prove(&rs[0], &rs[1]).unwrap();
    let a21 = agg.prove(&rs[2], &rs[1]).unwrap();
    a01.verify().unwrap();
    a21.verify().unwrap();
    // One verifier for both aggregates: each verifies against its own statement only.
    assert!(a01.verifier.verify(&a21.out.0, &a21.statement).is_ok());
    assert!(a01.verifier.verify(&a21.out.0, &a01.statement).is_err());
}

/// A digest leaf presented with another claim's statement does not recurse, through an owner
/// built from a third leaf.
#[test]
fn a_digest_leaf_with_a_wrong_claim_does_not_recurse() {
    use umis_agg::{Layer1Owner, digest_leaf};
    let (p, lp) = (AggParams::TEST, small());
    let cfg = config(&p);
    let rep = digest_leaf(&cfg, &lp, &board(&lp, 5), &[false; 8]);
    let l1 = Layer1Owner::new(&p, &rep).unwrap();
    let mut a = digest_leaf(&cfg, &lp, &board(&lp, 1), &[false; 8]);
    a.public = digest_leaf(&cfg, &lp, &board(&lp, 2), &[false; 8]).public;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| l1.prove(&a)));
    assert!(!matches!(r, Ok(Ok(_))), "a mismatched claim must not produce a layer-1 proof");
    // Flipping one absent flag in the statement fails too.
    let mut b = digest_leaf(&cfg, &lp, &board(&lp, 1), &[false; 8]);
    let n = b.public.len();
    b.public[n - 1] += task_life2d_one();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| l1.prove(&b)));
    assert!(!matches!(r, Ok(Ok(_))));
}

fn task_life2d_one() -> p3_koala_bear::KoalaBear {
    use p3_field::PrimeCharacteristicRing;
    p3_koala_bear::KoalaBear::ONE
}

/// The production parameters clear 101 proven bits for the digest leaf too.
#[test]
fn digest_leaf_shapes_are_secure() {
    use task_life2d::air::digest::DigestShape;
    for (n, k, strips, rpr) in [(16, 2, 1, 2), (64, 1, 4, 2), (256, 1, 8, 3), (256, 8, 8, 3)] {
        let mut lp = Life2dParams::default_for([1, 1, 0], 4);
        (lp.n, lp.gens, lp.strips, lp.rows_per_trace_row) = (n, k, strips, rpr);
        let d = DigestShape::new(task_life2d::air::Shape::of(&lp));
        let shape = umis_stark::security::ProofShape {
            log_height: d.life.height.trailing_zeros() as usize,
            num_constraints: d.num_constraints(),
            max_degree: 3,
            batched_openings: 2 * d.width() + 10,
            logup: None,
        };
        for p in [AggParams::J101, AggParams::U101] {
            let bits = p.bits(&shape);
            assert!(bits >= 101.0, "{n}/{k}/{strips}/{rpr} {p:?}: {bits:.1}");
        }
    }
}
