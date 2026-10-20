//! Toy AIR: honest proofs verify, tampered witnesses / claims are rejected.
//!
//! A bad witness is "rejected" if either the prover panics (debug builds run Plonky3's constraint
//! and bus-balance checks before proving) or verification fails (release builds, or a lie only
//! the verifier sees). This keeps the tests meaningful under `--release` too.

#![cfg(not(target_arch = "wasm32"))]

use std::panic::{AssertUnwindSafe, catch_unwind};

use p3_field::{Field, PrimeCharacteristicRing, PrimeField64};
use p3_matrix::dense::RowMajorMatrix;
use umis_stark::config::FriPreset;
use umis_stark::toy::{
    BIAS, SHIFT, ToyAir, ToyParams, WEIGHTS, col, fill_range_multiplicities, generate_trace, honest_instance,
    run_reference, sample_input,
};

/// N = 16 cells, T = 16 steps: 256 rows (the minimum for the 8-bit output range table).
fn params() -> ToyParams {
    ToyParams::new(16, 16, 2)
}

/// Row index of (step t, cell i).
fn row(p: &ToyParams, t: usize, i: usize) -> usize {
    t * p.n_cells + i
}

fn get<F: Copy>(trace: &RowMajorMatrix<F>, r: usize, c: usize) -> F {
    trace.values[r * trace.width + c]
}

fn cell<F>(trace: &mut RowMajorMatrix<F>, r: usize, c: usize) -> &mut F {
    let w = trace.width;
    &mut trace.values[r * w + c]
}

fn set<F>(trace: &mut RowMajorMatrix<F>, r: usize, c: usize, v: F) {
    *cell(trace, r, c) = v;
}

/// Recompute `q_next`, `rem` (honest split) and the dummy chain of row `r` from its
/// `q/left/right`, so the row's local constraints hold again.
fn rederive_row<F: PrimeField64>(p: &ToyParams, trace: &mut RowMajorMatrix<F>, r: usize) {
    let signed = |x: F| {
        let v = x.as_canonical_u64();
        if v > F::ORDER_U64 / 2 { v as i64 - F::ORDER_U64 as i64 } else { v as i64 }
    };
    let (q, l, rt) = (get(trace, r, col::Q), get(trace, r, col::LEFT), get(trace, r, col::RIGHT));
    let sum = WEIGHTS[0] * signed(q) + WEIGHTS[1] * signed(l) + WEIGHTS[2] * signed(rt) + BIAS;
    set(trace, r, col::Q_NEXT, F::from_i64(sum.div_euclid(1 << SHIFT)));
    set(trace, r, col::REM, F::from_i64(sum.rem_euclid(1 << SHIFT)));
    let mut expected = q * l + F::ONE;
    for k in 0..p.extra_cols {
        set(trace, r, col::DUMMY + k, expected);
        expected = expected * rt + q + F::from_usize(k + 1);
    }
    fill_range_multiplicities(p, trace);
}

macro_rules! toy_tests {
    ($field:ident) => {
        mod $field {
            use super::*;
            use umis_stark::config::$field::{Config, Val};
            use umis_stark::toy::{proof_size, prove, setup, verify};

            fn config() -> Config {
                umis_stark::config::$field::config(FriPreset::Test)
            }

            /// Prove with `prover_air` and verify with `verifier_air`; `true` iff accepted.
            fn accepted(prover_air: &ToyAir<Val>, verifier_air: &ToyAir<Val>, trace: &RowMajorMatrix<Val>) -> bool {
                let config = config();
                let pd = setup(&config, prover_air);
                let proof = catch_unwind(AssertUnwindSafe(|| prove(&config, prover_air, trace, &pd)));
                match proof {
                    Ok(Ok(proof)) => verify(&config, verifier_air, &proof, &pd).is_ok(),
                    Ok(Err(_)) | Err(_) => false,
                }
            }

            fn honest() -> (ToyAir<Val>, RowMajorMatrix<Val>) {
                honest_instance(params(), 7)
            }

            #[test]
            fn honest_proves_and_verifies() {
                let config = config();
                let (air, trace) = honest();
                let pd = setup(&config, &air);
                let proof = prove(&config, &air, &trace, &pd).expect("prove");
                verify(&config, &air, &proof, &pd).expect("verify");

                // Serde round trip, then verify the decoded proof.
                let bytes = postcard::to_allocvec(&proof).expect("serialize");
                assert_eq!(bytes.len(), proof_size(&proof));
                let decoded = postcard::from_bytes(&bytes).expect("deserialize");
                verify(&config, &air, &decoded, &pd).expect("verify decoded");
            }

            #[test]
            fn setup_is_reusable_across_claims() {
                let config = config();
                let p = params();
                let (air_a, trace_a) = honest_instance::<Val>(p, 1);
                let (air_b, trace_b) = honest_instance::<Val>(p, 2);
                let pd = setup(&config, &air_a);
                let proof_b = prove(&config, &air_b, &trace_b, &pd).expect("prove b");
                verify(&config, &air_b, &proof_b, &pd).expect("verify b");
                let proof_a = prove(&config, &air_a, &trace_a, &pd).expect("prove a");
                verify(&config, &air_a, &proof_a, &pd).expect("verify a");
                // A proof for claim A does not verify claim B.
                assert!(verify(&config, &air_b, &proof_a, &pd).is_err());
            }

            #[test]
            fn tiny_shapes_work() {
                // Single step (first == last on every row) and a single-row-per-step grid.
                for p in [ToyParams::new(256, 1, 0), ToyParams::new(1, 256, 0)] {
                    let (air, trace) = honest_instance::<Val>(p, 3);
                    assert!(accepted(&air, &air, &trace), "{p:?}");
                }
            }

            #[test]
            fn wrong_x_out_rejected_by_verifier() {
                let (air, trace) = honest();
                let mut x_out = air.x_out().to_vec();
                x_out[3] += Val::ONE;
                let lie = ToyAir::from_field(air.params, air.x_in().to_vec(), x_out);
                // Honest proof, verifier holds a different X_out.
                assert!(!accepted(&air, &lie, &trace));
                // Prover claims the wrong X_out for the honest trace.
                assert!(!accepted(&lie, &lie, &trace));
            }

            #[test]
            fn wrong_x_in_rejected_by_verifier() {
                let (air, trace) = honest();
                let mut x_in = air.x_in().to_vec();
                x_in[0] += Val::ONE;
                let lie = ToyAir::from_field(air.params, x_in, air.x_out().to_vec());
                assert!(!accepted(&air, &lie, &trace));
            }

            #[test]
            fn tampered_cell_value_rejected() {
                // Row-locally consistent, but the value read at (t=2, i=3) is not what step 1 wrote.
                let p = params();
                let (air, mut trace) = honest();
                let r = row(&p, 2, 3);
                *cell(&mut trace, r, col::Q) += Val::ONE;
                rederive_row(&p, &mut trace, r);
                assert!(!accepted(&air, &air, &trace));
            }

            #[test]
            fn tampered_neighbour_read_rejected() {
                let p = params();
                let (air, mut trace) = honest();
                let r = row(&p, 5, 7);
                *cell(&mut trace, r, col::LEFT) -= Val::ONE;
                rederive_row(&p, &mut trace, r);
                assert!(!accepted(&air, &air, &trace));
            }

            #[test]
            fn remainder_out_of_range_rejected() {
                // Last step, cell 5: rem += 2^s, q_next -= 1, and claim the matching X_out.
                // Every constraint holds; only the remainder range check fails.
                let p = params();
                let (air, mut trace) = honest();
                let r = row(&p, p.n_steps - 1, 5);
                let sixteen = Val::from_u64(1 << SHIFT);
                *cell(&mut trace, r, col::REM) += sixteen;
                *cell(&mut trace, r, col::Q_NEXT) -= Val::ONE;
                fill_range_multiplicities(&p, &mut trace);
                let mut x_out = air.x_out().to_vec();
                x_out[5] -= Val::ONE;
                let lie = ToyAir::from_field(p, air.x_in().to_vec(), x_out);
                assert!(!accepted(&lie, &lie, &trace));
            }

            #[test]
            fn fractional_output_rejected() {
                // Last step, cell 9: pick another in-range remainder; q_next becomes (L - rem)/2^s
                // in the field, which only the output range check catches.
                let p = params();
                let (air, mut trace) = honest();
                let r = row(&p, p.n_steps - 1, 9);
                let rem = get(&trace, r, col::REM).as_canonical_u64();
                let new_rem = Val::from_u64((rem + 1) % (1 << SHIFT));
                let lhs = get(&trace, r, col::Q_NEXT) * Val::from_u64(1 << SHIFT) + get(&trace, r, col::REM);
                let new_q = (lhs - new_rem) * Val::from_u64(1 << SHIFT).inverse();
                set(&mut trace, r, col::REM, new_rem);
                set(&mut trace, r, col::Q_NEXT, new_q);
                fill_range_multiplicities(&p, &mut trace);
                let mut x_out = air.x_out().to_vec();
                x_out[9] = new_q;
                let lie = ToyAir::from_field(p, air.x_in().to_vec(), x_out);
                assert!(!accepted(&lie, &lie, &trace));
            }

            #[test]
            fn wrong_multiplicity_rejected() {
                let p = params();
                for (t, i, delta) in [(1, 4, 1i64), (0, 0, -1), (p.n_steps - 1, 2, 1)] {
                    let (air, mut trace) = honest();
                    let r = row(&p, t, i);
                    *cell(&mut trace, r, col::MULT) += Val::from_i64(delta);
                    assert!(!accepted(&air, &air, &trace), "mult tamper at ({t}, {i})");
                }
            }

            #[test]
            fn wrong_range_multiplicity_rejected() {
                let (air, mut trace) = honest();
                *cell(&mut trace, 3, col::RS_MULT) += Val::ONE;
                assert!(!accepted(&air, &air, &trace));
            }

            #[test]
            fn witness_matches_reference() {
                let p = params();
                let x_in = sample_input(p.n_cells, 11);
                let states = run_reference(&x_in, p.n_steps);
                let trace = generate_trace::<Val>(&p, &states);
                for t in 0..p.n_steps {
                    for i in 0..p.n_cells {
                        assert_eq!(get(&trace, row(&p, t, i), col::Q_NEXT), Val::from_i64(states[t + 1][i]));
                    }
                }
            }
        }
    };
}

toy_tests!(goldilocks);
toy_tests!(koala_bear);
