//! Shared STARK configurations: Poseidon2 Merkle MMCS + two-adic FRI PCS + duplex challenger,
//! for Goldilocks (degree-2 extension) and KoalaBear (degree-4 extension).
//!
//! Poseidon2 round constants come from the field crates' fixed `default_*_poseidon2_*`
//! constructors, so prover and verifier agree without sharing an RNG seed.

use p3_fri::FriParameters;

/// FRI parameter preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FriPreset {
    /// Plonky3's benchmark preset: blowup 2 (`log_blowup = 1`), 100 queries, 16 query-PoW bits
    /// (~116 conjectured bits). `log_blowup = 1` caps the AIR constraint degree at 3.
    Benchmark,
    /// Same shape as [`FriPreset::Benchmark`] but only a few queries and no grinding: fast,
    /// insecure, for unit tests. Without grinding proofs are deterministic (native grinding
    /// is a parallel search that returns any valid nonce), so tests can pin proof bytes.
    Test,
}

impl FriPreset {
    /// FRI parameters for this preset over the given (extension-field) MMCS.
    pub const fn fri_params<M>(self, mmcs: M) -> FriParameters<M> {
        match self {
            Self::Benchmark => FriParameters::new_benchmark(mmcs),
            Self::Test => FriParameters {
                log_blowup: 1,
                log_final_poly_len: 0,
                max_log_arity: 1,
                num_queries: 4,
                batch_proof_of_work_bits: 0,
                commit_proof_of_work_bits: 0,
                query_proof_of_work_bits: 0,
                mmcs,
            },
        }
    }
}

/// Goldilocks (p = 2^64 - 2^32 + 1), challenge field GF(p^2), width-8 Poseidon2.
pub mod goldilocks {
    use p3_challenger::DuplexChallenger;
    use p3_commit::ExtensionMmcs;
    use p3_dft::Radix2DitParallel;
    use p3_field::Field;
    use p3_field::extension::BinomialExtensionField;
    use p3_fri::TwoAdicFriPcs;
    use p3_goldilocks::{Goldilocks, Poseidon2Goldilocks, default_goldilocks_poseidon2_8};
    use p3_merkle_tree::MerkleTreeMmcs;
    use p3_symmetric::{PaddingFreeSponge, TruncatedPermutation};
    use p3_uni_stark::StarkConfig;

    use super::FriPreset;

    pub type Val = Goldilocks;
    pub type Challenge = BinomialExtensionField<Val, 2>;
    pub type Perm = Poseidon2Goldilocks<8>;
    /// Sponge: width 8, rate 4, 4-element (256-bit) digest.
    pub type Hash = PaddingFreeSponge<Perm, 8, 4, 4>;
    /// 2-to-1 compression of two 4-element digests.
    pub type Compress = TruncatedPermutation<Perm, 2, 4, 8>;
    pub type ValMmcs = MerkleTreeMmcs<<Val as Field>::Packing, <Val as Field>::Packing, Hash, Compress, 2, 4>;
    pub type ChallengeMmcs = ExtensionMmcs<Val, Challenge, ValMmcs>;
    pub type Challenger = DuplexChallenger<Val, Perm, 8, 4>;
    pub type Dft = Radix2DitParallel<Val>;
    pub type Pcs = TwoAdicFriPcs<Val, Dft, ValMmcs, ChallengeMmcs>;
    pub type Config = StarkConfig<Pcs, Challenge, Challenger>;

    /// Build the Goldilocks STARK config.
    pub fn config(preset: FriPreset) -> Config {
        let perm = default_goldilocks_poseidon2_8();
        let val_mmcs = ValMmcs::new(Hash::new(perm.clone()), Compress::new(perm.clone()), 0);
        let challenge_mmcs = ChallengeMmcs::new(val_mmcs.clone());
        let pcs = Pcs::new(Dft::default(), val_mmcs, preset.fri_params(challenge_mmcs));
        Config::new(pcs, Challenger::new(perm))
    }
}

/// KoalaBear (p = 2^31 - 2^24 + 1), challenge field GF(p^4), width-16 Poseidon2.
pub mod koala_bear {
    use p3_challenger::DuplexChallenger;
    use p3_commit::ExtensionMmcs;
    use p3_dft::Radix2DitParallel;
    use p3_field::Field;
    use p3_field::extension::BinomialExtensionField;
    use p3_fri::TwoAdicFriPcs;
    use p3_koala_bear::{KoalaBear, Poseidon2KoalaBear, default_koalabear_poseidon2_16};
    use p3_merkle_tree::MerkleTreeMmcs;
    use p3_symmetric::{PaddingFreeSponge, TruncatedPermutation};
    use p3_uni_stark::StarkConfig;

    use super::FriPreset;

    pub type Val = KoalaBear;
    pub type Challenge = BinomialExtensionField<Val, 4>;
    pub type Perm = Poseidon2KoalaBear<16>;
    /// Sponge: width 16, rate 8, 8-element (~248-bit) digest.
    pub type Hash = PaddingFreeSponge<Perm, 16, 8, 8>;
    /// 2-to-1 compression of two 8-element digests.
    pub type Compress = TruncatedPermutation<Perm, 2, 8, 16>;
    pub type ValMmcs = MerkleTreeMmcs<<Val as Field>::Packing, <Val as Field>::Packing, Hash, Compress, 2, 8>;
    pub type ChallengeMmcs = ExtensionMmcs<Val, Challenge, ValMmcs>;
    pub type Challenger = DuplexChallenger<Val, Perm, 16, 8>;
    pub type Dft = Radix2DitParallel<Val>;
    pub type Pcs = TwoAdicFriPcs<Val, Dft, ValMmcs, ChallengeMmcs>;
    pub type Config = StarkConfig<Pcs, Challenge, Challenger>;

    /// Build the KoalaBear STARK config.
    pub fn config(preset: FriPreset) -> Config {
        let perm = default_koalabear_poseidon2_16();
        let val_mmcs = ValMmcs::new(Hash::new(perm.clone()), Compress::new(perm.clone()), 0);
        let challenge_mmcs = ChallengeMmcs::new(val_mmcs.clone());
        let pcs = Pcs::new(Dft::default(), val_mmcs, preset.fri_params(challenge_mmcs));
        Config::new(pcs, Challenger::new(perm))
    }
}
