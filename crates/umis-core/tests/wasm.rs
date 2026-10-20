//! The same bmt-js vectors under wasm32 (32-bit usize): `cargo test --target
//! wasm32-unknown-unknown -p umis-core --test wasm` (wasm-bindgen-test-runner).
#![cfg(target_arch = "wasm32")]

use umis_core::SegRecord;
use umis_core::bmt;
use umis_core::records::*;
use wasm_bindgen_test::*;

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 7 + (i >> 8) * 13 + 5) & 0xff) as u8).collect()
}

#[wasm_bindgen_test]
fn bmt_vectors_match_on_wasm() {
    let v: serde_json::Value = serde_json::from_str(include_str!("vectors/bmt-vectors.json")).unwrap();
    assert_eq!(bmt::chunk_address(&[1, 2, 3]).to_hex(), v["chunk"]["addr_1_2_3"].as_str().unwrap());
    for f in v["files"].as_array().unwrap() {
        let size = f["size"].as_u64().unwrap() as usize;
        if size > 2_000_000 {
            continue;
        }
        assert_eq!(bmt::file_address(&pattern(size)).to_hex(), f["address"].as_str().unwrap(), "size {size}");
    }
    let zc = v["seg"].as_array().unwrap().iter().find(|s| s["record"] == "ZoneCommitment").unwrap();
    assert_eq!(ZoneCommitment::sample().address().to_hex(), zc["address"].as_str().unwrap());
}
