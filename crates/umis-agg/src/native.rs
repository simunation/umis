//! Layer 1 over a **native batch leaf**: a `p3_batch_stark::BatchProof` of several AIRs joined by
//! LogUp buses (the stencil leaf: cells, Poseidon2 hash table, range table), verified
//! in-circuit by Plonky3-recursion's `verify_batch_circuit`.
//!
//! P3R's trusted owners take only uni-STARK leaves or P3R's own circuit proofs, so this module
//! assembles the same trusted construction from P3R's public pieces (INV-AGG-NB-*,
//! `docs/zk/aggregation.md` §native batch):
//!
//! 1. The leaf's AIRs, configuration and FRI parameters are circuit structure (fixed when the
//!    owner is built); the representative proof only fixes the shape (heights, opened widths).
//! 2. **VK pin** (INV-AGG-NB-1): the leaf's preprocessed commitment (`CommonData`) targets are
//!    constrained to the owner's constants, so a leaf for another relation cannot be verified.
//! 3. **Statement** (INV-AGG-NB-2): exactly the leaf's AIR public values (for a stencil leaf
//!    `[d_prev, d_halo, d_next]`, 24 elements) are exported as the recursive proof's statement.
//!    Every other circuit input (leaf commitments, openings, Merkle paths) is a witness: the
//!    circuit prover's verifier (`CircuitVerifier::verify(proof, statement)`) sees only the
//!    statement table, so the low-level allocator's `public_input()` targets are not exported.
//! 4. The in-circuit verifier replays the leaf's Fiat–Shamir transcript itself, so the witness
//!    commitments are bound by the transcript and FRI checks.
//!
//! Upper layers ([`crate::AggOwner`]) take these recursive proofs like any other.

use crate::{AggConfig, Backend, Recursive, Timing, Val, backend, config, params};
use p3_air::Air;
use p3_batch_stark::{BatchProof, CommonData, ProverData, StarkInstance, prove_batch, verify_batch};
use p3_circuit::{Circuit, CircuitBuilder, NonPrimitiveOpId, StatementExport};
use p3_circuit_prover::batch_stark_prover::{CircuitVerifier, TableProver};
use p3_circuit_prover::common::{NpoAirBuilder, NpoPreprocessor};
use p3_circuit_prover::{
    BatchStarkProver, PreparedCircuitProver, StatementAirBuilder, StatementPreprocessor, StatementProver,
};
use p3_lookup::InteractionSymbolicBuilder;
use p3_lookup::logup::LogUpGadget;
use p3_recursion::prepared::ConstrainConstantCommitment;
use p3_recursion::{
    BatchOnly, BatchStarkVerifierInputsBuilder, FriRecursionConfig, PcsRecursionBackend, Poseidon2Config,
    RecursionOutput, replay_batch_stark_transcript, verify_batch_circuit,
};
use p3_uni_stark::StarkGenericConfig;

type Challenge = <AggConfig as StarkGenericConfig>::Challenge;
type Comm = <AggConfig as FriRecursionConfig>::Commitment;
type OpenProof = <AggConfig as FriRecursionConfig>::OpeningProof;
type InProof = <AggConfig as FriRecursionConfig>::InputProof;
type Inputs = BatchStarkVerifierInputsBuilder<AggConfig, Comm, OpenProof>;

/// What a native batch AIR must implement to be proved, verified and recursed under the
/// aggregation configuration (debug builds also run the prover's constraint checker).
#[cfg(not(debug_assertions))]
pub trait NativeAir:
    Clone
    + Air<InteractionSymbolicBuilder<Val, Challenge>>
    + for<'a> Air<p3_batch_stark::folder::ProverConstraintFolderWithLookups<'a, AggConfig>>
    + for<'a> Air<p3_batch_stark::folder::VerifierConstraintFolderWithLookups<'a, AggConfig>>
{
}
#[cfg(not(debug_assertions))]
impl<T> NativeAir for T where
    T: Clone
        + Air<InteractionSymbolicBuilder<Val, Challenge>>
        + for<'a> Air<p3_batch_stark::folder::ProverConstraintFolderWithLookups<'a, AggConfig>>
        + for<'a> Air<p3_batch_stark::folder::VerifierConstraintFolderWithLookups<'a, AggConfig>>
{
}

/// What a native batch AIR must implement to be proved, verified and recursed under the
/// aggregation configuration (debug builds also run the prover's constraint checker).
#[cfg(debug_assertions)]
pub trait NativeAir:
    Clone
    + Air<InteractionSymbolicBuilder<Val, Challenge>>
    + for<'a> Air<p3_batch_stark::folder::ProverConstraintFolderWithLookups<'a, AggConfig>>
    + for<'a> Air<p3_batch_stark::folder::VerifierConstraintFolderWithLookups<'a, AggConfig>>
    + for<'a> Air<p3_air::DebugConstraintBuilder<'a, Val, Challenge>>
{
}
#[cfg(debug_assertions)]
impl<T> NativeAir for T where
    T: Clone
        + Air<InteractionSymbolicBuilder<Val, Challenge>>
        + for<'a> Air<p3_batch_stark::folder::ProverConstraintFolderWithLookups<'a, AggConfig>>
        + for<'a> Air<p3_batch_stark::folder::VerifierConstraintFolderWithLookups<'a, AggConfig>>
        + for<'a> Air<p3_air::DebugConstraintBuilder<'a, Val, Challenge>>
{
}

/// A native batch relation under the aggregation configuration: its AIRs, trace heights and
/// the setup (preprocessed commitment) a leaf is proved and verified with.
pub struct NativeRelation<A> {
    pub airs: Vec<A>,
    pub degrees: Vec<usize>,
    pub prover_data: ProverData<AggConfig>,
}

impl<A> NativeRelation<A>
where
    A: NativeAir,
{
    /// Setup of `airs` at `degrees` (log₂ heights) with LogUp degree budget `budget`.
    pub fn new(p: &crate::AggParams, airs: Vec<A>, degrees: Vec<usize>, budget: usize) -> Result<Self, String> {
        let prover_data = ProverData::from_airs_and_degrees_with_lookup_budgets(
            &config(p),
            &airs,
            &degrees,
            &vec![budget; airs.len()],
            p.fri.log_blowup,
        )
        .map_err(|e| format!("setup: {e:?}"))?;
        Ok(Self { airs, degrees, prover_data })
    }

    pub fn common(&self) -> &CommonData<AggConfig> {
        &self.prover_data.common
    }

    /// Proves one leaf (`traces[i]` for `airs[i]`, per-AIR public values `publics`) and checks it
    /// natively.
    pub fn prove(
        &self,
        p: &crate::AggParams,
        traces: &[p3_matrix::dense::RowMajorMatrix<Val>],
        publics: Vec<Vec<Val>>,
    ) -> Result<NativeLeaf, String> {
        let cfg = config(p);
        if traces.len() != self.airs.len() || publics.len() != self.airs.len() {
            return Err("one trace and one public vector per AIR".into());
        }
        let instances: Vec<StarkInstance<'_, AggConfig, A>> = self
            .airs
            .iter()
            .zip(traces)
            .zip(&publics)
            .map(|((air, trace), pv)| StarkInstance { air, trace, public_values: pv.clone() })
            .collect();
        let proof = prove_batch(&cfg, &instances, &self.prover_data).map_err(|e| format!("{e:?}"))?;
        self.verify(p, &proof, &publics)?;
        Ok(NativeLeaf { proof, publics })
    }

    /// Native verification of a leaf.
    pub fn verify(
        &self,
        p: &crate::AggParams,
        proof: &BatchProof<AggConfig>,
        publics: &[Vec<Val>],
    ) -> Result<(), String> {
        verify_batch(&config(p), &self.airs, proof, publics, self.common()).map_err(|e| format!("{e:?}"))
    }
}

/// A native batch leaf and its per-AIR public values.
pub struct NativeLeaf {
    pub proof: BatchProof<AggConfig>,
    pub publics: Vec<Vec<Val>>,
}

impl NativeLeaf {
    /// The statement a recursive proof of this leaf carries: the public values, AIR by AIR.
    pub fn statement(&self) -> Vec<Val> {
        self.publics.concat()
    }

    /// Serialized size.
    pub fn bytes(&self) -> usize {
        postcard::to_allocvec(&self.proof).map(|v| v.len()).unwrap_or(0)
    }
}

/// A layer-1 owner built once per native relation and leaf shape: it proves the recursive
/// proof of any leaf of that relation and shape (INV-AGG-NB-3).
pub struct NativeLayer1<'r, A> {
    rel: &'r NativeRelation<A>,
    params: crate::AggParams,
    circuit: Circuit<Challenge>,
    inputs: Inputs,
    op_ids: Vec<NonPrimitiveOpId>,
    counts: Vec<usize>,
    degree_bits: Vec<usize>,
    prepared: PreparedCircuitProver<AggConfig>,
    verifier: CircuitVerifier<AggConfig>,
    pub build: std::time::Duration,
}

impl<'r, A> NativeLayer1<'r, A>
where
    A: NativeAir + p3_recursion::RecursiveAir<Val, Challenge, LogUpGadget>,
{
    /// Builds the verifier circuit of `rel` from a representative leaf.
    pub fn new(p: &crate::AggParams, rel: &'r NativeRelation<A>, representative: &NativeLeaf) -> Result<Self, String> {
        let t = std::time::Instant::now();
        let cfg = config(p);
        let be: Backend = backend();
        rel.verify(p, &representative.proof, &representative.publics)?;
        let common = rel.common();
        let pre = common.preprocessed.as_ref().ok_or("a native batch relation without preprocessed columns")?;
        let counts: Vec<usize> = representative.publics.iter().map(Vec::len).collect();

        let mut b = CircuitBuilder::new();
        <Backend as PcsRecursionBackend<AggConfig, A, 5>>::prepare_circuit(&be, &cfg, &mut b)
            .map_err(|e| format!("{e:?}"))?;
        let inputs = Inputs::allocate(&mut b, &representative.proof, common, &counts).map_err(|e| format!("{e:?}"))?;
        let op_ids =
            verify_batch_circuit::<A, AggConfig, Comm, InProof, OpenProof, LogUpGadget, Poseidon2Config, 16, 8>(
                &cfg,
                &rel.airs,
                &mut b,
                &inputs.proof_targets,
                &inputs.air_public_targets,
                cfg.pcs_verifier_params(),
                &inputs.common_data,
                &LogUpGadget::new(),
                Poseidon2Config::KOALA_BEAR_D1_W16,
            )
            .map_err(|e| format!("{e:?}"))?;
        // INV-AGG-NB-1: the leaf relation's preprocessed commitment is a circuit constant.
        let target =
            inputs.common_data.preprocessed_commitment().ok_or("circuit allocated no preprocessed commitment")?;
        target.constrain_constant(&mut b, &pre.commitment).map_err(|e| format!("{e:?}"))?;
        // INV-AGG-NB-2: the statement is the leaf's public values, nothing else.
        let exports: Vec<StatementExport> =
            inputs.air_public_targets.iter().flatten().map(|t| StatementExport::Base(*t)).collect();
        let schema = b.set_statement_exports::<Val>(&exports).map_err(|e| format!("{e:?}"))?;
        let circuit = b.build().map_err(|e| format!("{e:?}"))?;

        let lp = params();
        let mut preprocessors: Vec<Box<dyn NpoPreprocessor<Val>>> =
            <Backend as PcsRecursionBackend<AggConfig, BatchOnly, 5>>::non_primitive_preprocessors(&be);
        let mut air_builders: Vec<Box<dyn NpoAirBuilder<AggConfig, 5>>> =
            <Backend as PcsRecursionBackend<AggConfig, BatchOnly, 5>>::non_primitive_air_builders(&be);
        let mut provers: Vec<Box<dyn TableProver<AggConfig>>> =
            <Backend as PcsRecursionBackend<AggConfig, BatchOnly, 5>>::non_primitive_provers(&be, 5);
        preprocessors.push(Box::new(StatementPreprocessor::new(schema.clone())));
        air_builders.push(Box::new(StatementAirBuilder::<5>::new(schema.clone())));
        provers.push(Box::new(StatementProver::<5>::new(schema)));
        let mut prover = BatchStarkProver::new(cfg.clone())
            .with_table_packing(lp.table_packing.clone())
            .with_alu_variant(match lp.constraint_profile {
                p3_circuit_prover::ConstraintProfile::Standard => p3_circuit_prover::AirVariant::Baseline,
                p3_circuit_prover::ConstraintProfile::RecursionOptimized => p3_circuit_prover::AirVariant::Optimized,
            });
        for pr in provers {
            prover.register_table_prover(pr);
        }
        let prepared = prover
            .prepare_circuit::<Challenge, 5>(&circuit, &preprocessors, &air_builders, lp.constraint_profile)
            .map_err(|e| format!("{e:?}"))?;
        let verifier = prepared.verifier();
        Ok(Self {
            rel,
            params: *p,
            circuit,
            inputs,
            op_ids,
            counts,
            degree_bits: representative.proof.degree_bits.clone(),
            prepared,
            verifier,
            build: t.elapsed(),
        })
    }

    /// The verifier of this owner's recursive proofs (retain it; check proofs against the
    /// expected statement).
    pub fn verifier(&self) -> CircuitVerifier<AggConfig> {
        self.verifier.clone()
    }

    /// The recursive proof of `leaf`; its statement is the leaf's public values.
    pub fn prove(&self, leaf: &NativeLeaf) -> Result<Recursive, String> {
        let t = std::time::Instant::now();
        if leaf.proof.degree_bits != self.degree_bits
            || leaf.publics.iter().map(Vec::len).collect::<Vec<_>>() != self.counts
        {
            return Err("leaf shape differs from the owner's".into());
        }
        self.rel.verify(&self.params, &leaf.proof, &leaf.publics)?;
        self.prove_unchecked(leaf, self.rel.common(), t)
    }

    /// [`Self::prove`] without the native pre-checks, with the witness packed from `common`
    /// (an attacker's own setup): the circuit alone decides. Tests use it to show that the
    /// circuit rejects what the pre-checks would have caught.
    #[doc(hidden)]
    pub fn prove_unchecked(
        &self,
        leaf: &NativeLeaf,
        common: &CommonData<AggConfig>,
        t: std::time::Instant,
    ) -> Result<Recursive, String> {
        let p = &self.params;
        let cfg = config(p);
        let (public, private) = self.inputs.pack_values(&leaf.publics, &leaf.proof, common);
        let mut runner = self.circuit.runner();
        runner.set_public_inputs(&public).map_err(|e| format!("{e:?}"))?;
        runner.set_private_inputs(&private).map_err(|e| format!("{e:?}"))?;
        let (transcript, _) = replay_batch_stark_transcript(
            &self.rel.airs,
            &cfg,
            &leaf.proof,
            &leaf.publics,
            common,
            &LogUpGadget::new(),
        )
        .map_err(|e| format!("{e:?}"))?;
        <AggConfig as FriRecursionConfig>::set_fri_private_data(
            &cfg,
            &mut runner,
            &self.op_ids,
            &leaf.proof.opening_proof,
            transcript,
        )
        .map_err(|e| e.to_string())?;
        let traces = runner.run().map_err(|e| format!("{e:?}"))?;
        let (proof, data) = self.prepared.prove_with_legacy_data(&traces).map_err(|e| format!("{e:?}"))?;
        let timing = Timing { build: std::time::Duration::ZERO, prove: t.elapsed() };
        Ok(Recursive {
            out: RecursionOutput(proof, data),
            statement: leaf.statement(),
            verifier: self.verifier(),
            timing,
        })
    }
}
