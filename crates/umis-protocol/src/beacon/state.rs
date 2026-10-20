//! The beacon state and its deterministic transition `apply(parent, block)`.
//!
//! The state holds the zone registry (keyed by location), members and stake,
//! recent zone commitments with their status (optimistic / proven / settled
//! / invalid) and the data needed for cross-link checks, fault keys,
//! rollback records, and zone-creation proposals.
//!
//! Order of a block's effects: housekeeping (missed proof deadlines,
//! expired proposals, unbonding, heartbeat removals), then txs, zone
//! commitments, proof commitments, and finally cross-link resolution and
//! settlement.

use crate::ctx::Ctx;
use crate::exec;
use crate::ranks;
use crate::types::*;
use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use umis_core::bmt;
use umis_core::genesis::ValidatorKind;
use umis_core::records::*;
use umis_core::{Hash, NodeId, SegRecord, keccak256_concat};
use umis_task::topology::{LayerId, Location, Point3, links_between};

/// Why something was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Reject(pub String);

pub type VResult<T> = Result<T, Reject>;

macro_rules! reject {
    ($($t:tt)*) => { return Err(Reject(format!($($t)*))) };
}
macro_rules! ensure {
    ($c:expr, $($t:tt)*) => { if !$c { reject!($($t)*); } };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub enum NodeKind {
    Member,
    FullNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub enum NodeStatus {
    Active,
    /// Left or removed; stake stays locked (and slashable) until `until`.
    Unbonding {
        until: u64,
    },
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct NodeEntry {
    pub kind: NodeKind,
    pub zone: Option<ZoneId>,
    pub stake: u64,
    pub status: NodeStatus,
    pub joined_at: u64,
    pub last_heartbeat: u64,
    pub genesis: bool,
}

/// An input link of a consumer zone: its input port `link_id` is fed by
/// `from_zone`'s output port `from_port` from beacon slot `active_from` on.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct LinkIn {
    pub link_id: u32,
    pub from_zone: ZoneId,
    pub from_port: u32,
    pub active_from: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct ZoneEntry {
    pub id: ZoneId,
    pub point: Point3,
    pub layer: String,
    pub task_id: String,
    pub params: Bytes,
    pub params_hash: Hash,
    pub slot_ticks: u64,
    /// First beacon slot of the zone.
    pub active_from: u64,
    /// First zone slot: `active_from · B / S`.
    pub j_start: u64,
    pub start_root: Hash,
    /// Virtual parent of the zone's first block.
    pub start_tip: Hash,
    /// Input links sorted by `link_id`.
    pub inputs: Vec<LinkIn>,
}

impl ZoneEntry {
    pub fn location(&self) -> (Point3, String) {
        (self.point, self.layer.clone())
    }
}

/// Virtual tip hash of a zone before its first block.
pub fn start_tip(zone: ZoneId, start_root: &Hash) -> Hash {
    keccak256_concat(&[b"zone-start", &zone.to_be_bytes(), &start_root.0])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub enum CommitStatus {
    Optimistic,
    Proven,
    Settled,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct BlockSummary {
    pub from_slot: u64,
    pub slot: u64,
    pub hash: Hash,
    pub state_root: Hash,
    pub outputs_root: Hash,
    pub inputs_commitment: Hash,
    pub minter: NodeId,
    /// This block's InputRefs are `CommitInfo::inputs[input_start .. input_start + input_count]`.
    pub input_start: u32,
    pub input_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct CommitInfo {
    /// Commit slot `e`.
    pub e: u64,
    pub included_at: u64,
    pub address: Hash,
    pub signer: NodeId,
    /// Covered zone slots `[from_slot, to_slot)`.
    pub from_slot: u64,
    pub to_slot: u64,
    pub prev_state_root: Hash,
    pub state_root: Hash,
    pub outputs_root: Hash,
    pub tip: Hash,
    pub status: CommitStatus,
    pub deps: Vec<Dep>,
    pub blocks: Vec<BlockSummary>,
    /// Every covered InputRef (dropped once settled).
    pub inputs: Vec<InputRef>,
    /// Newest consumed input per link up to the tip.
    pub last_inputs: BTreeMap<u32, InputRef>,
    pub unresolved: u32,
    pub proof_bmt_root: Option<Hash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub enum RollbackReason {
    Fault,
    CrossLink,
    MissingProof,
    /// Not affected itself: drops its uncommitted tail so every zone
    /// recovers in the same beacon slot (no cascades along input links).
    Synchronized,
}

/// Zone `zone` rolls back to the checkpoint and recomputes from `j_r`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct RollbackRecord {
    pub zone: ZoneId,
    /// Commit slot of the checkpoint commitment (`None`: the zone start).
    pub checkpoint_e: Option<u64>,
    pub j_r: u64,
    pub checkpoint_root: Hash,
    pub checkpoint_tip: Hash,
    pub e_record: u64,
    pub reason: RollbackReason,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct SlashRecord {
    pub e: u64,
    pub minter: NodeId,
    pub zone: ZoneId,
    pub zone_slot: u64,
    pub kind: u8,
    pub reporter: NodeId,
    pub slashed: u64,
    pub reward: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub enum ProposalStatus {
    Voting,
    Accepted { e_dec: u64, e_act: u64, zone: ZoneId },
    Rejected { e: u64 },
    Expired { e: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize, Default)]
pub struct Tally {
    pub yes: BTreeSet<NodeId>,
    pub no: BTreeSet<NodeId>,
    /// Counted votes in counting order.
    pub votes: Vec<ZoneVote>,
    pub decided: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct Proposal {
    pub id: Hash,
    pub point: Point3,
    pub layer: String,
    pub task_id: String,
    pub params: Bytes,
    pub params_hash: Hash,
    pub slot_ticks: u64,
    pub proposer: NodeId,
    pub founders: Vec<NodeId>,
    /// Adjacent zones `A`.
    pub adjacent: Vec<ZoneId>,
    /// `voters_a` snapshots.
    pub voters: BTreeMap<ZoneId, Vec<NodeId>>,
    pub e_reg: u64,
    pub deadline: u64,
    pub tally: BTreeMap<ZoneId, Tally>,
    pub status: ProposalStatus,
}

impl Proposal {
    pub fn is_pending(&self) -> bool {
        self.status == ProposalStatus::Voting
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize, serde::Serialize)]
pub struct BeaconState {
    /// Slot of the block that produced this state (`None`: genesis).
    pub slot: Option<u64>,
    pub seed: Hash,
    pub nodes: BTreeMap<NodeId, NodeEntry>,
    pub balances: BTreeMap<NodeId, u64>,
    pub zones: BTreeMap<ZoneId, ZoneEntry>,
    pub commits: BTreeMap<ZoneId, Vec<CommitInfo>>,
    /// Latest rollback record per zone.
    pub rollback: BTreeMap<ZoneId, RollbackRecord>,
    pub rollback_log: Vec<RollbackRecord>,
    /// Fault keys `(minter, zone, zone slot)` → beacon slot recorded.
    pub punished: BTreeMap<(NodeId, ZoneId, u64), u64>,
    pub slash_log: Vec<SlashRecord>,
    /// Zone blocks shown faulty (their hashes).
    pub discarded: BTreeMap<Hash, u64>,
    pub proposals: BTreeMap<Hash, Proposal>,
    pub next_zone_id: ZoneId,
    pub grant_nonces: BTreeSet<(NodeId, u64)>,
    pub burned: u64,
    /// Zones invalidated while applying the current block (always empty
    /// between blocks).
    pub invalidated_now: BTreeMap<ZoneId, RollbackReason>,
}

/// Beacon blocks of the branch being applied, for anchors.
pub trait ChainView {
    /// The anchor of beacon slot `e` on this branch: the newest block with
    /// slot ≤ `e − D` (or genesis), as `(hash, state)`.
    fn anchor(&self, e: u64) -> Option<(Hash, Arc<BeaconState>)>;
}

/// Ensures a zone header's anchor and minter legitimacy.
fn check_minter(ctx: &Ctx, view: &dyn ChainView, h: &ZoneBlockHeader, slot_ticks: u64) -> VResult<u32> {
    let e = ctx.timing(slot_ticks).beacon_slot_of(h.slot);
    let Some((ahash, a)) = view.anchor(e) else { reject!("no anchor for beacon slot {e}") };
    ensure!(h.anchor_hash == ahash, "zone {} slot {}: wrong anchor", h.zone, h.slot);
    let list = ranks::zone_ranks(&a, h.zone, h.slot);
    let Some(r) = ranks::rank_of(&list, &h.minter) else { reject!("minter not in zone {} ranks", h.zone) };
    ensure!(r <= ctx.p.max_zone_rank, "minter rank {r} above R");
    Ok(r)
}

/// Producer legitimacy: the header is signed by an elected minter of the
/// producer zone for its slot, under the producer's own anchor.
pub fn check_producer(ctx: &Ctx, view: &dyn ChainView, state: &BeaconState, s: &SignedHeader) -> VResult<()> {
    let h = &s.header;
    let Some(pz) = state.zones.get(&h.zone) else { reject!("unknown producer zone {}", h.zone) };
    check_minter(ctx, view, h, pz.slot_ticks)?;
    ensure!(ctx.verify(&h.minter, &s.hash(), &s.sig), "producer signature");
    Ok(())
}

impl BeaconState {
    pub fn genesis(ctx: &Ctx) -> Result<Self, String> {
        let g = &ctx.genesis;
        let mut s = BeaconState {
            slot: None,
            seed: g.initial_beacon_seed(),
            nodes: BTreeMap::new(),
            balances: BTreeMap::new(),
            zones: BTreeMap::new(),
            commits: BTreeMap::new(),
            rollback: BTreeMap::new(),
            rollback_log: Vec::new(),
            punished: BTreeMap::new(),
            slash_log: Vec::new(),
            discarded: BTreeMap::new(),
            proposals: BTreeMap::new(),
            next_zone_id: g.zones.len() as ZoneId,
            grant_nonces: BTreeSet::new(),
            burned: 0,
            invalidated_now: BTreeMap::new(),
        };
        for z in &g.zones {
            let task = ctx.tasks.get(&z.task_id).map_err(|e| e.to_string())?;
            let params = task.params_from_json(&z.params).map_err(|e| e.to_string())?;
            let params_hash = umis_task::params_hash(&z.task_id, &z.layer, &params);
            let inst = ctx.instance(&z.task_id, &params, &params_hash)?;
            let start_root = inst.state_root(&inst.initial_state(0));
            s.zones.insert(
                z.id,
                ZoneEntry {
                    id: z.id,
                    point: z.point,
                    layer: z.layer.clone(),
                    task_id: z.task_id.clone(),
                    params,
                    params_hash,
                    slot_ticks: z.slot_ticks,
                    active_from: 0,
                    j_start: 0,
                    start_root,
                    start_tip: start_tip(z.id, &start_root),
                    inputs: Vec::new(),
                },
            );
            s.commits.insert(z.id, Vec::new());
        }
        let ids: Vec<ZoneId> = s.zones.keys().copied().collect();
        for a in &ids {
            for b in &ids {
                s.wire(ctx, *a, *b, 0);
            }
        }
        for v in &g.validators {
            s.nodes.insert(
                v.node,
                NodeEntry {
                    kind: match v.kind {
                        ValidatorKind::Member => NodeKind::Member,
                        ValidatorKind::FullNode => NodeKind::FullNode,
                    },
                    zone: if v.kind == ValidatorKind::Member { v.zone } else { None },
                    stake: v.stake,
                    status: NodeStatus::Active,
                    joined_at: 0,
                    last_heartbeat: 0,
                    genesis: true,
                },
            );
        }
        Ok(s)
    }

    /// Adds the links `a → b` given by the topology (consumer `b`).
    fn wire(&mut self, ctx: &Ctx, a: ZoneId, b: ZoneId, active_from: u64) {
        if a == b {
            return;
        }
        let (za, zb) = (&self.zones[&a], &self.zones[&b]);
        let la = Location { point: za.point, layer: LayerId::new(&za.layer) };
        let lb = Location { point: zb.point, layer: LayerId::new(&zb.layer) };
        let links = links_between(&ctx.topo, &ctx.couplings, &la, &lb);
        let zb = self.zones.get_mut(&b).expect("zone");
        for l in links {
            if zb.inputs.iter().all(|x| x.link_id != l.to_port) {
                zb.inputs.push(LinkIn { link_id: l.to_port, from_zone: a, from_port: l.from_port, active_from });
            }
        }
        zb.inputs.sort_by_key(|l| l.link_id);
    }

    pub fn hash(&self) -> Hash {
        bmt::file_address(&borsh::to_vec(self).expect("borsh state"))
    }

    // ------------------------------------------------------------ queries

    pub fn is_active(&self, n: &NodeId) -> bool {
        self.nodes.get(n).is_some_and(|e| e.status == NodeStatus::Active)
    }

    /// Active members of zone `z`, sorted.
    pub fn members_of(&self, z: ZoneId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, e)| e.status == NodeStatus::Active && e.kind == NodeKind::Member && e.zone == Some(z))
            .map(|(n, _)| *n)
            .collect()
    }

    pub fn full_nodes(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, e)| e.status == NodeStatus::Active && e.kind == NodeKind::FullNode)
            .map(|(n, _)| *n)
            .collect()
    }

    pub fn active_nodes(&self) -> Vec<NodeId> {
        self.nodes.iter().filter(|(_, e)| e.status == NodeStatus::Active).map(|(n, _)| *n).collect()
    }

    /// May sign for zone `z` (member or full node).
    pub fn can_act_for(&self, z: ZoneId, n: &NodeId) -> bool {
        self.nodes
            .get(n)
            .is_some_and(|e| e.status == NodeStatus::Active && (e.kind == NodeKind::FullNode || e.zone == Some(z)))
    }

    /// Zones active at beacon slot `e`.
    pub fn active_zones(&self, e: u64) -> Vec<ZoneId> {
        self.zones.values().filter(|z| z.active_from <= e).map(|z| z.id).collect()
    }

    pub fn zone_at(&self, point: &Point3, layer: &str) -> Option<ZoneId> {
        self.zones.values().find(|z| z.point == *point && z.layer == layer).map(|z| z.id)
    }

    /// Input links of zone `z` active at beacon slot `e`.
    pub fn links_at(&self, z: ZoneId, e: u64) -> Vec<LinkIn> {
        self.zones
            .get(&z)
            .map(|zz| zz.inputs.iter().filter(|l| l.active_from <= e).cloned().collect())
            .unwrap_or_default()
    }

    /// Latest non-invalid commitment of `z`.
    pub fn last_valid(&self, z: ZoneId) -> Option<&CommitInfo> {
        self.commits.get(&z)?.iter().rev().find(|c| c.status != CommitStatus::Invalid)
    }

    /// `(boundary slot, state root, tip, last inputs)` the next commitment of
    /// `z` continues from (all zero for a zone this state does not know yet,
    /// e.g. an anchor older than the zone's creation).
    pub fn commit_base(&self, z: ZoneId) -> (u64, Hash, Hash, BTreeMap<u32, InputRef>, Option<u64>) {
        let Some(zz) = self.zones.get(&z) else { return (0, Hash::ZERO, Hash::ZERO, BTreeMap::new(), None) };
        match self.last_valid(z) {
            Some(c) => (c.to_slot, c.state_root, c.tip, c.last_inputs.clone(), Some(c.e)),
            None => (zz.j_start, zz.start_root, zz.start_tip, BTreeMap::new(), None),
        }
    }

    /// What the beacon chain says about zone `z`'s block at `slot`: `None` if
    /// no valid commitment covers the slot yet, `Some(h)` with the committed
    /// block ending at that slot (`Some(None)`: covered, but no block ends there).
    pub fn committed_block_at(&self, z: ZoneId, slot: u64) -> Option<Option<Hash>> {
        let c = self
            .commits
            .get(&z)?
            .iter()
            .rev()
            .find(|c| c.status != CommitStatus::Invalid && c.from_slot <= slot && slot < c.to_slot)?;
        Some(c.blocks.iter().find(|b| b.slot == slot).map(|b| b.hash))
    }

    pub fn deps_of(&self, z: ZoneId) -> Vec<Dep> {
        self.last_valid(z).map(|c| c.deps.clone()).unwrap_or_default()
    }

    pub fn pending_proposals(&self) -> impl Iterator<Item = &Proposal> {
        self.proposals.values().filter(|p| p.is_pending())
    }

    fn zone_member_count(&self, z: ZoneId) -> usize {
        self.members_of(z).len()
    }

    // ------------------------------------------------------------ transition

    /// Applies a block body for slot `e` (header fields `proposer`, `seed`
    /// given). Returns the new state; callers check the header roots.
    pub fn apply_body(
        &self,
        ctx: &Ctx,
        view: &dyn ChainView,
        e: u64,
        seed: Hash,
        body: &BeaconBody,
    ) -> VResult<BeaconState> {
        if let Some(ps) = self.slot {
            ensure!(e > ps, "slot {e} not after parent slot {ps}");
        }
        let mut s = self.clone();
        s.slot = Some(e);
        s.seed = seed;
        s.housekeeping(ctx, e);
        for t in &body.txs {
            s.apply_tx(ctx, view, e, t)?;
        }
        let mut last_key = None;
        for zc in &body.zone_commitments {
            let key = (zc.commitment.zone, zc.commitment.beacon_slot);
            ensure!(last_key.is_none_or(|k| k < key), "zone commitments must be sorted by (zone, slot)");
            last_key = Some(key);
            s.apply_zone_commitment(ctx, view, e, zc)?;
        }
        let mut last_key = None;
        for pc in &body.proof_commitments {
            let key = (pc.pc.zone, pc.pc.beacon_slot);
            ensure!(last_key.is_none_or(|k| k < key), "proof commitments must be sorted by (zone, slot)");
            last_key = Some(key);
            s.apply_proof_commitment(ctx, view, e, pc)?;
        }
        s.resolve(ctx, e);
        if !s.invalidated_now.is_empty() {
            s.global_rollback(e);
        }
        s.settle(ctx, e);
        s.prune(ctx, e);
        Ok(s)
    }

    /// Full block validation against this (parent) state.
    pub fn apply_block(
        &self,
        ctx: &Ctx,
        view: &dyn ChainView,
        parent_hash: Hash,
        b: &BeaconBlock,
    ) -> VResult<BeaconState> {
        let h = &b.header;
        let e = h.slot;
        ensure!(h.parent == parent_hash, "parent mismatch");
        let Some((_, anchor)) = view.anchor(e) else { reject!("no anchor for beacon slot {e}") };
        let list = ranks::beacon_ranks(&anchor, e);
        let Some(rank) = ranks::rank_of(&list, &h.proposer) else { reject!("proposer not registered") };
        ensure!(rank <= ctx.p.max_beacon_rank, "proposer rank {rank} above R_b");
        ensure!(ctx.verify(&h.proposer, &b.hash(), &b.sig), "block signature");
        ensure!(ctx.verify(&h.proposer, &SeedMsg { slot: e }.address(), &b.seed_sig), "seed signature");
        let seed = keccak256_concat(&[&self.seed.0, &b.seed_sig.0]);
        ensure!(h.seed == seed, "seed formula");
        ensure!(h.tx_count as usize == b.body.txs.len(), "tx_count");
        ensure!(h.zc_count as usize == b.body.zone_commitments.len(), "zc_count");
        ensure!(h.pc_count as usize == b.body.proof_commitments.len(), "pc_count");
        let (txs_root, zones_root, proofs_root) = body_roots(&b.body);
        ensure!(h.txs_root == txs_root && h.zones_root == zones_root && h.proofs_root == proofs_root, "body roots");
        let s = self.apply_body(ctx, view, e, seed, &b.body)?;
        ensure!(s.hash() == h.state_root, "state_root mismatch");
        Ok(s)
    }

    fn housekeeping(&mut self, ctx: &Ctx, e: u64) {
        let p = &ctx.p;
        // Missed proof deadlines: optimistic commitments included more than L slots ago.
        let zones: Vec<ZoneId> = self.commits.keys().copied().collect();
        for z in zones {
            let missed = self.commits[&z]
                .iter()
                .find(|c| c.status == CommitStatus::Optimistic && c.included_at + p.proof_lag < e)
                .map(|c| c.e);
            if let Some(ce) = missed {
                self.invalidate_from(z, ce, e, RollbackReason::MissingProof);
            }
        }
        // Proposals past their deadline.
        for prop in self.proposals.values_mut() {
            if prop.is_pending() && e > prop.deadline {
                prop.status = ProposalStatus::Expired { e };
            }
        }
        // Unbonding completion.
        for (n, entry) in self.nodes.iter_mut() {
            if let NodeStatus::Unbonding { until } = entry.status
                && until <= e
            {
                *self.balances.entry(*n).or_default() += entry.stake;
                entry.stake = 0;
                entry.status = NodeStatus::Removed;
            }
        }
        // Heartbeat timeouts (never genesis validators or a zone's last member).
        let stale: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|(_, n)| n.status == NodeStatus::Active && !n.genesis && n.last_heartbeat + p.heartbeat_timeout < e)
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            let zone = self.nodes[&id].zone;
            if let Some(z) = zone
                && self.zone_member_count(z) <= 1
            {
                continue;
            }
            let n = self.nodes.get_mut(&id).expect("node");
            n.status = NodeStatus::Unbonding { until: e + p.unbonding };
            n.zone = None;
        }
    }

    // ------------------------------------------------------------ txs

    fn apply_tx(&mut self, ctx: &Ctx, view: &dyn ChainView, e: u64, t: &SignedTx) -> VResult<()> {
        let signer = t.tx.signer(&ctx.genesis.faucet);
        ensure!(ctx.verify(&signer, &t.tx.address(), &t.sig), "{} tx signature", t.tx.kind_name());
        match &t.tx {
            Tx::Join(j) => self.apply_join(ctx, e, j),
            Tx::Leave(l) => {
                ensure!(self.is_active(&l.node), "leave: node not active");
                let n = self.nodes.get_mut(&l.node).expect("node");
                n.status = NodeStatus::Unbonding { until: e + ctx.p.unbonding };
                n.zone = None;
                Ok(())
            }
            Tx::Heartbeat(h) => {
                ensure!(self.is_active(&h.node), "heartbeat: node not active");
                ensure!(h.beacon_slot <= e && h.beacon_slot + 2 >= e, "stale heartbeat");
                let n = self.nodes.get_mut(&h.node).expect("node");
                n.last_heartbeat = n.last_heartbeat.max(h.beacon_slot);
                Ok(())
            }
            Tx::Grant(g) => {
                ensure!(self.grant_nonces.insert((g.to, g.nonce)), "grant nonce reused");
                *self.balances.entry(g.to).or_default() += g.amount;
                Ok(())
            }
            Tx::ProposeZone(p) => self.apply_propose(ctx, e, p),
            Tx::Fault(f) => self.apply_fault(ctx, view, e, f),
        }
    }

    fn apply_join(&mut self, ctx: &Ctx, e: u64, j: &Join) -> VResult<()> {
        ensure!(!self.is_active(&j.node), "join: already active");
        if let Some(n) = self.nodes.get(&j.node) {
            ensure!(n.status == NodeStatus::Removed, "join: still unbonding");
        }
        ensure!(j.stake >= ctx.p.min_stake, "join: stake below min_stake");
        let bal = self.balances.get(&j.node).copied().unwrap_or(0);
        ensure!(bal >= j.stake, "join: balance {bal} < stake {}", j.stake);
        let kind = match j.kind {
            0 => NodeKind::Member,
            1 => NodeKind::FullNode,
            _ => reject!("join: unknown kind"),
        };
        let zone = if kind == NodeKind::Member { Some(self.assign_zone(e, &j.node)?) } else { None };
        *self.balances.get_mut(&j.node).expect("balance") -= j.stake;
        self.nodes.insert(
            j.node,
            NodeEntry {
                kind,
                zone,
                stake: j.stake,
                status: NodeStatus::Active,
                joined_at: e,
                last_heartbeat: e,
                genesis: false,
            },
        );
        Ok(())
    }

    /// Empty zones first, then the least-populated zone, ties by hash.
    fn assign_zone(&self, e: u64, n: &NodeId) -> VResult<ZoneId> {
        let zones = self.active_zones(e);
        ensure!(!zones.is_empty(), "no active zone");
        let best = zones
            .into_iter()
            .min_by_key(|z| (self.zone_member_count(*z), keccak256_concat(&[&n.0, &z.to_be_bytes()])))
            .expect("non-empty");
        Ok(best)
    }

    // ------------------------------------------------------------ zone commitments

    fn apply_zone_commitment(
        &mut self,
        ctx: &Ctx,
        view: &dyn ChainView,
        e: u64,
        zc: &ZoneCommitEnvelope,
    ) -> VResult<()> {
        let c = &zc.commitment;
        let z = c.zone;
        let Some(zone) = self.zones.get(&z).cloned() else { reject!("commitment for unknown zone {z}") };
        let t = ctx.timing(zone.slot_ticks);
        ensure!(zone.active_from <= c.beacon_slot, "zone {z} not active at {}", c.beacon_slot);
        ensure!(t.is_commit_slot(c.beacon_slot), "{} is not a commit slot of zone {z}", c.beacon_slot);
        ensure!(c.beacon_slot < e, "commitment for slot {} in block {e}", c.beacon_slot);
        ensure!(self.can_act_for(z, &c.signer), "commitment signer not a member/full node");
        ensure!(ctx.verify(&c.signer, &zc.address(), &zc.sig), "commitment signature");
        let (base_slot, base_root, base_tip, mut last_inputs, base_e) = self.commit_base(z);
        if let Some(be) = base_e {
            ensure!(c.beacon_slot > be, "zone {z}: commitment for {} not after {}", c.beacon_slot, be);
        }
        let to_slot = t.j_star(c.beacon_slot);
        ensure!(to_slot > base_slot, "zone {z}: empty commitment range");
        ensure!(c.prev_state_root == base_root, "zone {z}: prev_state_root does not link");
        ensure!(!zc.blocks.is_empty(), "no blocks");

        // The covered chain.
        let mut parent = base_tip;
        let mut prev_root = base_root;
        let mut next_from = base_slot;
        let mut all_inputs = Vec::new();
        let mut summaries = Vec::new();
        let mut votes_by_block: Vec<(u64, &[SignedVote])> = Vec::new();
        for b in &zc.blocks {
            let h = &b.signed.header;
            ensure!(h.zone == z, "block of another zone");
            ensure!(h.parent == parent, "zone {z} slot {}: parent does not link", h.slot);
            ensure!(h.from_slot == next_from && h.slot >= h.from_slot, "zone {z}: slots not contiguous");
            ensure!(h.prev_state_root == prev_root, "zone {z} slot {}: prev_state_root", h.slot);
            ensure!(h.params_hash == zone.params_hash, "params_hash");
            ensure!(!self.discarded.contains_key(&b.signed.hash()), "zone {z}: commitment contains a faulty block");
            check_minter(ctx, view, h, zone.slot_ticks)?;
            if let Some(r) = self.rollback.get(&z)
                && h.slot >= r.j_r
            {
                let rollback_e = view
                    .anchor(t.beacon_slot_of(h.slot))
                    .and_then(|(_, a)| a.rollback.get(&z).map(|x| x.e_record))
                    .unwrap_or(0);
                ensure!(rollback_e >= r.e_record, "zone {z} slot {}: minted before the rollback took effect", h.slot);
            }
            ensure!(ctx.verify(&h.minter, &b.signed.hash(), &b.signed.sig), "zone block signature");
            ensure!(h.input_count as usize == b.inputs.len(), "input_count");
            ensure!(exec::inputs_commitment(&b.inputs) == h.inputs_commitment, "inputs_commitment");
            ensure!(h.vote_count as usize == b.votes.len(), "vote_count");
            ensure!(exec::votes_root(&b.votes) == h.votes_root, "votes_root");
            let links = self.links_at(z, t.beacon_slot_of(h.slot));
            let mut prev_link = None;
            for r in &b.inputs {
                ensure!(prev_link.is_none_or(|l| l < r.link_id), "inputs not in link order");
                prev_link = Some(r.link_id);
                let Some(l) = links.iter().find(|l| l.link_id == r.link_id) else {
                    reject!("unknown link {}", r.link_id)
                };
                ensure!(l.from_zone == r.src_zone, "link {} source zone", r.link_id);
                let Some(pz) = self.zones.get(&r.src_zone) else { reject!("unknown producer") };
                ensure!(
                    (r.src_slot + 1) * pz.slot_ticks <= h.from_slot * zone.slot_ticks,
                    "zone {z} slot {}: non-causal input from zone {} slot {}",
                    h.slot,
                    r.src_zone,
                    r.src_slot
                );
                if let Some(prev) = last_inputs.get(&r.link_id) {
                    ensure!(r.src_slot >= prev.src_slot, "non-monotonic input on link {}", r.link_id);
                }
                last_inputs.insert(r.link_id, r.clone());
            }
            all_inputs.extend(b.inputs.iter().cloned());
            votes_by_block.push((h.slot, &b.votes));
            summaries.push(BlockSummary {
                from_slot: h.from_slot,
                slot: h.slot,
                hash: b.signed.hash(),
                state_root: h.state_root,
                outputs_root: h.outputs_root,
                inputs_commitment: h.inputs_commitment,
                minter: h.minter,
                input_start: (all_inputs.len() - b.inputs.len()) as u32,
                input_count: b.inputs.len() as u32,
            });
            parent = b.signed.hash();
            prev_root = h.state_root;
            next_from = h.slot + 1;
        }
        ensure!(next_from == to_slot, "zone {z}: chain ends at {next_from}, commitment boundary is {to_slot}");
        ensure!(c.tip_block_hash == parent, "tip_block_hash");
        ensure!(c.state_root == prev_root, "state_root");
        let tip_outputs = summaries.last().expect("blocks").outputs_root;
        ensure!(c.outputs_root == tip_outputs, "outputs_root");
        ensure!(c.inputs_root == umis_core::records::list_root_of(&all_inputs), "inputs_root");
        // deps: newest consumed producer block per link.
        let deps: Vec<Dep> = last_inputs
            .values()
            .map(|r| Dep {
                link_id: r.link_id,
                src_zone: r.src_zone,
                src_slot: r.src_slot,
                src_block_hash: r.src_block_hash,
            })
            .collect();
        ensure!(zc.deps == deps, "deps");
        ensure!(
            c.dep_count as usize == deps.len() && c.deps_root == umis_core::records::list_root_of(&deps),
            "deps_root"
        );
        // Votes: valid, first per voter, and the decisions they produce.
        let decisions = self.tally_votes(ctx, view, z, &zone, &votes_by_block)?;
        ensure!(zc.decisions == decisions, "zone {z}: decisions do not match the recount");
        ensure!(
            c.decision_count as usize == decisions.len()
                && c.decisions_root == umis_core::records::list_root_of(&decisions),
            "decisions_root"
        );

        let info = CommitInfo {
            e: c.beacon_slot,
            included_at: e,
            address: zc.address(),
            signer: c.signer,
            from_slot: base_slot,
            to_slot,
            prev_state_root: base_root,
            state_root: c.state_root,
            outputs_root: c.outputs_root,
            tip: parent,
            status: CommitStatus::Optimistic,
            deps,
            blocks: summaries,
            inputs: all_inputs,
            last_inputs,
            unresolved: 0,
            proof_bmt_root: None,
        };
        self.commits.get_mut(&z).expect("zone commits").push(info);
        self.apply_decisions(ctx, e, z, &decisions);
        Ok(())
    }

    /// This state with pending zone commitments applied (those that apply),
    /// as the next beacon block would see them. Delegates build on it so a
    /// commitment for `e` can be made before the one for `e − 1` is included.
    pub fn with_pending(
        &self,
        ctx: &Ctx,
        view: &dyn ChainView,
        e_next: u64,
        pending: &[(&ZoneCommitEnvelope, Option<&ProofCommitEnvelope>)],
    ) -> BeaconState {
        let mut s = self.clone();
        for (zc, pc) in pending {
            // With L = 0 a commitment is only includable together with its proofs.
            if ctx.p.proof_lag == 0 && pc.is_none() {
                continue;
            }
            let mut trial = s.clone();
            let ok = trial.apply_zone_commitment(ctx, view, e_next, zc).is_ok()
                && pc.is_none_or(|pc| trial.apply_proof_commitment(ctx, view, e_next, pc).is_ok());
            if ok {
                s = trial;
            }
        }
        s
    }

    /// The decisions a commitment's votes produce (what its delegate must put
    /// in `decisions`).
    pub fn preview_decisions(
        &self,
        ctx: &Ctx,
        view: &dyn ChainView,
        zc: &ZoneCommitEnvelope,
    ) -> VResult<Vec<ZoneDecision>> {
        let z = zc.commitment.zone;
        let Some(zone) = self.zones.get(&z).cloned() else { reject!("unknown zone") };
        let blocks: Vec<(u64, &[SignedVote])> =
            zc.blocks.iter().map(|b| (b.signed.header.slot, b.votes.as_slice())).collect();
        self.clone().tally_votes(ctx, view, z, &zone, &blocks)
    }

    /// Counts the votes recorded in committed blocks; returns the decisions
    /// newly reached by them.
    fn tally_votes(
        &mut self,
        ctx: &Ctx,
        view: &dyn ChainView,
        z: ZoneId,
        zone: &ZoneEntry,
        blocks: &[(u64, &[SignedVote])],
    ) -> VResult<Vec<ZoneDecision>> {
        let t = ctx.timing(zone.slot_ticks);
        let mut decisions = Vec::new();
        for (slot, votes) in blocks {
            for v in votes.iter() {
                let vote = &v.vote;
                ensure!(vote.zone == z, "vote for another zone");
                ensure!(ctx.verify(&vote.voter, &vote.address(), &v.sig), "vote signature");
                // The proposal must be in Voting according to the block's anchor.
                let Some((_, a)) = view.anchor(t.beacon_slot_of(*slot)) else { reject!("no anchor") };
                ensure!(
                    a.proposals.get(&vote.proposal_id).is_some_and(|p| p.is_pending()),
                    "vote for a proposal not in Voting at the anchor"
                );
                let Some(prop) = self.proposals.get_mut(&vote.proposal_id) else { reject!("unknown proposal") };
                let Some(voters) = prop.voters.get(&z) else { reject!("zone {z} does not vote on this proposal") };
                ensure!(voters.contains(&vote.voter), "voter not in voters_a");
                let n = voters.len() as u64;
                let tally = prop.tally.entry(z).or_default();
                ensure!(!tally.yes.contains(&vote.voter) && !tally.no.contains(&vote.voter), "second vote");
                if !prop.status.eq(&ProposalStatus::Voting) || tally.decided.is_some() {
                    // Late votes are recorded but no longer count.
                    continue;
                }
                if vote.accept {
                    tally.yes.insert(vote.voter);
                } else {
                    tally.no.insert(vote.voter);
                }
                tally.votes.push(vote.clone());
                let need = (n * ctx.p.vote_num).div_ceil(ctx.p.vote_den);
                let decided = if tally.yes.len() as u64 >= need {
                    Some(true)
                } else if tally.no.len() as u64 > n - need {
                    Some(false)
                } else {
                    None
                };
                if let Some(accept) = decided {
                    tally.decided = Some(accept);
                    decisions.push(ZoneDecision {
                        accept,
                        yes: tally.yes.len() as u16,
                        no: tally.no.len() as u16,
                        proposal_id: vote.proposal_id,
                        votes_root: umis_core::records::list_root_of(&tally.votes),
                    });
                }
            }
        }
        Ok(decisions)
    }

    fn apply_decisions(&mut self, ctx: &Ctx, e: u64, _z: ZoneId, decisions: &[ZoneDecision]) {
        for d in decisions {
            let Some(prop) = self.proposals.get(&d.proposal_id) else { continue };
            if !prop.is_pending() {
                continue;
            }
            if !d.accept {
                self.proposals.get_mut(&d.proposal_id).expect("p").status = ProposalStatus::Rejected { e };
                continue;
            }
            let all = prop.adjacent.iter().all(|a| prop.tally.get(a).is_some_and(|t| t.decided == Some(true)));
            if all {
                self.activate(ctx, e, d.proposal_id);
            }
        }
    }

    /// Accepted: register the zone from `e_act` on and move the founders.
    fn activate(&mut self, ctx: &Ctx, e_dec: u64, id: Hash) {
        let prop = self.proposals[&id].clone();
        let t = ctx.timing(prop.slot_ticks);
        let p_new = t.commit_period();
        let mut e_act = e_dec + ctx.p.anchor_depth;
        while e_act % p_new != 0 {
            e_act += 1;
        }
        let zid = self.next_zone_id;
        self.next_zone_id += 1;
        let j_start = e_act * ctx.b() / prop.slot_ticks;
        let start_root = match ctx.instance(&prop.task_id, &prop.params, &prop.params_hash) {
            Ok(inst) => inst.state_root(&inst.initial_state(j_start)),
            Err(_) => Hash::ZERO,
        };
        self.zones.insert(
            zid,
            ZoneEntry {
                id: zid,
                point: prop.point,
                layer: prop.layer.clone(),
                task_id: prop.task_id.clone(),
                params: prop.params.clone(),
                params_hash: prop.params_hash,
                slot_ticks: prop.slot_ticks,
                active_from: e_act,
                j_start,
                start_root,
                start_tip: start_tip(zid, &start_root),
                inputs: Vec::new(),
            },
        );
        self.commits.insert(zid, Vec::new());
        for a in prop.adjacent.clone() {
            self.wire(ctx, a, zid, e_act);
            self.wire(ctx, zid, a, e_act);
        }
        for f in &prop.founders {
            if let Some(n) = self.nodes.get_mut(f) {
                n.zone = Some(zid);
            }
        }
        self.proposals.get_mut(&id).expect("p").status = ProposalStatus::Accepted { e_dec, e_act, zone: zid };
    }

    // ------------------------------------------------------------ proof commitments

    fn apply_proof_commitment(
        &mut self,
        ctx: &Ctx,
        view: &dyn ChainView,
        _e: u64,
        pce: &ProofCommitEnvelope,
    ) -> VResult<()> {
        let pc = &pce.pc;
        let z = pc.zone;
        ensure!(self.can_act_for(z, &pc.signer), "proof commitment signer");
        ensure!(ctx.verify(&pc.signer, &pc.address(), &pce.sig), "proof commitment signature");
        let Some(list) = self.commits.get(&z) else { reject!("unknown zone") };
        let Some(idx) = list.iter().position(|c| c.e == pc.beacon_slot && c.status == CommitStatus::Optimistic) else {
            reject!("no optimistic commitment of zone {z} for slot {}", pc.beacon_slot)
        };
        let c = &list[idx];
        let n = c.to_slot - c.from_slot;
        ensure!(
            pc.leaf_count as u64 == n && pce.leaves.len() as u64 == n && pce.proofs.len() as u64 == n,
            "leaf_count"
        );
        umis_proof::check_leaf_chain(&pce.leaves).map_err(|e| Reject(e.to_string()))?;
        let first = &pce.leaves[0];
        let last = pce.leaves.last().expect("leaves");
        ensure!(first.from_slot == c.from_slot && last.to_slot == c.to_slot, "leaf slot range");
        ensure!(first.prev_state_root == c.prev_state_root && last.state_root == c.state_root, "leaf ends");
        let zone = &self.zones[&z];
        let needs_data = ctx.backend.is_async();
        if needs_data {
            ensure!(pce.data.states.len() == pce.leaves.len() + 1, "proof data: states");
            ensure!(pce.data.inputs.len() == pce.leaves.len(), "proof data: inputs");
        }
        let input_maps: Vec<BTreeMap<u32, Bytes>> =
            if needs_data { pce.data.inputs.iter().map(|v| v.iter().cloned().collect()).collect() } else { Vec::new() };
        // The committed block covering a leaf's slot.
        let block_of = |leaf: &umis_core::records::ProofLeaf| {
            c.blocks.iter().find(|b| b.from_slot <= leaf.from_slot && leaf.from_slot <= b.slot)
        };
        let public = |i: usize, b: &BlockSummary| {
            if !needs_data {
                return umis_proof::PublicData::default();
            }
            umis_proof::PublicData {
                params: Some(&zone.params),
                genesis_seed: Some(ctx.genesis.seed),
                prev_state: Some(&pce.data.states[i]),
                next_state: Some(&pce.data.states[i + 1]),
                inputs: Some(&input_maps[i]),
                input_refs: Some(&c.inputs[b.input_start as usize..(b.input_start + b.input_count) as usize]),
            }
        };
        let warm: Vec<_> = pce
            .leaves
            .iter()
            .zip(&pce.proofs)
            .enumerate()
            .filter_map(|(i, (leaf, proof))| {
                Some((umis_proof::statement_of(leaf), public(i, block_of(leaf)?), proof.as_slice()))
            })
            .collect();
        ctx.warm_proofs(&warm);
        for (i, (leaf, proof)) in pce.leaves.iter().zip(&pce.proofs).enumerate() {
            ensure!(leaf.zone == z && leaf.params_hash == zone.params_hash, "leaf {i} zone/params");
            ensure!(leaf.to_slot == leaf.from_slot + 1, "leaf {i} must prove one slot");
            ensure!(leaf.proof_address == ctx.proof_address(proof), "leaf {i} proof address");
            // Bind the leaf to the committed block covering its slot.
            let Some(b) = block_of(leaf) else { reject!("leaf {i} not covered by a block") };
            ensure!(leaf.inputs_commitment == b.inputs_commitment, "leaf {i} inputs_commitment");
            if leaf.from_slot == b.slot {
                ensure!(leaf.state_root == b.state_root && leaf.outputs_root == b.outputs_root, "leaf {i} block end");
            }
            let stmt = umis_proof::statement_of(leaf);
            let prover = ctx.verify_proof(&stmt, &public(i, b), proof).map_err(|e| Reject(format!("leaf {i}: {e}")))?;
            if ctx.backend.identifies_prover() {
                // The prover must have been a member or full node at the slot's anchor.
                let slot_e = ctx.timing(zone.slot_ticks).beacon_slot_of(leaf.from_slot);
                let allowed =
                    view.anchor(slot_e).is_some_and(|(_, a)| a.can_act_for(z, &prover)) || self.can_act_for(z, &prover);
                ensure!(allowed, "leaf {i}: prover not a member/full node");
            }
        }
        let root = umis_proof::proof_bmt_root(&pce.leaves);
        ensure!(pc.proof_bmt_root == root, "proof_bmt_root");
        let c = &mut self.commits.get_mut(&z).expect("z")[idx];
        c.status = CommitStatus::Proven;
        c.proof_bmt_root = Some(root);
        Ok(())
    }

    // ------------------------------------------------------------ faults

    fn apply_fault(&mut self, ctx: &Ctx, view: &dyn ChainView, e: u64, f: &FaultTx) -> VResult<()> {
        let r = &f.record;
        ensure!(r.kind == f.body.kind(), "fault kind");
        ensure!(r.evidence_root == bmt::file_address(&borsh::to_vec(&f.body).expect("borsh")), "evidence_root");
        let key = (r.minter, r.zone, r.zone_slot);
        ensure!(!self.punished.contains_key(&key), "fault already punished");
        let Some(zone) = self.zones.get(&r.zone).cloned() else { reject!("fault: unknown zone") };
        let t = ctx.timing(zone.slot_ticks);
        ensure!(t.beacon_slot_of(r.zone_slot) + ctx.p.unbonding >= e, "fault older than U beacon slots");
        let stake = self.nodes.get(&r.minter).map(|n| n.stake).unwrap_or(0);
        ensure!(stake > 0, "fault: minter has no stake");
        let mut discard = None;
        match &f.body {
            FaultBody::Equivocation { a, b } => {
                let (ha, hb) = (&a.header, &b.header);
                ensure!(ha.minter == r.minter && hb.minter == r.minter, "equivocation: minter");
                ensure!(ha.zone == r.zone && hb.zone == r.zone, "equivocation: zone");
                ensure!(ha.slot == r.zone_slot && hb.slot == r.zone_slot, "equivocation: slot");
                ensure!(a.hash() != b.hash(), "equivocation: same block");
                ensure!(ctx.verify(&r.minter, &a.hash(), &a.sig) && ctx.verify(&r.minter, &b.hash(), &b.sig), "sigs");
            }
            FaultBody::InvalidTransition { block, parent_state } => {
                let h = block.header();
                self.check_fault_block(ctx, r, block)?;
                ensure!(bmt::file_address(parent_state) == h.prev_state_root, "parent state does not match");
                ensure!(block.attachments.len() == block.inputs.len(), "attachments");
                for (ir, a) in block.inputs.iter().zip(&block.attachments) {
                    ensure!(bmt::file_address(&a.payload) == ir.payload_address, "input payload address");
                }
                let inst = ctx.instance(&zone.task_id, &zone.params, &zone.params_hash).map_err(Reject)?;
                let honest = h.params_hash == zone.params_hash
                    && exec::reexecute_block(inst.as_ref(), parent_state, block)
                        .is_ok_and(|ex| exec::matches_header(&ex, h));
                ensure!(!honest, "invalid-transition evidence: the block re-executes correctly");
                discard = Some(block.hash());
            }
            FaultBody::InvalidInputRef { block, input_index, parent } => {
                let h = block.header();
                self.check_fault_block(ctx, r, block)?;
                let i = *input_index as usize;
                ensure!(i < block.inputs.len(), "input index");
                let ir = &block.inputs[i];
                let bad = self.input_violation(ctx, view, &zone, h, ir, block.attachments.get(i), parent.as_ref());
                ensure!(bad, "invalid-input evidence: the input is valid");
                discard = Some(block.hash());
            }
        }
        self.punish(ctx, e, r, stake);
        if let Some(hash) = discard {
            self.discard_block(e, r.zone, hash);
        }
        Ok(())
    }

    fn check_fault_block(&self, ctx: &Ctx, r: &FaultEvidence, block: &ZoneBlock) -> VResult<()> {
        let h = block.header();
        ensure!(
            h.minter == r.minter && h.zone == r.zone && h.slot == r.zone_slot,
            "evidence block does not match record"
        );
        ensure!(ctx.verify(&h.minter, &block.hash(), &block.signed.sig), "evidence block signature");
        ensure!(exec::inputs_commitment(&block.inputs) == h.inputs_commitment, "evidence inputs_commitment");
        Ok(())
    }

    /// Whether an InputRef breaks a cheap rule.
    fn input_violation(
        &self,
        ctx: &Ctx,
        view: &dyn ChainView,
        zone: &ZoneEntry,
        h: &ZoneBlockHeader,
        ir: &InputRef,
        att: Option<&InputAttachment>,
        parent: Option<&ZoneBlock>,
    ) -> bool {
        let t = ctx.timing(zone.slot_ticks);
        let links = self.links_at(zone.id, t.beacon_slot_of(h.slot));
        let Some(l) = links.iter().find(|l| l.link_id == ir.link_id) else { return true };
        if l.from_zone != ir.src_zone {
            return true;
        }
        let Some(pz) = self.zones.get(&ir.src_zone) else { return true };
        if (ir.src_slot + 1) * pz.slot_ticks > h.from_slot * zone.slot_ticks {
            return true;
        }
        let Some(a) = att else { return true };
        if exec::check_attachment(ir, a, |s| ctx.verify(&s.header.minter, &s.hash(), &s.sig)).is_err() {
            return true;
        }
        if check_producer(ctx, view, self, &a.producer).is_err() {
            return true;
        }
        if let Some(p) = parent
            && p.hash() == h.parent
            && exec::inputs_commitment(&p.inputs) == p.header().inputs_commitment
            && let Some(prev) = p.inputs.iter().find(|x| x.link_id == ir.link_id)
            && prev.src_slot > ir.src_slot
        {
            return true;
        }
        false
    }

    fn punish(&mut self, ctx: &Ctx, e: u64, r: &FaultEvidence, stake: u64) {
        let slashed = stake * ctx.p.slash_pct / 100;
        let reward = slashed * ctx.p.reward_pct / 100;
        *self.balances.entry(r.reporter).or_default() += reward;
        self.burned += slashed - reward;
        let n = self.nodes.get_mut(&r.minter).expect("minter has stake");
        n.stake -= slashed;
        n.zone = None;
        n.status =
            if n.stake == 0 { NodeStatus::Removed } else { NodeStatus::Unbonding { until: e + ctx.p.unbonding } };
        self.punished.insert((r.minter, r.zone, r.zone_slot), e);
        self.slash_log.push(SlashRecord {
            e,
            minter: r.minter,
            zone: r.zone,
            zone_slot: r.zone_slot,
            kind: r.kind,
            reporter: r.reporter,
            slashed,
            reward,
        });
    }

    /// A faulty block: commitments containing it become invalid.
    fn discard_block(&mut self, e: u64, z: ZoneId, hash: Hash) {
        self.discarded.insert(hash, e);
        let hit = self
            .commits
            .get(&z)
            .and_then(|l| {
                l.iter().find(|c| c.status != CommitStatus::Invalid && c.blocks.iter().any(|b| b.hash == hash))
            })
            .map(|c| (c.e, c.status));
        if let Some((ce, status)) = hit
            && status != CommitStatus::Settled
        {
            self.invalidate_from(z, ce, e, RollbackReason::Fault);
        }
    }

    /// Marks commitment `ce` of zone `z` and every later one invalid; the
    /// rollback records are written by `global_rollback` once the block's
    /// invalidations are complete.
    fn invalidate_from(&mut self, z: ZoneId, ce: u64, _e: u64, reason: RollbackReason) {
        let list = self.commits.get_mut(&z).expect("zone");
        let mut any = false;
        for c in list.iter_mut().filter(|c| c.e >= ce) {
            if c.status != CommitStatus::Invalid && c.status != CommitStatus::Settled {
                c.status = CommitStatus::Invalid;
                any = true;
            }
        }
        if any {
            self.invalidated_now.entry(z).or_insert(reason);
        }
    }

    /// Any invalidation rolls every active zone back to its newest valid
    /// commitment, all with the same `e_record`, so all recover together at
    /// `e_record + D`. Consumer commitments that took inputs from a producer's
    /// discarded range are invalidated first (to a fixed point).
    fn global_rollback(&mut self, e: u64) {
        loop {
            let bounds: BTreeMap<ZoneId, u64> = self.zones.keys().map(|z| (*z, self.commit_base(*z).0)).collect();
            let mut hit = None;
            'outer: for (c, list) in &self.commits {
                for ci in list {
                    if matches!(ci.status, CommitStatus::Invalid | CommitStatus::Settled) {
                        continue;
                    }
                    if ci.inputs.iter().any(|r| bounds.get(&r.src_zone).is_some_and(|jr| r.src_slot >= *jr)) {
                        hit = Some((*c, ci.e));
                        break 'outer;
                    }
                }
            }
            match hit {
                Some((c, ce)) => self.invalidate_from(c, ce, e, RollbackReason::CrossLink),
                None => break,
            }
        }
        let zones: Vec<ZoneId> = self.zones.values().filter(|z| z.active_from <= e).map(|z| z.id).collect();
        for z in zones {
            let (j_r, root, tip, _, cp) = self.commit_base(z);
            let reason = self.invalidated_now.get(&z).copied().unwrap_or(RollbackReason::Synchronized);
            let rec = RollbackRecord {
                zone: z,
                checkpoint_e: cp,
                j_r,
                checkpoint_root: root,
                checkpoint_tip: tip,
                e_record: e,
                reason,
            };
            self.rollback.insert(z, rec.clone());
            self.rollback_log.push(rec);
        }
        self.invalidated_now.clear();
    }

    // ------------------------------------------------------------ proposals

    fn apply_propose(&mut self, ctx: &Ctx, e: u64, p: &ProposeZoneTx) -> VResult<()> {
        let r = &p.record;
        let id = p.proposal_id();
        ensure!(!self.proposals.contains_key(&id), "proposal already registered");
        let point = [r.px, r.py, r.pz];
        let layer = r.layer_id.as_str().to_string();
        // proposer
        let proposer = self.nodes.get(&r.proposer);
        ensure!(
            proposer.is_some_and(|n| n.status == NodeStatus::Active && n.stake >= ctx.p.min_stake),
            "proposer needs min_stake locked"
        );
        ensure!(self.pending_proposals().all(|x| x.proposer != r.proposer), "proposer has a pending proposal");
        ensure!((self.pending_proposals().count() as u32) < ctx.p.max_pending_proposals, "too many pending proposals");
        // location
        use umis_task::Topology;
        ensure!(ctx.topo.in_bounds(&point), "point outside the lattice bounds");
        ensure!(ctx.layers.iter().any(|l| l.as_str() == layer), "unknown layer");
        ensure!(self.zone_at(&point, &layer).is_none(), "location occupied");
        let loc = Location { point, layer: LayerId::new(&layer) };
        for other in self.pending_proposals() {
            let ol = Location { point: other.point, layer: LayerId::new(&other.layer) };
            ensure!(ol != loc, "location locked by a pending proposal");
            ensure!(!umis_task::topology::linked(&ctx.topo, &ctx.couplings, &ol, &loc), "linked to a pending proposal");
        }
        // task and params
        let task = ctx.tasks.get(&p.task_id).map_err(|e| Reject(e.to_string()))?;
        ensure!(task.layer() == layer, "task occupies layer {}", task.layer());
        task.validate_params(&p.params).map_err(|e| Reject(e.to_string()))?;
        task.check_zone(&p.params, point, r.slot_ticks as u64).map_err(|e| Reject(e.to_string()))?;
        let params_hash = umis_task::params_hash(&p.task_id, &layer, &p.params);
        ensure!(params_hash == r.params_hash, "params_hash");
        let timing =
            umis_core::time::ZoneTiming::new(r.slot_ticks as u64, ctx.b()).map_err(|e| Reject(e.to_string()))?;
        // adjacency
        let mut adjacent = Vec::new();
        for z in self.zones.values().filter(|z| z.active_from <= e) {
            let zl = Location { point: z.point, layer: LayerId::new(&z.layer) };
            if umis_task::topology::linked(&ctx.topo, &ctx.couplings, &zl, &loc) {
                adjacent.push(z.id);
            }
        }
        ensure!(!adjacent.is_empty(), "no adjacent zone");
        let mut voters = BTreeMap::new();
        for a in &adjacent {
            let m = self.members_of(*a);
            ensure!(!m.is_empty(), "adjacent zone {a} has no members");
            voters.insert(*a, m);
        }
        // founders
        ensure!(!p.founders.is_empty() && p.founders.len() == p.founder_sigs.len(), "founders");
        ensure!(r.founder_count as usize == p.founders.len(), "founder_count");
        let froot = bmt::list_root(&p.founders.iter().map(|f| f.as_hash()).collect::<Vec<_>>());
        ensure!(froot == r.founders_root, "founders_root");
        let mut leaving: BTreeMap<ZoneId, usize> = BTreeMap::new();
        for (f, sig) in p.founders.iter().zip(&p.founder_sigs) {
            ensure!(ctx.verify(f, &id, sig), "founder signature");
            let n = self.nodes.get(f);
            ensure!(
                n.is_some_and(|n| n.status == NodeStatus::Active && n.kind == NodeKind::Member),
                "founder not a member"
            );
            ensure!(self.pending_proposals().all(|x| !x.founders.contains(f)), "founder in another proposal");
            if let Some(z) = n.and_then(|n| n.zone) {
                *leaving.entry(z).or_default() += 1;
            }
        }
        for (z, k) in leaving {
            ensure!(self.zone_member_count(z) > k, "founders would empty zone {z}");
        }
        let max_p = adjacent.iter().map(|a| ctx.timing(self.zones[a].slot_ticks).commit_period()).max().unwrap_or(1);
        let _ = timing;
        self.proposals.insert(
            id,
            Proposal {
                id,
                point,
                layer,
                task_id: p.task_id.clone(),
                params: p.params.clone(),
                params_hash,
                slot_ticks: r.slot_ticks as u64,
                proposer: r.proposer,
                founders: p.founders.clone(),
                adjacent,
                voters,
                e_reg: e,
                deadline: e + ctx.p.voting_window + max_p,
                tally: BTreeMap::new(),
                status: ProposalStatus::Voting,
            },
        );
        Ok(())
    }

    // ------------------------------------------------------------ cross-links, settlement

    /// Where an InputRef stands against the producer's commitments.
    fn input_status(&self, r: &InputRef) -> Link {
        let Some(list) = self.commits.get(&r.src_zone) else { return Link::Bad };
        let Some(pz) = self.zones.get(&r.src_zone) else { return Link::Bad };
        if r.src_slot < pz.j_start {
            return Link::Bad;
        }
        if let Some(c) = list
            .iter()
            .rev()
            .find(|c| c.status != CommitStatus::Invalid && c.from_slot <= r.src_slot && r.src_slot < c.to_slot)
        {
            return if c.blocks.iter().any(|b| b.slot == r.src_slot && b.hash == r.src_block_hash) {
                Link::Ok
            } else {
                Link::Bad
            };
        }
        if list.iter().any(|c| c.status == CommitStatus::Invalid && c.from_slot <= r.src_slot && r.src_slot < c.to_slot)
        {
            return Link::Bad;
        }
        // Older than everything we keep: settled history.
        if list.iter().find(|c| c.status != CommitStatus::Invalid).is_some_and(|c| r.src_slot < c.from_slot) {
            return Link::Ok;
        }
        if self.discarded.contains_key(&r.src_block_hash) {
            return Link::Bad;
        }
        Link::Pending
    }

    /// Cross-link check, run until nothing changes.
    fn resolve(&mut self, _ctx: &Ctx, e: u64) {
        loop {
            let mut bad: Option<(ZoneId, u64)> = None;
            let mut unresolved: Vec<(ZoneId, usize, u32)> = Vec::new();
            'outer: for (z, list) in &self.commits {
                for (i, c) in list.iter().enumerate() {
                    if matches!(c.status, CommitStatus::Invalid | CommitStatus::Settled) {
                        continue;
                    }
                    let mut pending = 0;
                    for r in &c.inputs {
                        match self.input_status(r) {
                            Link::Ok => {}
                            Link::Pending => pending += 1,
                            Link::Bad => {
                                tracing::debug!(
                                    zone = z,
                                    commit = c.e,
                                    src_zone = r.src_zone,
                                    src_slot = r.src_slot,
                                    "cross-link check failed at block {e}"
                                );
                                bad = Some((*z, c.e));
                                break 'outer;
                            }
                        }
                    }
                    unresolved.push((*z, i, pending));
                }
            }
            for (z, i, n) in unresolved {
                self.commits.get_mut(&z).expect("z")[i].unresolved = n;
            }
            match bad {
                Some((z, ce)) => self.invalidate_from(z, ce, e, RollbackReason::CrossLink),
                None => break,
            }
        }
    }

    /// Settled: proven, at least `D` slots old, all inputs resolved, the
    /// previous commitment settled, and every producer commitment it took
    /// inputs from settled too (greatest fixed point, so input cycles settle
    /// together).
    fn settle(&mut self, ctx: &Ctx, e: u64) {
        let mut cand: BTreeSet<(ZoneId, u64)> = BTreeSet::new();
        for (z, list) in &self.commits {
            for c in list {
                if c.status == CommitStatus::Proven && c.included_at + ctx.p.anchor_depth <= e && c.unresolved == 0 {
                    cand.insert((*z, c.e));
                }
            }
        }
        loop {
            let mut drop = Vec::new();
            for &(z, ce) in &cand {
                let list = &self.commits[&z];
                let c = list.iter().find(|c| c.e == ce).expect("cand");
                // previous valid commitment of the same zone
                let prev_ok = list
                    .iter()
                    .rev()
                    .find(|p| p.e < ce && p.status != CommitStatus::Invalid)
                    .is_none_or(|p| p.status == CommitStatus::Settled || cand.contains(&(z, p.e)));
                let deps_ok = c.inputs.iter().all(|r| {
                    self.commits.get(&r.src_zone).is_some_and(|pl| {
                        match pl.iter().rev().find(|pc| {
                            pc.status != CommitStatus::Invalid && pc.from_slot <= r.src_slot && r.src_slot < pc.to_slot
                        }) {
                            Some(pc) => pc.status == CommitStatus::Settled || cand.contains(&(r.src_zone, pc.e)),
                            None => pl
                                .iter()
                                .find(|f| f.status != CommitStatus::Invalid)
                                .is_some_and(|f| r.src_slot < f.from_slot),
                        }
                    })
                });
                if !prev_ok || !deps_ok {
                    drop.push((z, ce));
                }
            }
            if drop.is_empty() {
                break;
            }
            for d in drop {
                cand.remove(&d);
            }
        }
        for (z, ce) in cand {
            let c = self.commits.get_mut(&z).expect("z").iter_mut().find(|c| c.e == ce).expect("c");
            c.status = CommitStatus::Settled;
            c.inputs.clear();
        }
    }

    fn prune(&mut self, ctx: &Ctx, e: u64) {
        // Commitments before a zone's newest settled one are history: keep only
        // recent invalid ones (they still mark referenced blocks as bad).
        for list in self.commits.values_mut() {
            if let Some(ls) = list.iter().rposition(|c| c.status == CommitStatus::Settled) {
                let mut i = 0;
                list.retain(|c| {
                    let keep = i >= ls || (c.status == CommitStatus::Invalid && c.included_at + ctx.p.unbonding >= e);
                    i += 1;
                    keep
                });
            }
        }
        self.punished.retain(|_, at| *at + ctx.p.unbonding >= e);
        self.discarded.retain(|_, at| *at + ctx.p.unbonding >= e);
        if self.rollback_log.len() > 64 {
            let n = self.rollback_log.len() - 64;
            self.rollback_log.drain(..n);
        }
        if self.slash_log.len() > 64 {
            let n = self.slash_log.len() - 64;
            self.slash_log.drain(..n);
        }
        let old: Vec<Hash> = self
            .proposals
            .iter()
            .filter(|(_, p)| !p.is_pending() && p.deadline + 10 * ctx.p.unbonding < e)
            .map(|(id, _)| *id)
            .collect();
        for id in old {
            self.proposals.remove(&id);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    Ok,
    Pending,
    Bad,
}

/// `(txs_root, zones_root, proofs_root)` of a body.
pub fn body_roots(b: &BeaconBody) -> (Hash, Hash, Hash) {
    let txs: Vec<Hash> = b.txs.iter().map(|t| t.tx.address()).collect();
    let zs: Vec<Hash> = b.zone_commitments.iter().map(|z| z.address()).collect();
    let ps: Vec<Hash> = b.proof_commitments.iter().map(|p| p.pc.address()).collect();
    (bmt::list_root(&txs), bmt::list_root(&zs), bmt::list_root(&ps))
}
