//! Proven security of umis proofs (INV-PARAM-3), computed with p3-security for an exact proof
//! shape, never as `queries × log_blowup + PoW` (`docs/zk/notes/fri-soundness-foundations.md`).
//!
//! The figure is the **unique-decoding** bound: it rests only on BCI+20's UD proximity gaps
//! and the ethSTARK/Haböck composition, not on the 2025–26 list-decoding line counts. The
//! challenge field is fed as 123 bits (|EF4| = 2^123.95, rounded down) and the commitment
//! collision resistance as 124 bits (8-element Poseidon2 transcript capacity; Keccak-256
//! Merkle trees give 128).

use p3_security::fri::FriRegime;
use p3_security::logup::{self, LogUpAir};
use p3_security::stark::proven_security_report;
use p3_security::{InstanceShape, StarkAirParams};

use crate::config::FriPreset;

/// The release criterion: 100 bits plus ethSTARK Cor. 3's union-bound bit.
pub const TARGET_BITS: f64 = 101.0;

/// What a verifier checks in one proof, summed over a batch's instances.
#[derive(Clone, Copy, Debug)]
pub struct ProofShape {
    /// `log2` of the largest trace height the verifier accepts.
    pub log_height: usize,
    /// Constraints, summed over instances (they share one composition challenge).
    pub num_constraints: usize,
    /// Maximum constraint degree.
    pub max_degree: usize,
    /// Codewords batched into the low-degree test, summed over instances
    /// (`p3_batch_stark::num_batched_openings`).
    pub batched_openings: usize,
    /// LogUp interactions per row and widest message (`None`: no lookups).
    pub logup: Option<(usize, usize)>,
}

/// Proven bits (unique-decoding regime) of `shape` under `preset`.
pub fn proven_udr_bits(preset: FriPreset, shape: &ProofShape) -> f64 {
    let f = preset.fri_params(());
    let fri = FriRegime {
        log_blowup: f.log_blowup,
        num_queries: f.num_queries,
        log_final_poly_len: f.log_final_poly_len,
        max_log_arity: f.max_log_arity,
        commit_pow_bits: f.commit_proof_of_work_bits,
        query_pow_bits: f.query_proof_of_work_bits,
    };
    let chunks = (shape.max_degree.max(2) - 1).next_power_of_two();
    let air = StarkAirParams {
        num_constraints: shape.num_constraints,
        max_constraint_degree: shape.max_degree,
        num_quotient_chunks: chunks,
        max_combo: 2,
    };
    let inst = InstanceShape {
        log_trace_length: shape.log_height,
        modulus_bits: 123,
        collision_resistance: 124,
        num_batched_functions: shape.batched_openings,
    };
    let grinding = preset.grinding_sites();
    let extras: alloc::vec::Vec<_> = shape
        .logup
        .and_then(|(n, w)| {
            logup::security_term(&LogUpAir { num_interactions: n, max_message_width: w }, &inst, &grinding)
        })
        .into_iter()
        .collect();
    proven_security_report(&fri, &air, &inst, &extras, &grinding).udr.security_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The calculator reproduces the documented figures: `Benchmark` at the old water
    /// shape is 57.5 bits UDR (`fri-soundness-foundations.md` §3.3).
    #[test]
    fn benchmark_preset_is_far_below_target() {
        let water = ProofShape {
            log_height: 14,
            num_constraints: 56,
            max_degree: 3,
            batched_openings: 198,
            logup: Some((23, 5)),
        };
        let b = proven_udr_bits(FriPreset::Benchmark, &water);
        assert!((55.0..60.0).contains(&b), "{b}");
        assert!(proven_udr_bits(FriPreset::P100, &water) >= TARGET_BITS);
        assert!(proven_udr_bits(FriPreset::Test, &water) < 10.0);
    }
}
