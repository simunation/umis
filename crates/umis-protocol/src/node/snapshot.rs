//! A serializable view of a node for status endpoints and UIs.

use super::*;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ZoneView {
    pub zone: ZoneId,
    pub point: [i32; 3],
    pub layer: String,
    pub slot_ticks: u64,
    pub members: usize,
    pub following: bool,
    /// Canonical tip: `(slot, block hash, state root)`.
    pub tip: Option<(u64, String, String)>,
    /// Newest commitment on the beacon chain: `(beacon slot, status, state root)`.
    pub last_commit: Option<(u64, String, String)>,
    /// This node's minter rank in the current zone slot (if ≤ R).
    pub my_rank: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProposalView {
    pub id: String,
    pub point: [i32; 3],
    pub layer: String,
    pub status: String,
    /// adjacent zone → (yes, no, voters)
    pub tally: Vec<(ZoneId, usize, usize, usize)>,
    pub deadline: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub node_id: String,
    pub now_slot: Option<u64>,
    pub head_slot: Option<u64>,
    pub head_hash: String,
    pub head_height: u64,
    pub finalized_slot: Option<u64>,
    pub finalized_hash: String,
    pub kind: Option<String>,
    pub zone: Option<ZoneId>,
    pub stake: u64,
    pub balance: u64,
    pub peers: usize,
    pub zones: Vec<ZoneView>,
    pub proposals: Vec<ProposalView>,
    pub slashes: usize,
    pub rollbacks: usize,
    pub minted: u64,
    pub proposed: u64,
    pub faults_reported: u64,
    pub tx_pool: usize,
    pub zone_blocks_accepted: u64,
    pub invalid_blocks: u64,
    pub rejected_beacon: u64,
    /// Followed zones without state yet (waiting for a checkpoint).
    pub syncing: Vec<ZoneId>,
    pub pending_requests: usize,
    pub gossip_in: BTreeMap<String, u64>,
    pub forks: u64,
    pub proved: u64,
    pub jobs_dropped: u64,
    pub tx_kinds: BTreeMap<String, usize>,
}

impl Node {
    pub fn snapshot(&self, now: u64) -> Snapshot {
        let head = self.chain.head();
        let fin = self.chain.finalized();
        let st = self.head_state();
        let e_now = self.e_now(now);
        let me = st.nodes.get(&self.me);
        let mut zones = Vec::new();
        for z in st.zones.values() {
            let tip = self.engines.get(&z.id).and_then(|eng| {
                let (_, a) = self.chain.anchor(e_now.unwrap_or(0))?;
                eng.best_tip(&a, &self.chain).map(|t| (t.end.saturating_sub(1), t.hash.short(), t.state_root.short()))
            });
            let last_commit = st
                .commits
                .get(&z.id)
                .and_then(|l| l.last())
                .map(|c| (c.e, format!("{:?}", c.status), c.state_root.short()));
            let my_rank = self.ctx.grid.tick_at(now).and_then(|t| {
                let j = t / z.slot_ticks;
                let (_, a) = self.chain.anchor(self.ctx.timing(z.slot_ticks).beacon_slot_of(j))?;
                ranks::rank_of(&ranks::zone_ranks(&a, z.id, j), &self.me).filter(|r| *r <= self.ctx.p.max_zone_rank)
            });
            zones.push(ZoneView {
                zone: z.id,
                point: z.point,
                layer: z.layer.clone(),
                slot_ticks: z.slot_ticks,
                members: st.members_of(z.id).len(),
                following: self.engines.contains_key(&z.id),
                tip,
                last_commit,
                my_rank,
            });
        }
        let proposals = st
            .proposals
            .values()
            .map(|p| ProposalView {
                id: p.id.to_hex(),
                point: p.point,
                layer: p.layer.clone(),
                status: format!("{:?}", p.status),
                tally: p
                    .adjacent
                    .iter()
                    .map(|a| {
                        let t = p.tally.get(a);
                        (
                            *a,
                            t.map(|t| t.yes.len()).unwrap_or(0),
                            t.map(|t| t.no.len()).unwrap_or(0),
                            p.voters.get(a).map(|v| v.len()).unwrap_or(0),
                        )
                    })
                    .collect(),
                deadline: p.deadline,
            })
            .collect();
        Snapshot {
            node_id: self.me.to_hex(),
            now_slot: e_now,
            head_slot: head.slot,
            head_hash: head.hash.to_hex(),
            head_height: head.height,
            finalized_slot: fin.slot,
            finalized_hash: fin.hash.to_hex(),
            kind: me.map(|n| format!("{:?}", n.kind)),
            zone: self.my_zone(),
            stake: me.map(|n| n.stake).unwrap_or(0),
            balance: st.balances.get(&self.me).copied().unwrap_or(0),
            peers: self.peers.len(),
            zones,
            proposals,
            slashes: st.slash_log.len(),
            rollbacks: st.rollback_log.len(),
            minted: self.stats.minted,
            proposed: self.stats.proposed,
            faults_reported: self.stats.faults_reported,
            tx_pool: self.tx_pool.len(),
            zone_blocks_accepted: self.stats.zone_blocks_accepted,
            invalid_blocks: self.stats.invalid_blocks,
            rejected_beacon: self.stats.rejected_beacon,
            syncing: self.engines.iter().filter(|(_, e)| !e.has_root()).map(|(z, _)| *z).collect(),
            pending_requests: self.pending.len(),
            gossip_in: self.stats.gossip_in.clone(),
            forks: self.stats.forks,
            proved: self.stats.proved,
            jobs_dropped: self.stats.jobs_dropped,
            tx_kinds: self.tx_pool.values().fold(BTreeMap::new(), |mut m, (_, t)| {
                *m.entry(t.tx.kind_name().to_string()).or_default() += 1;
                m
            }),
        }
    }

    /// Display data of a followed zone's canonical tip.
    pub fn zone_render(&self, now: u64, z: ZoneId) -> Option<umis_task::RenderGrid> {
        let st = self.head_state();
        let zone = st.zones.get(&z)?;
        let eng = self.engines.get(&z)?;
        let (_, a) = self.chain.anchor(self.e_now(now)?)?;
        let tip = eng.best_tip(&a, &self.chain)?;
        let inst = self.ctx.instance(&zone.task_id, &zone.params, &zone.params_hash).ok()?;
        inst.render(tip.state.as_ref()?).ok()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LeafView {
    pub from_slot: u64,
    pub to_slot: u64,
    pub state_root: String,
    pub proof_address: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitView {
    pub zone: ZoneId,
    pub e: u64,
    pub address: String,
    pub state_root: String,
    pub outputs_root: String,
    pub inputs_root: String,
    pub tip: String,
    pub blocks: usize,
    /// optimistic / proven / settled / invalid, or `pruned` once out of the
    /// state's window.
    pub status: String,
    pub proof_root: Option<String>,
    pub leaves: Vec<LeafView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BeaconView {
    pub slot: u64,
    pub hash: String,
    pub proposer: String,
    pub txs: Vec<String>,
    pub commits: Vec<CommitView>,
    pub zones_root: String,
    pub proofs_root: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatticeView {
    pub blocks: Vec<BeaconView>,
    pub slashes: Vec<crate::beacon::state::SlashRecord>,
    pub rollbacks: Vec<crate::beacon::state::RollbackRecord>,
    pub free_points: Vec<[i32; 3]>,
    pub bounds: [i32; 3],
}

impl Node {
    /// Canonical beacon blocks with slots in `[from, to]` and their commitments.
    pub fn lattice(&self, from: u64, to: u64) -> LatticeView {
        let st = self.head_state();
        let status = |z: ZoneId, e: u64| {
            st.commits
                .get(&z)
                .and_then(|l| l.iter().find(|c| c.e == e))
                .map(|c| format!("{:?}", c.status).to_lowercase())
                .unwrap_or_else(|| "pruned".into())
        };
        let mut blocks = Vec::new();
        for entry in self.chain.canonical() {
            let (Some(slot), Some(b)) = (entry.slot, entry.block.as_ref()) else { continue };
            if slot < from || slot > to {
                continue;
            }
            let commits = b
                .body
                .zone_commitments
                .iter()
                .map(|zc| {
                    let c = &zc.commitment;
                    let pc = b
                        .body
                        .proof_commitments
                        .iter()
                        .find(|p| p.pc.zone == c.zone && p.pc.beacon_slot == c.beacon_slot);
                    CommitView {
                        zone: c.zone,
                        e: c.beacon_slot,
                        address: zc.address().to_hex(),
                        state_root: c.state_root.to_hex(),
                        outputs_root: c.outputs_root.to_hex(),
                        inputs_root: c.inputs_root.to_hex(),
                        tip: c.tip_block_hash.to_hex(),
                        blocks: zc.blocks.len(),
                        status: status(c.zone, c.beacon_slot),
                        proof_root: pc.map(|p| p.pc.proof_bmt_root.to_hex()),
                        leaves: pc
                            .map(|p| {
                                p.leaves
                                    .iter()
                                    .map(|l| LeafView {
                                        from_slot: l.from_slot,
                                        to_slot: l.to_slot,
                                        state_root: l.state_root.to_hex(),
                                        proof_address: l.proof_address.to_hex(),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    }
                })
                .collect();
            blocks.push(BeaconView {
                slot,
                hash: entry.hash.to_hex(),
                proposer: b.header.proposer.to_hex(),
                txs: b.body.txs.iter().map(|t| t.tx.kind_name().to_string()).collect(),
                commits,
                zones_root: b.header.zones_root.to_hex(),
                proofs_root: b.header.proofs_root.to_hex(),
            });
        }
        // Free water locations next to existing zones (candidates for creation).
        let mut free = std::collections::BTreeSet::new();
        use umis_task::Topology;
        for z in st.zones.values().filter(|z| z.layer == "water") {
            for p in self.ctx.topo.adjacent(&z.point) {
                if st.zone_at(&p, "water").is_none() {
                    free.insert(p);
                }
            }
        }
        LatticeView {
            blocks,
            slashes: st.slash_log.clone(),
            rollbacks: st.rollback_log.clone(),
            free_points: free.into_iter().collect(),
            bounds: self.ctx.topo.bounds,
        }
    }
}
