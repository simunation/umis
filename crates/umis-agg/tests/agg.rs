//! Aggregation round trip on the fast test preset (the R100 numbers are in `examples/agg_bench`).
use task_life2d::Life2dParams;
use task_life2d::board::Board;
use umis_agg::{AggConfig, aggregate, layer1, leaf, verify_recursive};
use umis_stark::config::FriPreset;

fn board(p: &Life2dParams, salt: u64) -> Board {
    let e = (p.n + 2 * p.gens) as usize;
    let mut b = Board::dead(e, e);
    for (i, c) in b.cells.iter_mut().enumerate() {
        *c = ((i as u64 ^ salt).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 5) as u8;
    }
    b
}

#[test]
fn leaves_recurse_and_aggregate() {
    let cfg = AggConfig::new(FriPreset::Test);
    let mut p = Life2dParams::default_for([1, 1, 0], 4);
    (p.n, p.gens, p.strips, p.rows_per_trace_row) = (16, 2, 1, 2);
    let a = leaf(&cfg, &p, 1, &board(&p, 1), &[false; 8]);
    let b = leaf(&cfg, &p, 2, &board(&p, 2), &[false; 8]);
    let (ra, rb) = (layer1(&cfg, &a).unwrap(), layer1(&cfg, &b).unwrap());
    verify_recursive(&cfg, &ra).unwrap();
    let agg = aggregate(&cfg, &ra, &rb).unwrap();
    verify_recursive(&cfg, &agg).unwrap();
}

/// The verifier circuit binds the leaf's public values: a leaf proof presented with another
/// claim's digest cannot be recursed.
#[test]
fn a_leaf_with_a_wrong_claim_does_not_recurse() {
    let cfg = AggConfig::new(FriPreset::Test);
    let mut p = Life2dParams::default_for([1, 1, 0], 4);
    (p.n, p.gens, p.strips, p.rows_per_trace_row) = (16, 2, 1, 2);
    let mut a = leaf(&cfg, &p, 1, &board(&p, 1), &[false; 8]);
    let other = leaf(&cfg, &p, 7, &board(&p, 1), &[false; 8]);
    a.public = other.public;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layer1(&cfg, &a)));
    assert!(!matches!(r, Ok(Ok(_))), "a mismatched claim must not produce a layer-1 proof");
}
