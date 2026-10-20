//! Committed records, all `seg`-encoded. A record's address (bmt chunk
//! address of its encoding) is its identity: the block hash, the
//! `proposal_id`, the leaf in list roots, the message a signer signs.
//!
//! Segment 0 holds the packed scalars; each later segment one 32-byte value.

use crate::crypto::NodeId;
use crate::hash::Hash;
use crate::seg::ShortStr;
use crate::seg_record;

seg_record! {
    /// Zone block header; its address is the block hash. A block computes
    /// zone slots `from_slot ..= slot` (normally `from_slot = slot`; recovery
    /// blocks and blocks after a missed slot cover several).
    pub struct ZoneBlockHeader [tag = 0x01, version = 1] {
        scalars { zone: u32, from_slot: u64, slot: u64, input_count: u16, vote_count: u16 }
        segments {
            parent: Hash,
            anchor_hash: Hash,
            params_hash: Hash,
            prev_state_root: Hash,
            state_root: Hash,
            outputs_root: Hash,
            /// bmt list root over the InputRef addresses in link order.
            inputs_commitment: Hash,
            votes_root: Hash,
            minter: NodeId,
        }
    }
}

seg_record! {
    /// Points at one port payload of one producer block.
    pub struct InputRef [tag = 0x02, version = 1] {
        scalars { link_id: u32, src_zone: u32, src_slot: u64 }
        segments {
            src_block_hash: Hash,
            /// Copy of the producer header's `outputs_root`.
            src_outputs_root: Hash,
            /// bmt file address of the consumed port payload.
            payload_address: Hash,
        }
    }
}

seg_record! {
    /// One proof leaf of a commitment's proof BMT: statement + proof address.
    pub struct ProofLeaf [tag = 0x03, version = 1] {
        scalars { zone: u32, from_slot: u64, to_slot: u64 }
        segments {
            params_hash: Hash,
            prev_state_root: Hash,
            state_root: Hash,
            inputs_commitment: Hash,
            outputs_root: Hash,
            /// bmt file address of the proof bytes.
            proof_address: Hash,
        }
    }
}

seg_record! {
    /// A zone's per-commit-slot commitment to the beacon chain.
    pub struct ZoneCommitment [tag = 0x04, version = 1] {
        scalars { zone: u32, beacon_slot: u64, dep_count: u16, decision_count: u16 }
        segments {
            tip_block_hash: Hash,
            prev_state_root: Hash,
            state_root: Hash,
            outputs_root: Hash,
            /// bmt list root over the InputRef leaves of the covered slots,
            /// in (zone slot, link) order.
            inputs_root: Hash,
            deps_root: Hash,
            decisions_root: Hash,
            signer: NodeId,
        }
    }
}

seg_record! {
    /// The newest producer block a commitment consumed on one link.
    pub struct Dep [tag = 0x05, version = 1] {
        scalars { link_id: u32, src_zone: u32, src_slot: u64 }
        segments { src_block_hash: Hash }
    }
}

seg_record! {
    pub struct ProofCommitment [tag = 0x06, version = 1] {
        scalars { zone: u32, beacon_slot: u64, leaf_count: u32 }
        segments { proof_bmt_root: Hash, signer: NodeId }
    }
}

seg_record! {
    pub struct ZoneVote [tag = 0x07, version = 1] {
        scalars { zone: u32, accept: bool }
        segments { proposal_id: Hash, voter: NodeId }
    }
}

seg_record! {
    pub struct ZoneDecision [tag = 0x08, version = 1] {
        scalars { accept: bool, yes: u16, no: u16 }
        segments {
            proposal_id: Hash,
            /// bmt list root over the counted vote addresses in voter order.
            votes_root: Hash,
        }
    }
}

seg_record! {
    /// Proposal for a new zone at a location; its address is the `proposal_id`.
    pub struct ProposeZone [tag = 0x09, version = 1] {
        scalars { px: i32, py: i32, pz: i32, slot_ticks: u32, founder_count: u16 }
        segments {
            layer_id: ShortStr,
            params_hash: Hash,
            /// bmt list root over the founders' node ids.
            founders_root: Hash,
            proposer: NodeId,
        }
    }
}

seg_record! {
    /// Beacon block header; its address is the beacon block hash.
    pub struct BeaconBlockHeader [tag = 0x0a, version = 1] {
        scalars { slot: u64, tx_count: u32, zc_count: u16, pc_count: u16 }
        segments {
            parent: Hash,
            seed: Hash,
            proposer: NodeId,
            txs_root: Hash,
            zones_root: Hash,
            proofs_root: Hash,
            state_root: Hash,
        }
    }
}

seg_record! {
    /// `{task_id, layer, params}`; its address is a zone's `params_hash`.
    pub struct ZoneParams [tag = 0x0b, version = 1] {
        scalars { params_len: u32 }
        segments {
            task_id: ShortStr,
            layer: ShortStr,
            /// bmt file address of the task's borsh-encoded params.
            params_root: Hash,
        }
    }
}

seg_record! {
    /// What a transition proof proves: zone slots `[from_slot, to_slot)`.
    pub struct TransitionStatement [tag = 0x0c, version = 1] {
        scalars { zone: u32, from_slot: u64, to_slot: u64 }
        segments {
            params_hash: Hash,
            prev_state_root: Hash,
            state_root: Hash,
            inputs_commitment: Hash,
            outputs_root: Hash,
        }
    }
}

seg_record! {
    /// The message a beacon proposer signs for the slot seed: "seed" ‖ e.
    pub struct SeedMsg [tag = 0x0d, version = 1] {
        scalars { slot: u64 }
        segments {}
    }
}

seg_record! {
    /// Membership: lock `stake` and join as a zone member (kind 0) or a native
    /// full node (kind 1).
    pub struct Join [tag = 0x10, version = 1] {
        scalars { stake: u64, kind: u8, nonce: u64 }
        segments { node: NodeId }
    }
}

seg_record! {
    pub struct Leave [tag = 0x11, version = 1] {
        scalars { nonce: u64 }
        segments { node: NodeId }
    }
}

seg_record! {
    pub struct Heartbeat [tag = 0x12, version = 1] {
        scalars { beacon_slot: u64 }
        segments { node: NodeId }
    }
}

seg_record! {
    /// Devnet faucet credit, signed by the genesis faucet key.
    pub struct Grant [tag = 0x13, version = 1] {
        scalars { amount: u64, nonce: u64 }
        segments { to: NodeId }
    }
}

seg_record! {
    /// Fault evidence header; the evidence body is committed by `evidence_root`.
    pub struct FaultEvidence [tag = 0x14, version = 1] {
        scalars { kind: u8, zone: u32, zone_slot: u64 }
        segments {
            minter: NodeId,
            reporter: NodeId,
            /// bmt file address of the borsh-encoded evidence body.
            evidence_root: Hash,
        }
    }
}

/// `ZoneVote` addresses as a list root.
pub fn list_root_of<R: crate::seg::SegRecord>(items: &[R]) -> Hash {
    let leaves: Vec<Hash> = items.iter().map(|r| r.address()).collect();
    crate::bmt::list_root(&leaves)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seg::SegRecord;

    #[test]
    fn record_sizes_match_the_plan() {
        assert_eq!(ZoneBlockHeader::SEGMENTS, 10);
        assert_eq!(InputRef::SEGMENTS, 4);
        assert_eq!(ProofLeaf::SEGMENTS, 7);
        assert_eq!(ZoneCommitment::SEGMENTS, 9);
        assert_eq!(Dep::SEGMENTS, 2);
        assert_eq!(ProofCommitment::SEGMENTS, 3);
        assert_eq!(ZoneVote::SEGMENTS, 3);
        assert_eq!(ZoneDecision::SEGMENTS, 3);
        assert_eq!(ProposeZone::SEGMENTS, 5);
        assert_eq!(BeaconBlockHeader::SEGMENTS, 8);
    }

    #[test]
    fn plan_example_state_root_is_segment_3() {
        // "prove zone z's state_root ... that zone's ZoneCommitment address → its segment 3"
        let l = ZoneCommitment::layout();
        let sr = l.iter().find(|f| f.name == "state_root").unwrap();
        assert_eq!(sr.segment, 3);
    }
}
