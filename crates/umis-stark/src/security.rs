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

impl ProofShape {
    /// The same proof modelled at each of `log_heights` (a batch proof's instance heights).
    /// p3-security takes one trace length, and its terms pull both ways: the unique-decoding
    /// agreement uses `ρ⁺ = (k + 2)/n` (the two out-of-domain points), worst at the **shortest**
    /// instance, the DEEP and list terms are worst at the tallest. A batch proof must clear
    /// every one ([`worst_bits`]).
    pub fn at_heights(&self, log_heights: impl IntoIterator<Item = usize>) -> alloc::vec::Vec<ProofShape> {
        let mut hs: alloc::vec::Vec<usize> = log_heights.into_iter().collect();
        hs.sort_unstable_by(|a, b| b.cmp(a));
        hs.dedup();
        hs.into_iter().map(|log_height| ProofShape { log_height, ..*self }).collect()
    }
}

/// The proven bits of a proof modelled by several shapes ([`ProofShape::at_heights`]): the
/// smallest figure.
pub fn worst_bits(shapes: &[ProofShape], bits: impl Fn(&ProofShape) -> f64) -> f64 {
    shapes.iter().map(bits).fold(f64::INFINITY, f64::min)
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
    if !fits(f.log_blowup, shape) {
        return 0.0;
    }
    report(&fri, preset.grinding_sites(), shape, 123).udr.security_bits()
}

/// Whether the calculator's model covers `shape`: a constraint degree above `blowup + 1` is
/// outside it (p3-security: "the prover cannot commit a quotient"), so its figure would be
/// meaningless; such shapes are reported as 0 bits.
pub fn fits(log_blowup: usize, shape: &ProofShape) -> bool {
    shape.max_degree <= (1 << log_blowup) + 1
}

/// The challenge field a proof samples from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// KoalaBear⁴ (2^123.95, fed as 123 bits).
    Ef4,
    /// KoalaBear⁵, `QuinticTrinomialExtensionField` (2^154.94, fed as 154 bits).
    Ef5,
}

/// Which proximity bound a figure rests on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    /// Unique decoding only (BCI+20 UD; the conservative choice).
    UniqueDecoding,
    /// The better of unique decoding and the Johnson (list-decoding) regime; the latter rests
    /// on the 2026 DKT26 line count as p3-security implements it.
    Johnson,
}

/// FRI parameters without grinding at the OOD, batch or lookup sites (recursion-compatible).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecursiveFri {
    pub log_blowup: usize,
    pub max_log_arity: usize,
    pub num_queries: usize,
    pub commit_pow_bits: usize,
    pub query_pow_bits: usize,
}

/// Proven bits of `shape` under recursion-compatible FRI parameters.
pub fn proven_bits(fri: &RecursiveFri, field: Field, regime: Regime, shape: &ProofShape) -> f64 {
    let regime_fri = FriRegime {
        log_blowup: fri.log_blowup,
        num_queries: fri.num_queries,
        log_final_poly_len: 0,
        max_log_arity: fri.max_log_arity,
        commit_pow_bits: fri.commit_pow_bits,
        query_pow_bits: fri.query_pow_bits,
    };
    let none = p3_security::GrindingSites { out_of_domain: 0, batch_combination: 0, lookup_challenge: 0 };
    let bits = match field {
        Field::Ef4 => 123,
        Field::Ef5 => 154,
    };
    if !fits(fri.log_blowup, shape) {
        return 0.0;
    }
    let r = report(&regime_fri, none, shape, bits);
    match regime {
        Regime::UniqueDecoding => r.udr.security_bits(),
        Regime::Johnson => r.security_bits(),
    }
}

fn report(
    fri: &FriRegime,
    grinding: p3_security::GrindingSites,
    shape: &ProofShape,
    modulus_bits: usize,
) -> p3_security::SecurityReport {
    let chunks = (shape.max_degree.max(2) - 1).next_power_of_two();
    let air = StarkAirParams {
        num_constraints: shape.num_constraints,
        max_constraint_degree: shape.max_degree,
        num_quotient_chunks: chunks,
        max_combo: 2,
    };
    let inst = InstanceShape {
        log_trace_length: shape.log_height,
        modulus_bits,
        collision_resistance: 124,
        num_batched_functions: shape.batched_openings,
    };
    let extras: alloc::vec::Vec<_> = shape
        .logup
        .and_then(|(n, w)| {
            logup::security_term(&LogUpAir { num_interactions: n, max_message_width: w }, &inst, &grinding)
        })
        .into_iter()
        .collect();
    proven_security_report(fri, &air, &inst, &extras, &grinding)
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
