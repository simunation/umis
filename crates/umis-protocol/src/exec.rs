//! Deterministic execution of zone blocks and the cheap input checks, shared
//! by zone validators, minters and the beacon state transition (fault
//! evidence re-execution).

use crate::types::{Bytes, InputAttachment, OutputBundle, PortPayload, SignedHeader, ZoneBlock};
use std::collections::BTreeMap;
use umis_core::bmt::{self, SegmentProof};
use umis_core::records::{InputRef, TransitionStatement, ZoneBlockHeader};
use umis_core::{Hash, SegRecord};
use umis_task::{PortId, ZoneInstance};

/// `outputs_root`: list root over the port payload addresses in port order.
pub fn outputs_root(outputs: &BTreeMap<PortId, Bytes>) -> Hash {
    let leaves: Vec<Hash> = outputs.values().map(|p| bmt::file_address(p)).collect();
    bmt::list_root(&leaves)
}

/// Inclusion proof of the payload at `port_index` under `outputs_root`.
pub fn port_proof(outputs: &BTreeMap<PortId, Bytes>, port_index: usize) -> SegmentProof {
    let mut bytes = Vec::with_capacity(outputs.len() * 32);
    for p in outputs.values() {
        bytes.extend_from_slice(&bmt::file_address(p).0);
    }
    SegmentProof::new(&bytes, port_index).expect("port index inside outputs")
}

/// Index of a port in the producer's port order (ports sorted by id).
pub fn port_index(inst: &dyn ZoneInstance, port: PortId) -> Option<usize> {
    let mut ids: Vec<PortId> = inst.ports().iter().map(|p| p.id).collect();
    ids.sort();
    ids.iter().position(|p| *p == port)
}

/// `inputs_commitment`: list root over the InputRef addresses in link order.
pub fn inputs_commitment(inputs: &[InputRef]) -> Hash {
    umis_core::records::list_root_of(inputs)
}

/// Per-slot results of executing a block.
#[derive(Debug, Clone)]
pub struct Exec {
    pub state: Bytes,
    pub outputs: BTreeMap<PortId, Bytes>,
    /// `(slot, state_root after it, outputs_root after it)` for every covered slot.
    pub slots: Vec<(u64, Hash, Hash)>,
}

impl Exec {
    pub fn state_root(&self) -> Hash {
        self.slots.last().map(|s| s.1).unwrap_or_default()
    }
    pub fn outputs_root(&self) -> Hash {
        self.slots.last().map(|s| s.2).unwrap_or_default()
    }
}

/// Input payloads keyed by the consumer's input port (= link id).
pub fn input_map(inputs: &[InputRef], attachments: &[InputAttachment]) -> BTreeMap<PortId, Bytes> {
    inputs.iter().zip(attachments).map(|(r, a)| (r.link_id, a.payload.clone())).collect()
}

/// Runs slots `from_slot ..= slot` from `parent_state` with fixed inputs.
pub fn execute(
    inst: &dyn ZoneInstance,
    parent_state: &[u8],
    inputs: &BTreeMap<PortId, Bytes>,
    from_slot: u64,
    slot: u64,
) -> Result<Exec, String> {
    let mut state = parent_state.to_vec();
    let mut slots = Vec::new();
    let mut outputs = BTreeMap::new();
    for j in from_slot..=slot {
        state = inst.step(&state, inputs, j).map_err(|e| e.to_string())?;
        outputs = inst.outputs(&state).map_err(|e| e.to_string())?;
        slots.push((j, inst.state_root(&state), outputs_root(&outputs)));
    }
    Ok(Exec { state, outputs, slots })
}

/// Re-executes a block and compares the header's claims.
pub fn reexecute_block(inst: &dyn ZoneInstance, parent_state: &[u8], b: &ZoneBlock) -> Result<Exec, String> {
    let h = b.header();
    let ex = execute(inst, parent_state, &input_map(&b.inputs, &b.attachments), h.from_slot, h.slot)?;
    Ok(ex)
}

pub fn matches_header(ex: &Exec, h: &ZoneBlockHeader) -> bool {
    ex.state_root() == h.state_root && ex.outputs_root() == h.outputs_root
}

/// Cheap structural checks of one input attachment against its InputRef
/// (signature, header binding, payload address, inclusion proof). The
/// producer minter's legitimacy needs the producer's anchor and is checked
/// by the caller.
pub fn check_attachment(
    r: &InputRef,
    a: &InputAttachment,
    verify: impl Fn(&SignedHeader) -> bool,
) -> Result<(), String> {
    let h = &a.producer.header;
    if a.producer.hash() != r.src_block_hash {
        return Err("attachment header is not src_block_hash".into());
    }
    if h.zone != r.src_zone || h.slot != r.src_slot || h.outputs_root != r.src_outputs_root {
        return Err("attachment header does not match the InputRef".into());
    }
    if bmt::file_address(&a.payload) != r.payload_address {
        return Err("payload does not hash to payload_address".into());
    }
    if !a.proof.verify(&r.src_outputs_root, &r.payload_address.0, a.port_index as usize) {
        return Err("payload inclusion proof fails".into());
    }
    if !verify(&a.producer) {
        return Err("producer header signature".into());
    }
    Ok(())
}

/// The bundle a producer publishes after minting.
pub fn output_bundle(signed: &SignedHeader, outputs: &BTreeMap<PortId, Bytes>, inputs: &[InputRef]) -> OutputBundle {
    let ports = outputs
        .iter()
        .enumerate()
        .map(|(i, (port, payload))| PortPayload {
            port: *port,
            payload: payload.clone(),
            proof: port_proof(outputs, i),
        })
        .collect();
    OutputBundle { producer: signed.clone(), ports, inputs: inputs.to_vec() }
}

/// The transition statement of one slot `j` of a block, given the state
/// root before it.
pub fn slot_statement(h: &ZoneBlockHeader, prev_root: Hash, slot: (u64, Hash, Hash)) -> TransitionStatement {
    TransitionStatement {
        zone: h.zone,
        from_slot: slot.0,
        to_slot: slot.0 + 1,
        params_hash: h.params_hash,
        prev_state_root: prev_root,
        state_root: slot.1,
        inputs_commitment: h.inputs_commitment,
        outputs_root: slot.2,
    }
}

/// Address of a vote list (`votes_root`).
pub fn votes_root(votes: &[crate::types::SignedVote]) -> Hash {
    let leaves: Vec<Hash> = votes.iter().map(|v| v.vote.address()).collect();
    bmt::list_root(&leaves)
}
