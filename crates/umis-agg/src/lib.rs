//! Recursive aggregation of umis leaf proofs with Plonky3-recursion (native only).
//!
//! Status: a spike that measures the cost of aggregation (`docs/zk/aggregation.md`).
//! - [`leaf`]: one life2d slot proved as a uni-STARK under the recursion-compatible
//!   [`FriPreset::R100`] (Poseidon2 Merkle trees, no OOD/batch/lookup grinding).
//! - [`layer1`]: a recursive proof that verifies one leaf in-circuit.
//! - [`aggregate`]: a 2-to-1 recursive proof over two recursive proofs.
//!
//! Plonky3-recursion is unaudited (its README says so); this crate is pinned to one commit.

use std::sync::Arc;

use p3_circuit::ops::{generate_poseidon2_trace, generate_recompose_trace};
use p3_circuit::{CircuitBuilder, CircuitRunner, NonPrimitiveOpId};
use p3_circuit_prover::BatchStarkProof;
use p3_circuit_prover::batch_stark_prover::BatchStarkProver;
use p3_commit::Pcs;
use p3_lookup::logup::LogUpGadget;
use p3_poseidon2_circuit_air::KoalaBearD4Width16;
use p3_recursion::pcs::{
    FriProofTargets, InputProofTargets, MerkleCapTargets, RecExtensionValMmcs, RecValMmcs, restore_fri_query_paths,
    set_fri_mmcs_private_data,
};
use p3_recursion::traits::{RecursiveAir, RecursivePcs};
use p3_recursion::verifier::VerificationError;
use p3_recursion::{
    BatchOnly, FriRecursionBackend, FriRecursionBackendForExt, FriRecursionConfig, FriVerifierParams, NativeFriParams,
    OpeningTranscript, Poseidon2Config, ProveNextLayerParams, RecursionInput, RecursionOutput,
    build_and_prove_aggregation_layer, build_and_prove_next_layer, observe_opened_values,
};
use p3_uni_stark::{Proof, StarkGenericConfig, Val as ValOf, prove, verify};
use task_life2d::Life2dParams;
use task_life2d::air::{LifeAir, Shape, claim_data, generate_trace};
use task_life2d::board::Board;
use umis_core::Hash;
use umis_stark::config::FriPreset;
use umis_stark::config::koala_bear::{
    Challenge, ChallengeMmcs, Challenger, Compress, Config, Dft, Hash as P2Hash, Pcs as MyPcs, Val, ValMmcs,
};

const DIGEST_ELEMS: usize = 8;

/// The opening-proof targets of a KoalaBear D4 `TwoAdicFriPcs` verifier circuit.
pub type InnerFri = FriProofTargets<
    Val,
    Challenge,
    RecExtensionValMmcs<Val, Challenge, DIGEST_ELEMS, RecValMmcs<Val, DIGEST_ELEMS, P2Hash, Compress>>,
    InputProofTargets<Val, Challenge, RecValMmcs<Val, DIGEST_ELEMS, P2Hash, Compress>>,
    p3_recursion::pcs::Witness<Val>,
>;

/// The recursion backend for KoalaBear D4 with width-16 Poseidon2.
pub type Backend = FriRecursionBackendForExt<4, 16, 8, Poseidon2Config>;

/// The umis KoalaBear/Poseidon2 STARK config at a preset, with the FRI verifier parameters a
/// recursive verifier circuit needs (Plonky3-recursion's `FriRecursionConfig`).
#[derive(Clone)]
pub struct AggConfig {
    config: Arc<Config>,
    fri_verifier_params: FriVerifierParams,
    native_fri_params: NativeFriParams,
    val_mmcs: ValMmcs,
    fri_params: p3_fri::FriParameters<ChallengeMmcs>,
}

impl core::ops::Deref for AggConfig {
    type Target = Config;
    fn deref(&self) -> &Config {
        &self.config
    }
}

impl AggConfig {
    /// `preset` must not grind at the OOD, batch or lookup sites ([`FriPreset::R100`] or
    /// [`FriPreset::Test`]).
    pub fn new(preset: FriPreset) -> Self {
        assert_eq!(preset.ood_pow_bits() + preset.lookup_pow_bits(), 0, "recursion rejects OOD/lookup grinding");
        let perm = p3_koala_bear::default_koalabear_poseidon2_16();
        let val_mmcs = ValMmcs::new(P2Hash::new(perm.clone()), Compress::new(perm.clone()), 0);
        let fri_params = preset.fri_params(ChallengeMmcs::new(val_mmcs.clone()));
        assert_eq!(fri_params.batch_proof_of_work_bits, 0, "recursion rejects batch grinding");
        let native_fri_params = NativeFriParams::try_from_native::<Val, _>(&fri_params).expect("fri params");
        let fri_verifier_params = FriVerifierParams::with_mmcs(
            native_fri_params.log_blowup(),
            native_fri_params.log_final_poly_len(),
            native_fri_params.max_log_arity(),
            native_fri_params.commit_pow_bits(),
            native_fri_params.query_pow_bits(),
            native_fri_params.num_queries(),
            Poseidon2Config::KOALA_BEAR_D4_W16,
        );
        let pcs = MyPcs::new(Dft::default(), val_mmcs.clone(), fri_params.clone());
        let config = Arc::new(Config::new(pcs, Challenger::new(perm)));
        Self { config, fri_verifier_params, native_fri_params, val_mmcs, fri_params }
    }
}

impl StarkGenericConfig for AggConfig {
    type Challenge = Challenge;
    type Challenger = Challenger;
    type Pcs = MyPcs;
    fn pcs(&self) -> &MyPcs {
        self.config.pcs()
    }
    fn initialise_challenger(&self) -> Challenger {
        self.config.initialise_challenger()
    }
}

type InputTargets = InputProofTargets<Val, Challenge, RecValMmcs<Val, DIGEST_ELEMS, P2Hash, Compress>>;

impl FriRecursionConfig for AggConfig
where
    MyPcs: RecursivePcs<
            Self,
            InputTargets,
            InnerFri,
            MerkleCapTargets<Val, DIGEST_ELEMS>,
            <MyPcs as Pcs<Challenge, Challenger>>::Domain,
        >,
{
    type Commitment = MerkleCapTargets<Val, DIGEST_ELEMS>;
    type InputProof = InputTargets;
    type OpeningProof = InnerFri;
    type RawOpeningProof = <MyPcs as Pcs<Challenge, Challenger>>::Proof;
    const DIGEST_ELEMS: usize = DIGEST_ELEMS;

    fn native_fri_validation_params(&self) -> Option<NativeFriParams> {
        Some(self.native_fri_params)
    }

    fn with_fri_opening_proof<'a, A, R>(
        prev: &RecursionInput<'a, Self, A>,
        f: impl FnOnce(&Self::RawOpeningProof) -> R,
    ) -> R
    where
        A: RecursiveAir<ValOf<Self>, Self::Challenge, LogUpGadget>,
    {
        match prev {
            RecursionInput::UniStark { proof, .. } => f(&proof.opening_proof),
            RecursionInput::BatchStark { proof, .. } => f(&proof.proof.opening_proof),
        }
    }

    fn prepare_circuit_for_verification(
        &self,
        circuit: &mut CircuitBuilder<Challenge>,
    ) -> Result<(), VerificationError> {
        circuit.enable_poseidon2_perm::<KoalaBearD4Width16, _>(
            generate_poseidon2_trace::<Challenge, KoalaBearD4Width16>,
            p3_koala_bear::default_koalabear_poseidon2_16(),
        );
        circuit.enable_recompose::<Val>(generate_recompose_trace::<Val, Challenge>);
        Ok(())
    }

    fn pcs_verifier_params(
        &self,
    ) -> &<MyPcs as RecursivePcs<
        Self,
        InputTargets,
        InnerFri,
        MerkleCapTargets<Val, DIGEST_ELEMS>,
        <MyPcs as Pcs<Challenge, Challenger>>::Domain,
    >>::VerifierParams {
        &self.fri_verifier_params
    }

    fn set_fri_private_data(
        config: &Self,
        runner: &mut CircuitRunner<'_, Challenge>,
        op_ids: &[NonPrimitiveOpId],
        opening_proof: &Self::RawOpeningProof,
        transcript: OpeningTranscript<Self>,
    ) -> Result<(), &'static str> {
        let OpeningTranscript { mut challenger, commitments_with_opening_points } = transcript;
        observe_opened_values::<Self>(
            &mut challenger,
            &commitments_with_opening_points,
            config.fri_params.batch_proof_of_work_bits,
        );
        let claims: Vec<_> = commitments_with_opening_points.into_iter().map(Into::into).collect();
        let paths = restore_fri_query_paths(
            &config.fri_params,
            &config.val_mmcs,
            &config.val_mmcs,
            opening_proof,
            &mut challenger,
            &claims,
        )
        .map_err(|_| "restoring the FRI query paths")?;
        set_fri_mmcs_private_data::<Val, Challenge, DIGEST_ELEMS>(
            runner,
            op_ids,
            &paths,
            Poseidon2Config::KOALA_BEAR_D4_W16,
        )
    }
}

/// The recursion backend.
pub fn backend() -> Backend {
    FriRecursionBackend::<16, 8, _>::new(Poseidon2Config::KOALA_BEAR_D4_W16).for_extension_degree::<4>()
}

/// One life2d slot proved for aggregation: its AIR (with the claim's periodic data), proof
/// and public values.
pub struct Leaf {
    pub air: LifeAir,
    pub proof: Proof<AggConfig>,
    pub public: Vec<Val>,
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

/// A recursive proof that verifies `leaf` in-circuit.
pub fn layer1(cfg: &AggConfig, leaf: &Leaf) -> Result<RecursionOutput<AggConfig>, String> {
    let input = RecursionInput::UniStark {
        proof: &leaf.proof,
        air: &leaf.air,
        public_inputs: leaf.public.clone(),
        preprocessed_commit: None,
    };
    build_and_prove_next_layer::<_, _, _, 4>(&input, cfg, &backend(), &ProveNextLayerParams::default())
        .map_err(|e| format!("{e:?}"))
}

/// A 2-to-1 recursive proof over two recursive proofs.
pub fn aggregate(
    cfg: &AggConfig,
    left: &RecursionOutput<AggConfig>,
    right: &RecursionOutput<AggConfig>,
) -> Result<RecursionOutput<AggConfig>, String> {
    let l = left.into_recursion_input::<BatchOnly>();
    let r = right.into_recursion_input::<BatchOnly>();
    build_and_prove_aggregation_layer::<_, _, _, _, 4>(&l, &r, cfg, &backend(), &ProveNextLayerParams::default())
        .map_err(|e| format!("{e:?}"))
}

/// Verifies a recursive proof natively.
pub fn verify_recursive(cfg: &AggConfig, out: &RecursionOutput<AggConfig>) -> Result<(), String> {
    let mut v = BatchStarkProver::new(cfg.clone()).with_table_packing(ProveNextLayerParams::default().table_packing);
    v.register_poseidon2_table::<4>(Poseidon2Config::KOALA_BEAR_D4_W16.for_shared_challenger_table());
    v.register_recompose_table::<4>(true);
    v.verify_all_tables::<Challenge>(&out.0).map_err(|e| format!("{e:?}"))
}

/// Serialized size of a recursive proof.
pub fn proof_bytes(p: &BatchStarkProof<AggConfig>) -> usize {
    postcard::to_allocvec(p).map(|v| v.len()).unwrap_or(0)
}

/// The shape of a recursive (circuit-table) batch proof for the security calculator: its
/// tallest table, every opened codeword (base columns count 1, extension columns 4), and
/// upper estimates for what the proof does not carry (constraints: 4 per opened codeword;
/// LogUp: 4 interactions of width 8 per permutation column).
pub fn recursive_shape(p: &BatchStarkProof<AggConfig>) -> umis_stark::security::ProofShape {
    let bp = &p.proof;
    let log_height = bp.degree_bits.iter().copied().max().unwrap_or(0);
    let (mut openings, mut perm_cols) = (0usize, 0usize);
    for inst in &bp.opened_values.instances {
        let b = &inst.base_opened_values;
        openings += b.trace_local.len() + b.trace_next.as_ref().map_or(0, Vec::len);
        if let Some(pre) = &b.preprocessed {
            openings += pre.local.len() + pre.next.as_ref().map_or(0, Vec::len);
        }
        openings += 4 * b.quotient_chunks.iter().map(Vec::len).sum::<usize>();
        openings += 4 * (inst.permutation_local.len() + inst.permutation_next.len());
        perm_cols += inst.permutation_local.len();
    }
    umis_stark::security::ProofShape {
        log_height,
        num_constraints: 4 * openings,
        max_degree: 3,
        batched_openings: openings,
        logup: (perm_cols > 0).then_some((4 * perm_cols, 8)),
    }
}
