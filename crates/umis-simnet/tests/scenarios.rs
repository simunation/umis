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
fn same_seed_same_trace() {
    let a = scenarios::run(Scenario::Basic, 7, 15);
    let b = scenarios::run(Scenario::Basic, 7, 15);
    assert_eq!(a.trace, b.trace);
    let c = scenarios::run(Scenario::Basic, 8, 15);
    assert_ne!(a.trace, c.trace);
}
