//! Every SimNet scenario on one seed (the full sweep is
//! `cargo run --release -p umis-simnet -- --seeds 100 --scenario all`).

use umis_simnet::scenarios::{self, Scenario};

fn ok(name: &str, slots: u64) {
    let s: Scenario = name.parse().unwrap();
    let o = scenarios::run(s, 1, slots);
    assert!(o.report.ok(), "{name}: {:?}", o.report.violations);
}

#[test]
fn basic() {
    ok("basic", 40);
}
#[test]
fn churn() {
    ok("churn", 60);
}
#[test]
fn partition() {
    ok("partition", 60);
}
#[test]
fn adversary() {
    ok("adversary", 40);
}
#[test]
fn faults() {
    ok("faults", 60);
}
#[test]
fn zone_create() {
    ok("zone-create", 50);
}
#[test]
fn zone_reject() {
    ok("zone-reject", 40);
}
#[test]
fn zone_expire() {
    ok("zone-expire", 40);
}
#[test]
fn zone_layers() {
    ok("zone-layers", 50);
}
#[test]
fn zone_sparse() {
    ok("zone-sparse", 50);
}

#[test]
fn junk_proofs() {
    ok("junk-proofs", 30);
}

#[test]
fn junk_proofs_l0() {
    ok("junk-proofs-l0", 30);
}

#[test]
fn same_seed_same_trace() {
    let a = scenarios::run(Scenario::Basic, 7, 15);
    let b = scenarios::run(Scenario::Basic, 7, 15);
    assert_eq!(a.trace, b.trace);
    let c = scenarios::run(Scenario::Basic, 8, 15);
    assert_ne!(a.trace, c.trace);
}

// Real Plonky3 proofs (test FRI preset). Each scenario proves every advancing slot, so they
// are the slowest here; `slots` is the minimum each scenario clamps to.
#[test]
fn stark() {
    ok("stark", 20);
}

/// B8: shares are checked off the event loop; corrupted ones are rejected, never trusted.
#[test]
fn stark_bad_shares() {
    ok("stark-bad-shares", 20);
}

#[test]
fn life_stark_keccak() {
    ok("life-stark-keccak", 20);
}

#[test]
fn life_stark_junk() {
    ok("life-stark-junk", 20);
}

#[test]
fn life_stark_l0() {
    ok("life-stark-l0", 20);
}

// Recursive aggregation (koalabear-agg-test): about 100 s each in release, far longer in
// debug, so release only (`cargo test --release -p umis-simnet`).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn life_agg() {
    ok("life-agg", 20);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn life_agg_junk() {
    ok("life-agg-junk", 20);
}

// The generic local-rule task (task-stencil).
#[test]
fn stencil_life() {
    ok("stencil-life", 30);
}

#[test]
fn stencil_lbm() {
    ok("stencil-lbm", 30);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn stencil_heat() {
    ok("stencil-heat", 30);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn stencil_life_stark() {
    ok("stencil-life-stark", 20);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn stencil_lbm_stark() {
    ok("stencil-lbm-stark", 20);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn stencil_lbm_junk() {
    ok("stencil-lbm-junk", 20);
}

// Stencil commitments aggregated recursively (S4, koalabear-agg-test).
#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn stencil_agg() {
    ok("stencil-agg", 20);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn stencil_agg_lbm() {
    ok("stencil-agg-lbm", 20);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn stencil_agg_junk() {
    ok("stencil-agg-junk", 20);
}

// Gray–Scott (toy 4).
#[test]
fn stencil_gs() {
    ok("stencil-gs", 30);
}

#[test]
#[cfg_attr(debug_assertions, ignore = "release only")]
fn stencil_gs_stark() {
    ok("stencil-gs-stark", 20);
}
