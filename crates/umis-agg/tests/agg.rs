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
