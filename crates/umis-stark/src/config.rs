//! Shared STARK configurations: Poseidon2 Merkle MMCS + two-adic FRI PCS + duplex challenger,
//! for Goldilocks (degree-2 extension) and KoalaBear (degree-4 extension).
//!
//! Poseidon2 round constants come from the field crates' fixed `default_*_poseidon2_*`
//! constructors, so prover and verifier agree without sharing an RNG seed.

use p3_fri::FriParameters;

/// FRI parameter preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FriPreset {
    /// The production preset: **≥ 101 proven bits in the unique-decoding regime** (BCI+20 UD
    /// only, no 2025–26 list-decoding result) for every shape the umis verifiers accept
    /// (INV-PARAM-3, `docs/zk/air-spec.md` §6): blowup 4 (`log_blowup = 2`, so constraint
    /// degree ≤ 5), fold arity 8, 128 queries; grinding 16 bits at the query, out-of-domain,
    /// batch-combination and lookup sites, 12 bits per commit-phase fold round. The
    /// security tests in the task crates recompute the bound with p3-security.
    P100,
    /// Leaf preset for recursive aggregation (Plonky3-recursion verifies it in-circuit, which
    /// rejects grinding at the out-of-domain, batch and lookup sites): blowup 4, arity 8,
    /// 128 queries, 16 query-PoW bits, 16 commit-PoW bits per fold round, nothing else.
    /// Without batch grinding the batch-combination term caps the bound, so it reaches 101
    /// proven bits (unique decoding) only for single proofs of small shapes, such as a
    /// 256×256 life zone at the default layout (101.8): check the exact shape with
    /// [`crate::security::proven_udr_bits`].
    R100,
    /// Plonky3's benchmark preset: blowup 2 (`log_blowup = 1`), 100 queries, 16 query-PoW bits.
    /// About 66 proven / 102 conjectured bits: benchmarks only, not selectable by a genesis.
    Benchmark,
    /// Same shape as [`FriPreset::Benchmark`] but only a few queries and no grinding: fast,
    /// insecure (≈2 bits), for unit tests and genesis files marked `insecure_proofs`. Without
    /// grinding proofs are deterministic (native grinding is a parallel search that returns any
    /// valid nonce), so tests can pin proof bytes.
    Test,
}

impl FriPreset {
    /// FRI parameters for this preset over the given (extension-field) MMCS.
    pub const fn fri_params<M>(self, mmcs: M) -> FriParameters<M> {
        match self {
            Self::P100 => FriParameters {
                log_blowup: 2,
                log_final_poly_len: 0,
                max_log_arity: 3,
                num_queries: 128,
                batch_proof_of_work_bits: 16,
                commit_proof_of_work_bits: 12,
                query_proof_of_work_bits: 16,
                mmcs,
            },
            Self::R100 => FriParameters {
                log_blowup: 2,
                log_final_poly_len: 0,
                max_log_arity: 3,
                num_queries: 128,
                batch_proof_of_work_bits: 0,
                commit_proof_of_work_bits: 16,
                query_proof_of_work_bits: 16,
                mmcs,
            },
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

    /// `log2` of the FRI blowup (the constraint degree must be ≤ `2^log_blowup + 1`).
    pub const fn log_blowup(self) -> usize {
        match self {
            Self::P100 | Self::R100 => 2,
            Self::Benchmark | Self::Test => 1,
        }
    }

    /// Grinding bits before the out-of-domain point (`StarkConfig::with_ood_proof_of_work_bits`).
    pub const fn ood_pow_bits(self) -> usize {
        match self {
            Self::P100 => 16,
            Self::R100 | Self::Benchmark | Self::Test => 0,
        }
    }

    /// Grinding bits before the LogUp challenges (`StarkConfig::with_lookup_proof_of_work_bits`).
    pub const fn lookup_pow_bits(self) -> usize {
        match self {
            Self::P100 => 16,
            Self::R100 | Self::Benchmark | Self::Test => 0,
        }
    }

    /// Whether proofs under this preset are secure (≥ 100 proven bits). Only these may be
    /// selected by a genesis that does not opt into `insecure_proofs` (INV-PARAM-1).
    pub const fn is_secure(self) -> bool {
        matches!(self, Self::P100)
    }

    /// The grinding this preset puts at the non-FRI sites, as p3-security counts it.
    pub const fn grinding_sites(self) -> p3_security::GrindingSites {
        p3_security::GrindingSites {
            out_of_domain: self.ood_pow_bits(),
            batch_combination: match self {
                Self::P100 => 16,
                Self::R100 | Self::Benchmark | Self::Test => 0,
            },
            lookup_challenge: self.lookup_pow_bits(),
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
            .with_ood_proof_of_work_bits(preset.ood_pow_bits())
            .with_lookup_proof_of_work_bits(preset.lookup_pow_bits())
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
            .with_ood_proof_of_work_bits(preset.ood_pow_bits())
            .with_lookup_proof_of_work_bits(preset.lookup_pow_bits())
    }
}

/// KoalaBear with a Keccak Merkle MMCS: the same field, FRI and presets as [`koala_bear`], but
/// the Merkle trees hash with Keccak-f[1600]. On x86 with AVX2, `p3-keccak` hashes four rows per
/// permutation and commits about 3x (large traces) to 13x (small ones) faster than width-16
/// Poseidon2 (`examples/mmcs_bench.rs`). The transcript stays the Poseidon2 duplex challenger
/// (its proof-of-work grinding is a cheap parallel Poseidon2 search; a Keccak transcript's
/// grinding cost ~2 ms per proof); Merkle roots enter it as 22-bit limbs ([`HybridChallenger`]).
/// Not recursion-friendly: for leaf proofs that are verified natively.
pub mod koala_bear_keccak {
    use p3_challenger::{CanObserve, CanSample, CanSampleBits, DuplexChallenger, FieldChallenger, GrindingChallenger};
    use p3_commit::ExtensionMmcs;
    use p3_dft::Radix2DitParallel;
    use p3_field::PrimeCharacteristicRing;
    use p3_field::extension::BinomialExtensionField;
    use p3_fri::TwoAdicFriPcs;
    use p3_keccak::{KeccakF, VECTOR_LEN};
    use p3_koala_bear::{KoalaBear, default_koalabear_poseidon2_16};
    use p3_merkle_tree::MerkleTreeMmcs;
    use p3_symmetric::{CompressionFunctionFromHasher, MerkleCap, PaddingFreeSponge, SerializingHasher};
    use p3_uni_stark::StarkConfig;

    use super::FriPreset;
    pub use super::koala_bear::Perm;

    pub type Val = KoalaBear;
    pub type Challenge = BinomialExtensionField<Val, 4>;
    /// Keccak-f[1600] sponge over u64 words: rate 17 words, 4-word (256-bit) digest.
    pub type Sponge = PaddingFreeSponge<KeccakF, 25, 17, 4>;
    pub type Hash = SerializingHasher<Sponge>;
    pub type Compress = CompressionFunctionFromHasher<Sponge, 2, 4>;
    pub type ValMmcs = MerkleTreeMmcs<[Val; VECTOR_LEN], [u64; VECTOR_LEN], Hash, Compress, 2, 4>;
    pub type ChallengeMmcs = ExtensionMmcs<Val, Challenge, ValMmcs>;
    pub type Challenger = HybridChallenger;
    pub type Dft = Radix2DitParallel<Val>;
    pub type Pcs = TwoAdicFriPcs<Val, Dft, ValMmcs, ChallengeMmcs>;
    pub type Config = StarkConfig<Pcs, Challenge, Challenger>;

    /// The Poseidon2 duplex challenger, absorbing Keccak Merkle roots: every u64 word as three
    /// limbs of 22, 22 and 20 bits (each below p, so the encoding is injective).
    #[derive(Clone, Debug)]
    pub struct HybridChallenger(pub DuplexChallenger<Val, Perm, 16, 8>);

    impl HybridChallenger {
        pub fn new(perm: Perm) -> Self {
            Self(DuplexChallenger::new(perm))
        }

        /// The state `DuplexChallenger::grind` permutes per candidate and the witness's
        /// rate slot: buffered inputs, the witness slot and zeros in the rate, the length
        /// tag in the first capacity element (Plonky3 0.8 duplex absorption).
        pub fn grind_base(&self) -> ([Val; 16], usize) {
            let c = &self.0;
            let widx = c.input_buffer.len();
            let base = core::array::from_fn(|i| {
                if i < widx {
                    c.input_buffer[i]
                } else if i < 8 {
                    Val::ZERO
                } else if i == 8 {
                    c.sponge_state[8] + Val::from_u8((widx + 1) as u8)
                } else {
                    c.sponge_state[i]
                }
            });
            (base, widx)
        }

        fn observe_word(&mut self, w: u64) {
            for limb in [w & 0x3f_ffff, (w >> 22) & 0x3f_ffff, w >> 44] {
                self.0.observe(Val::from_u32(limb as u32));
            }
        }
    }

    impl CanObserve<Val> for HybridChallenger {
        fn observe(&mut self, value: Val) {
            self.0.observe(value);
        }
    }

    impl<const N: usize> CanObserve<&MerkleCap<Val, [u64; N]>> for HybridChallenger {
        fn observe(&mut self, cap: &MerkleCap<Val, [u64; N]>) {
            for digest in cap.roots() {
                for w in digest {
                    self.observe_word(*w);
                }
            }
        }
    }

    impl<const N: usize> CanObserve<MerkleCap<Val, [u64; N]>> for HybridChallenger {
        fn observe(&mut self, cap: MerkleCap<Val, [u64; N]>) {
            self.observe(&cap);
        }
    }

    impl<T> CanSample<T> for HybridChallenger
    where
        DuplexChallenger<Val, Perm, 16, 8>: CanSample<T>,
    {
        fn sample(&mut self) -> T {
            self.0.sample()
        }
    }

    impl CanSampleBits<usize> for HybridChallenger {
        fn sample_bits(&mut self, bits: usize) -> usize {
            self.0.sample_bits(bits)
        }
    }

    impl FieldChallenger<Val> for HybridChallenger {}

    /// A proof-of-work search offloaded from the CPU (a GPU): given the sponge state
    /// before the witness's permutation (`DuplexChallenger::grind`'s base state), the
    /// rate slot the witness goes into and the difficulty, a candidate witness.
    pub type Grinder = fn(&[Val; 16], usize, usize) -> Option<Val>;

    static GRINDER: core::sync::atomic::AtomicPtr<()> = core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

    /// Installs a [`Grinder`] for [`HybridChallenger::grind`] (prover side only: every
    /// candidate is checked with `check_witness` before it is used, and the CPU search
    /// runs if it fails, so the transcript and the proofs are those of the CPU grind).
    pub fn set_grinder(g: Grinder) {
        GRINDER.store(g as *mut (), core::sync::atomic::Ordering::Release);
    }

    fn grinder() -> Option<Grinder> {
        let p = GRINDER.load(core::sync::atomic::Ordering::Acquire);
        // SAFETY: only `set_grinder` stores a non-null value, and it stores a `Grinder`.
        (!p.is_null()).then(|| unsafe { core::mem::transmute::<*mut (), Grinder>(p) })
    }

    impl GrindingChallenger for HybridChallenger {
        type Witness = Val;

        fn grind(&mut self, bits: usize) -> Val {
            if bits > 0
                && let Some(g) = grinder()
            {
                let (base, widx) = self.grind_base();
                if let Some(w) = g(&base, widx, bits) {
                    let mut probe = self.0.clone();
                    if probe.check_witness(bits, w) {
                        self.0 = probe;
                        return w;
                    }
                }
            }
            self.0.grind(bits)
        }
    }

    /// The FRI parameters of [`config`] (for provers that drive FRI themselves).
    pub fn fri_params(preset: FriPreset) -> p3_fri::FriParameters<ChallengeMmcs> {
        let sponge = Sponge::new(KeccakF);
        preset.fri_params(ChallengeMmcs::new(ValMmcs::new(Hash::new(sponge), Compress::new(sponge), 0)))
    }

    /// Build the KoalaBear/Keccak STARK config.
    pub fn config(preset: FriPreset) -> Config {
        let sponge = Sponge::new(KeccakF);
        let val_mmcs = ValMmcs::new(Hash::new(sponge), Compress::new(sponge), 0);
        let challenge_mmcs = ChallengeMmcs::new(val_mmcs.clone());
        let pcs = Pcs::new(Dft::default(), val_mmcs, preset.fri_params(challenge_mmcs));
        Config::new(pcs, HybridChallenger::new(default_koalabear_poseidon2_16()))
            .with_ood_proof_of_work_bits(preset.ood_pow_bits())
            .with_lookup_proof_of_work_bits(preset.lookup_pow_bits())
    }
}
