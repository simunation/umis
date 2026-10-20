//! STARK backend round trip; the proof must match the wasm build's (see `wasm.rs`).
//! `cargo test -p task-water3d --features stark --test stark`
#![cfg(feature = "stark")]

mod common;

#[test]
fn proof_is_deterministic_and_matches_golden() {
    let a = common::stark_roundtrip();
    assert_eq!(a, common::stark_roundtrip(), "proving must be deterministic");
    let golden = include_str!("golden-proof-4.txt").trim();
    if golden.is_empty() {
        panic!("golden-proof-4.txt is empty; the proof address is {}", a.to_hex());
    }
    assert_eq!(a.to_hex(), golden);
}
