//! Proofs of zone transitions.
//!
//! A [`TransitionStatement`] says: zone `z` with params `params_hash`, from
//! state `prev_state_root` at slot `from_slot`, consuming the inputs committed
//! by `inputs_commitment`, reaches `state_root` with outputs `outputs_root`
//! at `to_slot`. A [`ProofBackend`] proves and verifies statements; genesis
//! selects the backend. Proof leaves ([`ProofLeaf`]) bind a statement to the
//! bmt file address of its proof bytes, and a commitment's proof BMT is the
//! list root over its leaves in slot order.

use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use umis_core::records::{ProofLeaf, TransitionStatement};
use umis_core::{Hash, NodeId, SecretKey, SegRecord, Signature, bmt};
use umis_task::{Bytes, PortId, ZoneInstance};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    #[error("malformed proof: {0}")]
    Malformed(String),
    #[error("proof does not verify: {0}")]
    Invalid(String),
    #[error("prover failed: {0}")]
    Prover(String),
}

/// What the prover needs besides the statement.
pub struct Witness<'a> {
    pub instance: &'a dyn ZoneInstance,
    /// The zone's task params (borsh) and the genesis seed, for backends that
    /// rebuild the task's own context (the STARK AIR needs its coefficients).
    pub params: &'a [u8],
    pub genesis_seed: Hash,
    /// State at `from_slot`.
    pub prev_state: &'a [u8],
    /// Input payloads by input port.
    pub inputs: &'a BTreeMap<PortId, Bytes>,
}

/// What a verifier needs besides the statement and the proof. The mock
/// backend needs nothing; the STARK backend evaluates its periodic columns
/// from the states and halos and checks them against the statement's roots.
#[derive(Default, Clone, Copy)]
pub struct PublicData<'a> {
    pub params: Option<&'a [u8]>,
    pub genesis_seed: Option<Hash>,
    pub prev_state: Option<&'a [u8]>,
    pub next_state: Option<&'a [u8]>,
    pub inputs: Option<&'a BTreeMap<PortId, Bytes>>,
    /// The InputRefs behind `inputs_commitment`, in link order.
    pub input_refs: Option<&'a [umis_core::records::InputRef]>,
}

/// Public data shipped with a STARK proof commitment: the states at every
/// leaf boundary (`leaves + 1`) and each leaf's input payloads.
#[derive(Debug, Clone, PartialEq, Eq, Default, BorshSerialize, BorshDeserialize)]
pub struct LeafData {
    pub states: Vec<Bytes>,
    pub inputs: Vec<Vec<(PortId, Bytes)>>,
}

/// One slot's proving job, self-contained so it can run in a worker that
/// only has the genesis seed and the task registry.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProveJob {
    pub id: u64,
    pub block_hash: Hash,
    pub stmt: TransitionStatement,
    pub task_id: String,
    pub params: Bytes,
    pub genesis_seed: Hash,
    /// State at the start of the slot.
    pub prev_state: Bytes,
    pub inputs: BTreeMap<PortId, Bytes>,
}

/// Runs a job: instantiates the task and proves.
pub fn run_job(
    tasks: &umis_task::TaskRegistry,
    backend: &dyn ProofBackend,
    key: &SecretKey,
    job: &ProveJob,
) -> Result<Bytes, ProofError> {
    let task = tasks.get(&job.task_id).map_err(|e| ProofError::Prover(e.to_string()))?;
    let inst = task.instantiate(&job.params, &job.genesis_seed).map_err(|e| ProofError::Prover(e.to_string()))?;
    run_job_on(inst.as_ref(), backend, key, job)
}

/// Runs a job on an already prepared instance (workers cache them).
pub fn run_job_on(
    inst: &dyn ZoneInstance,
    backend: &dyn ProofBackend,
    key: &SecretKey,
    job: &ProveJob,
) -> Result<Bytes, ProofError> {
    let w = Witness {
        instance: inst,
        params: &job.params,
        genesis_seed: job.genesis_seed,
        prev_state: &job.prev_state,
        inputs: &job.inputs,
    };
    backend.prove(&job.stmt, &w, key)
}

/// Runs jobs with a cache of prepared instances (per task, params and
/// seed), for executors that see the same zones over and over.
pub struct JobRunner {
    tasks: umis_task::TaskRegistry,
    backend: std::sync::Arc<dyn ProofBackend>,
    key: SecretKey,
    instances: std::sync::Mutex<BTreeMap<Hash, std::sync::Arc<dyn ZoneInstance>>>,
}

impl JobRunner {
    pub fn new(tasks: umis_task::TaskRegistry, backend: std::sync::Arc<dyn ProofBackend>, key: SecretKey) -> Self {
        Self { tasks, backend, key, instances: Default::default() }
    }

    pub fn run(&self, job: &ProveJob) -> Result<Bytes, ProofError> {
        let k = umis_core::hash::keccak256_concat(&[
            job.task_id.as_bytes(),
            &[0],
            &job.params,
            job.genesis_seed.as_bytes(),
        ]);
        let cached = self.instances.lock().expect("instances").get(&k).cloned();
        let inst = match cached {
            Some(i) => i,
            None => {
                let task = self.tasks.get(&job.task_id).map_err(|e| ProofError::Prover(e.to_string()))?;
                let i =
                    task.instantiate(&job.params, &job.genesis_seed).map_err(|e| ProofError::Prover(e.to_string()))?;
                self.instances.lock().expect("instances").insert(k, i.clone());
                i
            }
        };
        run_job_on(inst.as_ref(), self.backend.as_ref(), &self.key, job)
    }
}

pub trait ProofBackend: Send + Sync {
    fn id(&self) -> &'static str;
    /// Whether proving is slow enough to run off the event loop (a worker or
    /// a blocking thread); the mock backend proves inline.
    fn is_async(&self) -> bool {
        false
    }
    /// Whether `verify` returns the prover's identity (mock: a signature) —
    /// then callers check that the prover was allowed to prove.
    fn identifies_prover(&self) -> bool {
        false
    }
    fn prove(&self, stmt: &TransitionStatement, w: &Witness<'_>, prover: &SecretKey) -> Result<Bytes, ProofError>;
    /// Verifies the proof; returns the prover (for the mock backend, whose
    /// "proof" is a signature, callers check the prover is allowed).
    fn verify(&self, stmt: &TransitionStatement, public: &PublicData<'_>, proof: &[u8]) -> Result<NodeId, ProofError>;
}

/// The mock proof: the prover's signature over the statement's address.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct MockProof {
    pub prover: NodeId,
    pub sig: Signature,
}

/// Mock backend: proving is signing. Validity comes from re-execution by
/// zone members and native full nodes, who report invalid blocks as faults.
#[derive(Debug, Clone, Copy, Default)]
pub struct MockBackend;

pub const MOCK_BACKEND: &str = "mock";

impl ProofBackend for MockBackend {
    fn id(&self) -> &'static str {
        MOCK_BACKEND
    }

    fn identifies_prover(&self) -> bool {
        true
    }

    fn prove(&self, stmt: &TransitionStatement, _w: &Witness<'_>, prover: &SecretKey) -> Result<Bytes, ProofError> {
        let p = MockProof { prover: prover.node_id(), sig: prover.sign(&stmt.address()) };
        Ok(borsh::to_vec(&p).expect("borsh"))
    }

    fn verify(&self, stmt: &TransitionStatement, _p: &PublicData<'_>, proof: &[u8]) -> Result<NodeId, ProofError> {
        let p = MockProof::try_from_slice(proof).map_err(|e| ProofError::Malformed(e.to_string()))?;
        if umis_core::crypto::verify(&p.prover, &stmt.address(), &p.sig) {
            Ok(p.prover)
        } else {
            Err(ProofError::Invalid("bad mock signature".into()))
        }
    }
}

/// Leaf of a proof BMT: the statement fields plus the proof's file address.
pub fn proof_leaf(stmt: &TransitionStatement, proof: &[u8]) -> ProofLeaf {
    ProofLeaf {
        zone: stmt.zone,
        from_slot: stmt.from_slot,
        to_slot: stmt.to_slot,
        params_hash: stmt.params_hash,
        prev_state_root: stmt.prev_state_root,
        state_root: stmt.state_root,
        inputs_commitment: stmt.inputs_commitment,
        outputs_root: stmt.outputs_root,
        proof_address: bmt::file_address(proof),
    }
}

pub fn statement_of(leaf: &ProofLeaf) -> TransitionStatement {
    TransitionStatement {
        zone: leaf.zone,
        from_slot: leaf.from_slot,
        to_slot: leaf.to_slot,
        params_hash: leaf.params_hash,
        prev_state_root: leaf.prev_state_root,
        state_root: leaf.state_root,
        inputs_commitment: leaf.inputs_commitment,
        outputs_root: leaf.outputs_root,
    }
}

/// `proof_bmt_root`: list root over the leaf addresses (span = 32 · n).
pub fn proof_bmt_root(leaves: &[ProofLeaf]) -> Hash {
    umis_core::records::list_root_of(leaves)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    #[error("no leaves")]
    Empty,
    #[error("leaf {0} does not continue the previous one")]
    Broken(usize),
}

/// Leaves must chain: consecutive slots, `state_root` of one is the
/// `prev_state_root` of the next, same zone and params.
pub fn check_leaf_chain(leaves: &[ProofLeaf]) -> Result<(), ChainError> {
    if leaves.is_empty() {
        return Err(ChainError::Empty);
    }
    for (i, w) in leaves.windows(2).enumerate() {
        let (a, b) = (&w[0], &w[1]);
        if a.zone != b.zone
            || a.params_hash != b.params_hash
            || a.to_slot != b.from_slot
            || a.state_root != b.prev_state_root
        {
            return Err(ChainError::Broken(i + 1));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stmt(from: u64, prev: &[u8], next: &[u8]) -> TransitionStatement {
        TransitionStatement {
            zone: 1,
            from_slot: from,
            to_slot: from + 1,
            params_hash: umis_core::keccak256(b"p"),
            prev_state_root: umis_core::keccak256(prev),
            state_root: umis_core::keccak256(next),
            inputs_commitment: Hash::ZERO,
            outputs_root: Hash::ZERO,
        }
    }

    #[test]
    fn mock_prove_verify_and_chain() {
        let k = SecretKey::from_label("prover");
        let b = MockBackend;
        let s0 = stmt(10, b"a", b"b");
        let s1 = stmt(11, b"b", b"c");
        let sig = MockProof { prover: k.node_id(), sig: k.sign(&s0.address()) };
        let proof = borsh::to_vec(&sig).unwrap();
        assert_eq!(b.verify(&s0, &PublicData::default(), &proof).unwrap(), k.node_id());
        assert!(b.verify(&s1, &PublicData::default(), &proof).is_err());
        let l0 = proof_leaf(&s0, &proof);
        let l1 = proof_leaf(&s1, &proof);
        assert!(check_leaf_chain(&[l0.clone(), l1.clone()]).is_ok());
        assert_eq!(check_leaf_chain(&[l1.clone(), l0.clone()]), Err(ChainError::Broken(1)));
        assert_eq!(statement_of(&l0), s0);
        assert_ne!(proof_bmt_root(std::slice::from_ref(&l0)), proof_bmt_root(&[l0, l1]));
    }
}
