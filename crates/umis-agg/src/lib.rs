//! Recursive aggregation of umis leaf proofs with Plonky3-recursion (native only).
//!
//! Every proof here samples its challenges from KoalaBear⁵ (`QuinticTrinomialExtensionField`,
//! about 2^155): Plonky3-recursion's in-circuit verifier cannot check proof-of-work at the
//! out-of-domain, batch or lookup steps, and over KoalaBear⁴ the batch-combination term then
//! caps the recursive proofs at about 93 bits (`docs/zk/aggregation.md`). The circuit field is
//! the leaves' challenge field, so leaves are KoalaBear⁵ proofs too.
//!
//! - [`config`]: upstream's built-in `KoalaBearD5Poseidon2BinaryConfig` at [`AggParams`].
//! - [`leaf`]: one life2d slot proved as a uni-STARK.
//! - [`Layer1`]: a trusted prepared owner whose recursive proofs verify one leaf in-circuit;
//!   [`Aggregator`]: one whose proofs verify two recursive proofs (2-to-1). Their verifiers
//!   check every proof against its expected statement (the leaf's public values).
//!
//! Plonky3-recursion is unaudited (its README says so); this crate is pinned to one commit.

use p3_circuit_prover::BatchStarkProof;
use p3_circuit_prover::batch_stark_prover::CircuitVerifier;
use p3_recursion::builtin_config::{
    FriConfigV1, KoalaBearD5Poseidon2BinaryConfig, SuiteIdV1, koala_bear_d5_poseidon2_binary,
};
use p3_recursion::verifier::VerifierLimits;
use p3_recursion::{
    BatchOnly, FriRecursionBackend, FriRecursionBackendD5, Poseidon2Config, ProveNextLayerParams, RecursionOutput,
    TrustedPreparedAggregation, TrustedPreparedInput, TrustedPreparedLayer, TrustedPreparedSource,
};
use p3_uni_stark::{Proof, prove, verify};
use task_life2d::Life2dParams;
use task_life2d::air::digest::{DIGEST, LifeDigestAir, N_PUBLIC, extend_trace, public_values};
use task_life2d::air::{LifeAir, Shape, claim_data, generate_trace};
use task_life2d::board::Board;
use umis_core::Hash;
use umis_stark::security::{Field, ProofShape, RecursiveFri, Regime, proven_bits};

/// The STARK configuration of every aggregation proof (leaves and recursive layers).
pub type AggConfig = KoalaBearD5Poseidon2BinaryConfig;
type Val = p3_koala_bear::KoalaBear;

/// The recursion backend for KoalaBear⁵ (base-field Poseidon2 challenger).
pub type Backend = FriRecursionBackendD5<16, 8, Poseidon2Config>;

/// FRI parameters of the aggregation configuration and the bound they are claimed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AggParams {
    pub fri: RecursiveFri,
    pub regime: Regime,
}

impl AggParams {
    /// ≥ 101 proven bits in the Johnson regime (DKT26 line count) for leaves and recursive
    /// proofs: blowup 8, arity 8, 58 queries, 16-bit query and commit grinding.
    pub const J101: Self = Self {
        fri: RecursiveFri { log_blowup: 3, max_log_arity: 3, num_queries: 58, commit_pow_bits: 16, query_pow_bits: 16 },
        regime: Regime::Johnson,
    };
    /// ≥ 101 proven bits in the unique-decoding regime only: blowup 4, arity 8, 128 queries.
    pub const U101: Self = Self {
        fri: RecursiveFri {
            log_blowup: 2,
            max_log_arity: 3,
            num_queries: 128,
            commit_pow_bits: 16,
            query_pow_bits: 16,
        },
        regime: Regime::UniqueDecoding,
    };
    /// Fast and insecure (tests).
    pub const TEST: Self = Self {
        fri: RecursiveFri { log_blowup: 1, max_log_arity: 1, num_queries: 2, commit_pow_bits: 0, query_pow_bits: 0 },
        regime: Regime::UniqueDecoding,
    };

    /// Proven bits of a proof of `shape` under these parameters.
    pub fn bits(&self, shape: &ProofShape) -> f64 {
        proven_bits(&self.fri, Field::Ef5, self.regime, shape)
    }
}

/// The built-in KoalaBear⁵ Poseidon2 configuration at `p`.
pub fn config(p: &AggParams) -> AggConfig {
    let f = &p.fri;
    let descriptor = FriConfigV1::new(
        SuiteIdV1::KoalaBearD5Poseidon2BinaryFri,
        f.log_blowup as u32,
        0,
        f.max_log_arity as u32,
        f.num_queries as u32,
        f.commit_pow_bits as u32,
        f.query_pow_bits as u32,
        0,
        0,
        0,
        0,
    );
    koala_bear_d5_poseidon2_binary(&descriptor, &VerifierLimits::default()).expect("aggregation FRI parameters")
}

/// The recursion backend.
pub fn backend() -> Backend {
    FriRecursionBackend::<16, 8, _>::new_d5(Poseidon2Config::KOALA_BEAR_D1_W16)
        .with_extra_poseidon2_table(Poseidon2Config::KOALA_BEAR_D1_W32)
}

/// One life2d slot proved for aggregation: its AIR (with the claim's periodic data), proof
/// and public values (the claim digest, its statement).
pub struct Leaf {
    pub air: LifeAir,
    pub proof: Proof<AggConfig>,
    pub public: Vec<Val>,
}

impl Leaf {
    /// The leaf's shape for the security calculator.
    pub fn shape(&self) -> ProofShape {
        let s = self.air.shape;
        ProofShape {
            log_height: s.height.trailing_zeros() as usize,
            num_constraints: s.num_constraints(),
            max_degree: 3,
            batched_openings: 2 * s.width() + 2 * 5,
            logup: None,
        }
    }
}

/// Proves one advancing slot of a zone with layout `p` on the extended board `ext`.
pub fn leaf(cfg: &AggConfig, p: &Life2dParams, slot: u64, ext: &Board, absent: &[bool; 8]) -> Leaf {
    let s = Shape::of(p);
    let (trace, x_out) = generate_trace(&s, ext, absent);
    let (per, public) = claim_data(&s, &Hash::ZERO, slot, ext, absent, &x_out);
    let air = LifeAir::new(s, per);
    let proof = prove(cfg, &air, trace, &public).expect("leaf proves");
    verify(cfg, &air, &proof, &public).expect("leaf verifies natively");
    Leaf { air, proof, public }
}

/// One life2d slot proved with the claim-independent [`LifeDigestAir`] (ROADMAP A2): its
/// statement is the Poseidon2 digest of the claim and the `absent` flags, and one AIR (so one
/// layer-1 circuit) serves every slot of a zone shape.
pub struct DigestLeaf {
    pub air: LifeDigestAir,
    pub proof: Proof<AggConfig>,
    pub public: Vec<Val>,
}

impl DigestLeaf {
    /// The leaf's shape for the security calculator.
    pub fn shape(&self) -> ProofShape {
        let d = self.air.shape;
        ProofShape {
            log_height: d.life.height.trailing_zeros() as usize,
            num_constraints: d.num_constraints(),
            max_degree: 3,
            batched_openings: 2 * d.width() + 2 * 5,
            logup: None,
        }
    }
}

/// Proves one advancing slot of a zone with layout `p` on the extended board `ext` with the
/// digest AIR.
pub fn digest_leaf(cfg: &AggConfig, p: &Life2dParams, ext: &Board, absent: &[bool; 8]) -> DigestLeaf {
    let air = LifeDigestAir::new(Shape::of(p));
    let (life, x_out) = generate_trace(&air.shape.life, ext, absent);
    let trace = extend_trace(&air.shape, life);
    let public = public_values(&air.shape, ext, absent, &x_out);
    let proof = prove(cfg, &air, trace, &public).expect("digest leaf proves");
    verify(cfg, &air, &proof, &public).expect("digest leaf verifies natively");
    DigestLeaf { air, proof, public }
}

/// A layer-1 owner built once per zone shape (from any representative leaf) that proves the
/// recursive proof of every digest leaf of that shape.
pub struct Layer1Owner<'a> {
    owner: TrustedPreparedLayer<'a, AggConfig, AggConfig, LifeDigestAir, Backend, 5>,
    pub build: std::time::Duration,
}

impl<'a> Layer1Owner<'a> {
    pub fn new(p: &AggParams, representative: &'a DigestLeaf) -> Result<Self, String> {
        let t = std::time::Instant::now();
        let owner = TrustedPreparedLayer::new(
            TrustedPreparedSource::UniStark {
                config: config(p),
                air: &representative.air,
                preprocessed_commit: None,
                proof: &representative.proof,
                public_inputs: &representative.public,
            },
            config(p),
            backend(),
            params(),
        )
        .map_err(|e| format!("{e:?}"))?;
        Ok(Self { owner, build: t.elapsed() })
    }

    /// The recursive proof of `leaf` (its statement: the leaf's public values).
    pub fn prove(&self, leaf: &DigestLeaf) -> Result<Recursive, String> {
        let t = std::time::Instant::now();
        let out = self
            .owner
            .prove(TrustedPreparedInput::UniStark { proof: &leaf.proof, public_inputs: &leaf.public })
            .map_err(|e| format!("{e:?}"))?;
        let timing = Timing { build: std::time::Duration::ZERO, prove: t.elapsed() };
        Ok(Recursive { out, statement: leaf.public.clone(), verifier: self.owner.verifier(), timing })
    }
}

/// A 2-to-1 owner built once (from representative children) that aggregates any two
/// recursive proofs with the same verifiers as those children.
pub struct AggOwner {
    owner: TrustedPreparedAggregation<'static, 'static, AggConfig, AggConfig, BatchOnly, BatchOnly, Backend, 5>,
    pub build: std::time::Duration,
}

impl AggOwner {
    pub fn new(p: &AggParams, left: &Recursive, right: &Recursive) -> Result<Self, String> {
        let t = std::time::Instant::now();
        let owner = TrustedPreparedAggregation::new(
            TrustedPreparedSource::BatchStark {
                verifier: left.verifier.clone(),
                proof: &left.out.0,
                statement: &left.statement,
            },
            TrustedPreparedSource::BatchStark {
                verifier: right.verifier.clone(),
                proof: &right.out.0,
                statement: &right.statement,
            },
            config(p),
            backend(),
            params(),
        )
        .map_err(|e| format!("{e:?}"))?;
        Ok(Self { owner, build: t.elapsed() })
    }

    /// The 2-to-1 proof of `left` and `right` (statement: left's, then right's).
    pub fn prove(&self, left: &Recursive, right: &Recursive) -> Result<Recursive, String> {
        let t = std::time::Instant::now();
        let out = self
            .owner
            .prove(
                TrustedPreparedInput::BatchStark { proof: &left.out.0, statement: &left.statement },
                TrustedPreparedInput::BatchStark { proof: &right.out.0, statement: &right.statement },
            )
            .map_err(|e| format!("{e:?}"))?;
        let statement = [left.statement.as_slice(), right.statement.as_slice()].concat();
        let timing = Timing { build: std::time::Duration::ZERO, prove: t.elapsed() };
        Ok(Recursive { out, statement, verifier: self.owner.verifier(), timing })
    }
}

/// The board digests that a run of consecutive digest-leaf statements (`N_PUBLIC` elements
/// each, as an aggregate exports them, left to right) starts and ends with, if every leaf's
/// `d_next` equals the next leaf's `d_prev` (INV-LIFE-DIG-5). The checker compares them with
/// the zone commitment's Poseidon2 state roots; halos and absent flags are checked against
/// the neighbours separately.
pub fn chain(statement: &[Val]) -> Result<([Val; DIGEST], [Val; DIGEST]), String> {
    if statement.is_empty() || statement.len() % N_PUBLIC != 0 {
        return Err(format!("statement length {} is not a positive multiple of {N_PUBLIC}", statement.len()));
    }
    let leaves: Vec<&[Val]> = statement.chunks(N_PUBLIC).collect();
    for (i, w) in leaves.windows(2).enumerate() {
        if w[0][2 * DIGEST..3 * DIGEST] != w[1][..DIGEST] {
            return Err(format!("leaf {} does not continue leaf {i}", i + 1));
        }
    }
    let first = leaves[0][..DIGEST].try_into().expect("digest");
    let last = leaves[leaves.len() - 1][2 * DIGEST..3 * DIGEST].try_into().expect("digest");
    Ok((first, last))
}

/// A recursive proof and the statement it carries.
pub struct Recursive {
    pub out: RecursionOutput<AggConfig>,
    pub statement: Vec<Val>,
    pub verifier: CircuitVerifier<AggConfig>,
    /// Time spent building the prepared owner (verifier circuit) and proving with it.
    pub timing: Timing,
}

/// Where a recursive step's time went.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    pub build: std::time::Duration,
    pub prove: std::time::Duration,
}

impl Recursive {
    /// Verifies the proof against its retained verifier and statement.
    pub fn verify(&self) -> Result<(), String> {
        self.verifier.verify(&self.out.0, &self.statement).map_err(|e| format!("{e:?}"))
    }
}

fn params() -> ProveNextLayerParams {
    ProveNextLayerParams::default()
}

/// Layer 1: a recursive proof that verifies `leaf` in-circuit, with the leaf's public values
/// as its statement. The life AIR's per-claim periodic data are circuit constants, so each
/// claim needs its own owner (INV-LIFE-FS-2).
pub fn layer1(p: &AggParams, leaf: &Leaf) -> Result<Recursive, String> {
    let t = std::time::Instant::now();
    let owner = TrustedPreparedLayer::<_, _, _, _, 5>::new(
        TrustedPreparedSource::UniStark {
            config: config(p),
            air: &leaf.air,
            preprocessed_commit: None,
            proof: &leaf.proof,
            public_inputs: &leaf.public,
        },
        config(p),
        backend(),
        params(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let build = t.elapsed();
    let t = std::time::Instant::now();
    let out = owner
        .prove(TrustedPreparedInput::UniStark { proof: &leaf.proof, public_inputs: &leaf.public })
        .map_err(|e| format!("{e:?}"))?;
    let timing = Timing { build, prove: t.elapsed() };
    Ok(Recursive { out, statement: leaf.public.clone(), verifier: owner.verifier(), timing })
}

/// A 2-to-1 recursive proof over two recursive proofs; its statement is the left statement
/// followed by the right one.
pub fn aggregate(p: &AggParams, left: &Recursive, right: &Recursive) -> Result<Recursive, String> {
    let t = std::time::Instant::now();
    let owner = TrustedPreparedAggregation::<_, _, BatchOnly, BatchOnly, _, 5>::new(
        TrustedPreparedSource::BatchStark {
            verifier: left.verifier.clone(),
            proof: &left.out.0,
            statement: &left.statement,
        },
        TrustedPreparedSource::BatchStark {
            verifier: right.verifier.clone(),
            proof: &right.out.0,
            statement: &right.statement,
        },
        config(p),
        backend(),
        params(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let build = t.elapsed();
    let t = std::time::Instant::now();
    let out = owner
        .prove(
            TrustedPreparedInput::BatchStark { proof: &left.out.0, statement: &left.statement },
            TrustedPreparedInput::BatchStark { proof: &right.out.0, statement: &right.statement },
        )
        .map_err(|e| format!("{e:?}"))?;
    let statement = [left.statement.as_slice(), right.statement.as_slice()].concat();
    let timing = Timing { build, prove: t.elapsed() };
    Ok(Recursive { out, statement, verifier: owner.verifier(), timing })
}

/// Serialized size of a recursive proof.
pub fn proof_bytes(p: &BatchStarkProof<AggConfig>) -> usize {
    postcard::to_allocvec(p).map(|v| v.len()).unwrap_or(0)
}

/// The shape of a recursive (circuit-table) batch proof for the security calculator: its
/// tallest table, every opened codeword (base columns count 1, extension columns 5), the
/// degree its quotient chunking allows, and upper estimates for what the proof does not carry
/// (constraints: 4 per opened codeword; LogUp: 4 interactions of width 8 per permutation column).
pub fn recursive_shape(p: &BatchStarkProof<AggConfig>) -> ProofShape {
    const E: usize = 5;
    let bp = &p.proof;
    let log_height = bp.degree_bits.iter().copied().max().unwrap_or(0);
    let (mut openings, mut perm_cols, mut chunks) = (0usize, 0usize, 1usize);
    for inst in &bp.opened_values.instances {
        let b = &inst.base_opened_values;
        openings += b.trace_local.len() + b.trace_next.as_ref().map_or(0, Vec::len);
        if let Some(pre) = &b.preprocessed {
            openings += pre.local.len() + pre.next.as_ref().map_or(0, Vec::len);
        }
        openings += E * b.quotient_chunks.iter().map(Vec::len).sum::<usize>();
        openings += E * (inst.permutation_local.len() + inst.permutation_next.len());
        perm_cols += inst.permutation_local.len();
        chunks = chunks.max(b.quotient_chunks.len());
    }
    ProofShape {
        log_height,
        num_constraints: 4 * openings,
        max_degree: chunks + 1,
        batched_openings: openings,
        logup: (perm_cols > 0).then_some((4 * perm_cols, 8)),
    }
}
