//! wasm32 smoke test: the toy AIR proves and verifies single-threaded in wasm (Node runner).
//!
//! ```text
//! cargo test --target wasm32-unknown-unknown -p umis-stark --test wasm
//! cargo test --release --target wasm32-unknown-unknown -p umis-stark --test wasm -- --include-ignored  # timings
//! ```

#![cfg(target_arch = "wasm32")]

use p3_matrix::Matrix;
use umis_stark::config::{FriPreset, goldilocks, koala_bear};
use umis_stark::toy::{ToyParams, honest_instance, proof_size, prove, setup, verify};
use wasm_bindgen_test::{console_log, wasm_bindgen_test};
use web_time::Instant;

macro_rules! prove_verify {
    ($field:ident, $params:expr, $preset:expr) => {{
        let config = $field::config($preset);
        let (air, trace) = honest_instance::<$field::Val>($params, 5);
        let pd = setup(&config, &air);
        let t = Instant::now();
        let proof = prove(&config, &air, &trace, &pd).expect("prove");
        let prove_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        verify(&config, &air, &proof, &pd).expect("verify");
        let verify_ms = t.elapsed().as_secs_f64() * 1e3;
        console_log!(
            "{} {} rows x {} cols: prove {:.1} ms, verify {:.1} ms, proof {} B",
            stringify!($field),
            trace.height(),
            trace.width(),
            prove_ms,
            verify_ms,
            proof_size(&proof)
        );
    }};
}

#[wasm_bindgen_test]
fn toy_proves_and_verifies_in_wasm() {
    prove_verify!(goldilocks, ToyParams::new(16, 16, 2), FriPreset::Test);
    prove_verify!(koala_bear, ToyParams::new(16, 16, 2), FriPreset::Test);
}

/// Benchmark-preset timings at 2^14 and 2^16 rows, 50 columns (slow in debug builds).
#[wasm_bindgen_test]
#[ignore]
fn toy_bench_in_wasm() {
    for log_rows in [14, 16] {
        let params = ToyParams::new(256, (1 << log_rows) / 256, 42);
        prove_verify!(goldilocks, params, FriPreset::Benchmark);
        prove_verify!(koala_bear, params, FriPreset::Benchmark);
    }
}
