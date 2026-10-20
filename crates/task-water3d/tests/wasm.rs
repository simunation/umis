//! native == wasm: the wasm build must reproduce the native golden trace.
//! `cargo test --target wasm32-unknown-unknown -p task-water3d --test wasm`
#![cfg(target_arch = "wasm32")]

mod common;

use wasm_bindgen_test::*;

#[wasm_bindgen_test]
fn golden_trace_matches_native() {
    let mut sim = common::domain(8);
    sim.run_until(64).unwrap();
    assert_eq!(sim.trace_hash().to_hex(), include_str!("golden-trace-8.txt"));
}

/// The wasm prover (what browser workers run) proves and verifies, and
/// produces the same proof as native.
#[cfg(feature = "stark")]
#[wasm_bindgen_test]
fn stark_proof_matches_native() {
    let a = common::stark_roundtrip();
    assert_eq!(a.to_hex(), include_str!("golden-proof-4.txt").trim());
}

/// Benchmark-preset timings of the wasm prover (single-threaded, as in a
/// browser worker): `cargo test --release --target wasm32-unknown-unknown
/// -p task-water3d --features stark --test wasm -- --include-ignored`.
#[cfg(feature = "stark")]
#[wasm_bindgen_test]
#[ignore]
fn stark_bench_in_wasm() {
    let now = || web_time::Instant::now().duration_since(start()).as_secs_f64() * 1e3;
    for n in [4, 8, 16] {
        let r = common::stark_roundtrip_with(n, umis_stark::config::FriPreset::Benchmark, &now);
        console_log!(
            "n_loc {n:2}: prove {:.0} ms, verify {:.0} ms, proof {} KB",
            r.prove_ms,
            r.verify_ms,
            r.bytes / 1000
        );
    }
}

#[cfg(feature = "stark")]
fn start() -> web_time::Instant {
    static START: std::sync::OnceLock<web_time::Instant> = std::sync::OnceLock::new();
    *START.get_or_init(web_time::Instant::now)
}
