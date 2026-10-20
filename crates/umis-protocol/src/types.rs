//! Transport types (borsh). Committed parts are `seg` records from
//! `umis_core::records`; envelopes add signatures and the data validators
//! need, and are never hashed as a whole.

use borsh::{BorshDeserialize, BorshSerialize};
use umis_core::bmt::SegmentProof;
use umis_core::records::*;
use umis_core::{Hash, NodeId, SegRecord, Signature};

pub type ZoneId = u32;
pub type Bytes = Vec<u8>;

/// A zone block header and its minter's signature over the header address.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedHeader {
    pub header: ZoneBlockHeader,
    pub sig: Signature,
}

impl SignedHeader {
    pub fn hash(&self) -> Hash {
        self.header.address()
    }

    pub fn verify_sig(&self) -> bool {
        umis_core::crypto::verify(&self.header.minter, &self.hash(), &self.sig)
    }
}

/// The payload an `InputRef` consumes, plus what lets anyone check it: the
/// producer's signed header and the payload's inclusion proof at its port
/// index under the header's `outputs_root`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct InputAttachment {
    pub producer: SignedHeader,
    pub port_index: u32,
    pub payload: Bytes,
    pub proof: SegmentProof,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedVote {
    pub vote: ZoneVote,
    pub sig: Signature,
}

impl SignedVote {
    pub fn verify_sig(&self) -> bool {
        umis_core::crypto::verify(&self.vote.voter, &self.vote.address(), &self.sig)
    }
}

/// A zone block. `attachments[i]` belongs to `inputs[i]`; they make the
/// block self-contained for validation, fault evidence and sync.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ZoneBlock {
    pub signed: SignedHeader,
    pub inputs: Vec<InputRef>,
    pub attachments: Vec<InputAttachment>,
    pub votes: Vec<SignedVote>,
}

impl ZoneBlock {
    pub fn hash(&self) -> Hash {
        self.signed.hash()
    }
    pub fn header(&self) -> &ZoneBlockHeader {
        &self.signed.header
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PortPayload {
    pub port: u32,
    pub payload: Bytes,
    pub proof: SegmentProof,
}

/// Pushed on `zone/<z>/out` right after minting, for consuming zones.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct OutputBundle {
    pub producer: SignedHeader,
    pub ports: Vec<PortPayload>,
    /// The producer block's own inputs (lets consumers follow contamination).
    pub inputs: Vec<InputRef>,
}

/// A transition proof of one zone slot.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProofShare {
    pub leaf: ProofLeaf,
    pub proof: Bytes,
    /// The block whose transition this slot belongs to.
    pub block_hash: Hash,
}

/// A covered block inside a commitment envelope (no payloads).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CommitBlock {
    pub signed: SignedHeader,
    pub inputs: Vec<InputRef>,
    pub votes: Vec<SignedVote>,
}

/// A zone commitment as carried in a beacon block body. `blocks` are the
/// covered zone blocks in slot order, so beacon validators can check the
/// chain linkage, minter legitimacy, inputs, deps and vote tallies.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ZoneCommitEnvelope {
    pub commitment: ZoneCommitment,
    pub sig: Signature,
    pub deps: Vec<Dep>,
    pub decisions: Vec<ZoneDecision>,
    pub blocks: Vec<CommitBlock>,
}

impl ZoneCommitEnvelope {
    pub fn address(&self) -> Hash {
        self.commitment.address()
    }
}

/// A proof commitment with its leaves and proofs (beacon validators verify
/// every proof).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProofCommitEnvelope {
    pub pc: ProofCommitment,
    pub sig: Signature,
    pub leaves: Vec<ProofLeaf>,
    pub proofs: Vec<Bytes>,
    /// Leaf boundary states and inputs, for backends whose verifier needs
    /// them (STARK); empty for the mock backend.
    pub data: umis_proof::LeafData,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProposeZoneTx {
    pub record: ProposeZone,
    pub task_id: String,
    pub params: Bytes,
    pub founders: Vec<NodeId>,
    /// Each founder's signature over the proposal id.
    pub founder_sigs: Vec<Signature>,
}

impl ProposeZoneTx {
    pub fn proposal_id(&self) -> Hash {
        self.record.address()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum FaultBody {
    /// The beacon state transition re-executes the block from `parent_state`.
    InvalidTransition {
        block: ZoneBlock,
        parent_state: Bytes,
    },
    /// `block.inputs[input_index]` is illegitimate, non-causal or not
    /// monotonic (the latter shown against `parent`).
    InvalidInputRef {
        block: ZoneBlock,
        input_index: u32,
        parent: Option<ZoneBlock>,
    },
    Equivocation {
        a: SignedHeader,
        b: SignedHeader,
    },
}

pub const FAULT_INVALID_TRANSITION: u8 = 0;
pub const FAULT_INVALID_INPUT: u8 = 1;
pub const FAULT_EQUIVOCATION: u8 = 2;

impl FaultBody {
    pub fn kind(&self) -> u8 {
        match self {
            FaultBody::InvalidTransition { .. } => FAULT_INVALID_TRANSITION,
            FaultBody::InvalidInputRef { .. } => FAULT_INVALID_INPUT,
            FaultBody::Equivocation { .. } => FAULT_EQUIVOCATION,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct FaultTx {
    pub record: FaultEvidence,
    pub body: FaultBody,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Tx {
    Join(Join),
    Leave(Leave),
    Heartbeat(Heartbeat),
    Grant(Grant),
    ProposeZone(ProposeZoneTx),
    Fault(FaultTx),
}

impl Tx {
    /// The record address: the tx's leaf in `txs_root` and the signed message.
    pub fn address(&self) -> Hash {
        match self {
            Tx::Join(r) => r.address(),
            Tx::Leave(r) => r.address(),
            Tx::Heartbeat(r) => r.address(),
            Tx::Grant(r) => r.address(),
            Tx::ProposeZone(p) => p.record.address(),
            Tx::Fault(f) => f.record.address(),
        }
    }

    /// Who must sign (the faucet signs grants).
    pub fn signer(&self, faucet: &NodeId) -> NodeId {
        match self {
            Tx::Join(r) => r.node,
            Tx::Leave(r) => r.node,
            Tx::Heartbeat(r) => r.node,
            Tx::Grant(_) => *faucet,
            Tx::ProposeZone(p) => p.record.proposer,
            Tx::Fault(f) => f.record.reporter,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Tx::Join(_) => "join",
            Tx::Leave(_) => "leave",
            Tx::Heartbeat(_) => "heartbeat",
            Tx::Grant(_) => "grant",
            Tx::ProposeZone(_) => "propose_zone",
            Tx::Fault(_) => "fault",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedTx {
    pub tx: Tx,
    pub sig: Signature,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, Default)]
pub struct BeaconBody {
    pub txs: Vec<SignedTx>,
    pub zone_commitments: Vec<ZoneCommitEnvelope>,
    pub proof_commitments: Vec<ProofCommitEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BeaconBlock {
    pub header: BeaconBlockHeader,
    pub sig: Signature,
    /// `sig_proposer(SeedMsg{slot})`; the seed is `H(parent.seed ‖ seed_sig)`.
    pub seed_sig: Signature,
    pub body: BeaconBody,
}

impl BeaconBlock {
    pub fn hash(&self) -> Hash {
        self.header.address()
    }
    pub fn slot(&self) -> u64 {
        self.header.slot
    }
}

/// Gossip topics (the runtime prefixes them with `/simu/<genesis8>/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize)]
pub enum Topic {
    BeaconBlock,
    BeaconTx,
    Commit,
    ZoneBlock(ZoneId),
    ZoneProof(ZoneId),
    ZoneOut(ZoneId),
    ZoneVote(ZoneId),
}

impl Topic {
    pub fn path(&self) -> String {
        match self {
            Topic::BeaconBlock => "beacon/block".into(),
            Topic::BeaconTx => "beacon/tx".into(),
            Topic::Commit => "commit".into(),
            Topic::ZoneBlock(z) => format!("zone/{z}/block"),
            Topic::ZoneProof(z) => format!("zone/{z}/proof"),
            Topic::ZoneOut(z) => format!("zone/{z}/out"),
            Topic::ZoneVote(z) => format!("zone/{z}/vote"),
        }
    }

    pub fn parse(path: &str) -> Option<Topic> {
        match path {
            "beacon/block" => return Some(Topic::BeaconBlock),
            "beacon/tx" => return Some(Topic::BeaconTx),
            "commit" => return Some(Topic::Commit),
            _ => {}
        }
        let rest = path.strip_prefix("zone/")?;
        let (z, kind) = rest.split_once('/')?;
        let z: ZoneId = z.parse().ok()?;
        match kind {
            "block" => Some(Topic::ZoneBlock(z)),
            "proof" => Some(Topic::ZoneProof(z)),
            "out" => Some(Topic::ZoneOut(z)),
            "vote" => Some(Topic::ZoneVote(z)),
            _ => None,
        }
    }
}

/// Everything published by gossip.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Gossip {
    BeaconBlock(Box<BeaconBlock>),
    Tx(Box<SignedTx>),
    /// A zone commitment and, as checkpoint, the state blob at its end.
    ZoneCommit(Box<ZoneCommitEnvelope>, Option<Bytes>),
    ProofCommit(Box<ProofCommitEnvelope>),
    ZoneBlock(Box<ZoneBlock>),
    ProofShare(Box<ProofShare>),
    Output(Box<OutputBundle>),
    Vote(Box<SignedVote>),
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct StatusInfo {
    pub head_slot: Option<u64>,
    pub head_hash: Hash,
    pub finalized_slot: Option<u64>,
    pub finalized_hash: Hash,
    pub zone: Option<ZoneId>,
    pub full_node: bool,
}

/// A zone checkpoint: a valid commitment's end state, plus the tip block
/// (its inputs give the "hold last value" inputs to continue from).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Checkpoint {
    pub zone: ZoneId,
    pub beacon_slot: u64,
    pub state: Bytes,
    pub tip: Option<ZoneBlock>,
    /// Newest consumed input per link along the chain up to the tip.
    pub last_inputs: Vec<(InputRef, InputAttachment)>,
}

/// Request-response `/simu/sync/1`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum SyncRequest {
    Status,
    Time,
    BeaconBlocksBySlot { from: u64, count: u32 },
    BeaconBlockByHash(Hash),
    GetCheckpoint { zone: ZoneId, beacon_slot: Option<u64> },
    ZoneBlocks { zone: ZoneId, from: u64, to: u64 },
    ZoneHeaders { zone: ZoneId, from: u64, to: u64 },
    GetOutputs { zone: ZoneId, slot: Option<u64> },
    GetProofs { zone: ZoneId, from_slot: u64, to_slot: u64 },
    GetProposal(Hash),
    GetVotes { zone: ZoneId, proposal_id: Hash },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum SyncResponse {
    Status(StatusInfo),
    Time(u64),
    BeaconBlocks(Vec<BeaconBlock>),
    Checkpoint(Option<Box<Checkpoint>>),
    ZoneBlocks(Vec<ZoneBlock>),
    ZoneHeaders(Vec<SignedHeader>),
    Outputs(Vec<OutputBundle>),
    Proofs(Vec<ProofShare>),
    Proposal(Option<ProposeZoneTx>),
    Votes(Vec<SignedVote>),
    Error(String),
}

/// Builds a `ProposeZone` tx: founders co-sign the proposal id.
pub fn build_propose_tx(
    proposer: NodeId,
    founders: &[&umis_core::SecretKey],
    point: [i32; 3],
    layer: &str,
    task_id: &str,
    params: Bytes,
    slot_ticks: u64,
) -> ProposeZoneTx {
    let ids: Vec<NodeId> = founders.iter().map(|k| k.node_id()).collect();
    let record = ProposeZone {
        px: point[0],
        py: point[1],
        pz: point[2],
        slot_ticks: slot_ticks as u32,
        founder_count: ids.len() as u16,
        layer_id: umis_core::ShortStr::new(layer).expect("layer id ≤ 32 bytes"),
        params_hash: umis_task::params_hash(task_id, layer, &params),
        founders_root: umis_core::bmt::list_root(&ids.iter().map(|n| n.as_hash()).collect::<Vec<_>>()),
        proposer,
    };
    let id = record.address();
    let founder_sigs = founders.iter().map(|k| k.sign(&id)).collect();
    ProposeZoneTx { record, task_id: task_id.into(), params, founders: ids, founder_sigs }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_paths_round_trip() {
        for t in [
            Topic::BeaconBlock,
            Topic::BeaconTx,
            Topic::Commit,
            Topic::ZoneBlock(3),
            Topic::ZoneProof(0),
            Topic::ZoneOut(12),
            Topic::ZoneVote(7),
        ] {
            assert_eq!(Topic::parse(&t.path()), Some(t));
        }
        assert_eq!(Topic::parse("zone/x/out"), None);
    }
}
