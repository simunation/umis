//! One zone's chain as seen by a node that follows it: block store, fork
//! choice, validation (cheap input checks + re-execution), minting,
//! commitments and proof commitments.
//!
//! Fork choice for zone slot `j` uses the anchor state `A = anchor(j)`: the
//! longest chain descending from `A`'s latest valid commitment, ties to the
//! lower minter rank, then the lower hash. A rollback record in `A` discards
//! every block covering slots ≥ `j_r` that was minted under an anchor without
//! that record (`BlockEntry::rollback_e`); the minter then builds the
//! recovery block on the checkpoint tip, recomputing `j_r ..= j` with the
//! inputs held from before the checkpoint.

use super::inputs::{Candidate, InputStore};
use crate::beacon::state::check_producer;
use crate::beacon::{BeaconChain, BeaconState};
use crate::ctx::Ctx;
use crate::exec::{self, Exec};
use crate::ranks;
use crate::types::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use umis_core::records::*;
use umis_core::{Hash, NodeId, SecretKey, SegRecord};

#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub hash: Hash,
    pub parent: Hash,
    /// `None` for the zone start; a synced checkpoint keeps its tip block.
    pub block: Option<Arc<ZoneBlock>>,
    /// The zone start or a synced checkpoint: history before it is not held.
    pub root: bool,
    pub from_slot: u64,
    /// Exclusive end of the covered slots (`slot + 1`).
    pub end: u64,
    pub height: u64,
    pub rank: u32,
    /// `e_record` of the zone's newest rollback record in the block's anchor
    /// state (0: none): blocks minted before a rollback took effect are discarded.
    pub rollback_e: u64,
    /// The beacon slot whose anchor the block used, and that anchor's hash.
    pub anchor: (u64, Hash),
    pub prev_root: Hash,
    pub state_root: Hash,
    pub outputs_root: Hash,
    pub state: Option<Arc<Bytes>>,
    /// `(slot, state_root, outputs_root)` per covered slot.
    pub slot_roots: Vec<(u64, Hash, Hash)>,
    /// Newest consumed input per link along this chain.
    pub last_inputs: Arc<BTreeMap<u32, Candidate>>,
    /// Votes recorded along this chain.
    pub voted: Arc<BTreeSet<(Hash, NodeId)>>,
}

#[derive(Debug)]
pub enum BlockOutcome {
    Accepted,
    Duplicate,
    /// Parent unknown; buffered.
    Orphan {
        parent: Hash,
    },
    /// Cannot be judged yet (anchor not known / future slot).
    Retry(String),
    Invalid {
        reason: String,
        fault: Option<FaultBody>,
    },
}

pub struct ZoneEngine {
    pub z: ZoneId,
    pub entries: HashMap<Hash, BlockEntry>,
    pub children: HashMap<Hash, Vec<Hash>>,
    pub orphans: HashMap<Hash, Vec<Arc<ZoneBlock>>>,
    pub rejected: BTreeMap<Hash, Arc<ZoneBlock>>,
    pub minted: BTreeSet<u64>,
    pub seen_headers: BTreeMap<(u64, NodeId), SignedHeader>,
    /// `(block hash, slot)` → proof share.
    pub shares: BTreeMap<(Hash, u64), ProofShare>,
    /// Slots with a proving job in flight.
    pub proving: BTreeSet<(Hash, u64)>,
    pub sent_commit: BTreeSet<u64>,
    pub sent_proof: BTreeSet<u64>,
}

fn slot_of(ctx: &Ctx, zone: &crate::beacon::ZoneEntry, j: u64) -> u64 {
    ctx.timing(zone.slot_ticks).beacon_slot_of(j)
}

impl ZoneEngine {
    pub fn new(z: ZoneId) -> Self {
        Self {
            z,
            entries: HashMap::new(),
            children: HashMap::new(),
            orphans: HashMap::new(),
            rejected: BTreeMap::new(),
            minted: BTreeSet::new(),
            seen_headers: BTreeMap::new(),
            shares: BTreeMap::new(),
            proving: BTreeSet::new(),
            sent_commit: BTreeSet::new(),
            sent_proof: BTreeSet::new(),
        }
    }

    pub fn has_root(&self) -> bool {
        !self.entries.is_empty()
    }

    /// The zone start as root: `initial_state(j_start)`.
    pub fn install_start(&mut self, ctx: &Ctx, zone: &crate::beacon::ZoneEntry) -> Result<(), String> {
        let inst = ctx.instance(&zone.task_id, &zone.params, &zone.params_hash)?;
        let state = inst.initial_state(zone.j_start);
        let outputs = inst.outputs(&state).map_err(|e| e.to_string())?;
        self.insert_root(BlockEntry {
            hash: zone.start_tip,
            parent: Hash::ZERO,
            block: None,
            root: true,
            from_slot: zone.j_start,
            end: zone.j_start,
            height: 0,
            rank: 0,
            rollback_e: 0,
            anchor: (0, Hash::ZERO),
            prev_root: zone.start_root,
            state_root: zone.start_root,
            outputs_root: exec::outputs_root(&outputs),
            state: Some(Arc::new(state)),
            slot_roots: Vec::new(),
            last_inputs: Arc::new(BTreeMap::new()),
            voted: Arc::new(BTreeSet::new()),
        });
        Ok(())
    }

    /// A synced checkpoint (verified against a valid commitment) as root.
    pub fn install_checkpoint(&mut self, cp: &Checkpoint, info: &crate::beacon::CommitInfo) {
        let (parent, from) = match &cp.tip {
            Some(b) => (b.header().parent, b.header().from_slot),
            None => (Hash::ZERO, info.to_slot),
        };
        let last: BTreeMap<u32, Candidate> =
            cp.last_inputs.iter().map(|(r, a)| (r.link_id, (r.clone(), a.clone()))).collect();
        self.insert_root(BlockEntry {
            hash: info.tip,
            parent,
            block: cp.tip.clone().map(Arc::new),
            root: true,
            from_slot: from,
            end: info.to_slot,
            height: info.to_slot,
            rank: 0,
            rollback_e: 0,
            anchor: (0, Hash::ZERO),
            prev_root: info.prev_state_root,
            state_root: info.state_root,
            outputs_root: info.outputs_root,
            state: Some(Arc::new(cp.state.clone())),
            slot_roots: Vec::new(),
            last_inputs: Arc::new(last),
            voted: Arc::new(BTreeSet::new()),
        });
    }

    fn insert_root(&mut self, e: BlockEntry) {
        self.entries.entry(e.hash).or_insert(e);
    }

    pub fn get(&self, h: &Hash) -> Option<&BlockEntry> {
        self.entries.get(h)
    }

    /// Whether the chain ending at `h` is valid under anchor state `a`.
    /// Blocks must also have used the canonical anchor of their slot (a block
    /// minted on a beacon fork that lost is discarded with it).
    pub fn valid_under(&self, h: &Hash, a: &BeaconState, chain: &BeaconChain) -> bool {
        if !a.zones.contains_key(&self.z) {
            return false;
        }
        let (base_end, _, base_tip, _, _) = a.commit_base(self.z);
        let rb = a.rollback.get(&self.z);
        let mut cur = *h;
        loop {
            if cur == base_tip {
                return true;
            }
            let Some(e) = self.entries.get(&cur) else { return false };
            if e.root {
                return e.end >= base_end;
            }
            if e.end <= base_end {
                return false;
            }
            if a.discarded.contains_key(&cur) {
                return false;
            }
            if chain.anchor(e.anchor.0).map(|x| x.0) != Some(e.anchor.1) {
                return false;
            }
            if let Some(r) = rb
                && e.end > r.j_r
                && e.rollback_e < r.e_record
            {
                return false;
            }
            cur = e.parent;
        }
    }

    /// Fork choice under anchor state `a`.
    pub fn best_tip(&self, a: &BeaconState, chain: &BeaconChain) -> Option<&BlockEntry> {
        let (base_end, ..) = a.commit_base(self.z);
        self.entries
            .values()
            .filter(|e| e.end >= base_end && e.state.is_some())
            .filter(|e| self.valid_under(&e.hash, a, chain))
            .max_by(|x, y| x.end.cmp(&y.end).then(y.rank.cmp(&x.rank)).then(y.hash.cmp(&x.hash)))
    }

    /// Whether some valid block already covers slot `j`.
    pub fn has_block_for(&self, j: u64, a: &BeaconState, chain: &BeaconChain) -> bool {
        self.entries
            .values()
            .any(|e| e.block.is_some() && e.from_slot <= j && j < e.end && self.valid_under(&e.hash, a, chain))
    }

    // ------------------------------------------------------------ validation

    /// Validates and (if valid) inserts a block. Orphans are buffered.
    pub fn on_block(&mut self, ctx: &Ctx, chain: &BeaconChain, b: Arc<ZoneBlock>, reexecute: bool) -> BlockOutcome {
        let hash = b.hash();
        if self.entries.contains_key(&hash) || self.rejected.contains_key(&hash) {
            return BlockOutcome::Duplicate;
        }
        let h = b.header().clone();
        if h.zone != self.z {
            return BlockOutcome::Invalid { reason: "wrong zone".into(), fault: None };
        }
        let Some(parent) = self.entries.get(&h.parent).cloned() else {
            let v = self.orphans.entry(h.parent).or_default();
            if v.iter().all(|x| x.hash() != hash) && v.len() < 16 {
                v.push(b);
            }
            return BlockOutcome::Orphan { parent: h.parent };
        };
        match self.validate(ctx, chain, &b, &parent, reexecute) {
            Ok(entry) => {
                self.insert_entry(entry);
                BlockOutcome::Accepted
            }
            Err(Verdict::Retry(r)) => BlockOutcome::Retry(r),
            Err(Verdict::Invalid(reason, fault)) => {
                self.rejected.insert(hash, b);
                while self.rejected.len() > 256 {
                    let k = *self.rejected.keys().next().expect("non-empty");
                    self.rejected.remove(&k);
                }
                BlockOutcome::Invalid { reason, fault }
            }
        }
    }

    /// Orphans whose parent just arrived.
    pub fn take_orphans(&mut self, parent: &Hash) -> Vec<Arc<ZoneBlock>> {
        self.orphans.remove(parent).unwrap_or_default()
    }

    fn insert_entry(&mut self, e: BlockEntry) {
        self.children.entry(e.parent).or_default().push(e.hash);
        self.entries.insert(e.hash, e);
    }

    /// Records a header for equivocation detection; returns the conflicting one.
    pub fn note_header(&mut self, s: &SignedHeader) -> Option<SignedHeader> {
        let key = (s.header.slot, s.header.minter);
        match self.seen_headers.get(&key) {
            Some(prev) if prev.hash() != s.hash() => Some(prev.clone()),
            Some(_) => None,
            None => {
                self.seen_headers.insert(key, s.clone());
                None
            }
        }
    }

    fn validate(
        &self,
        ctx: &Ctx,
        chain: &BeaconChain,
        b: &Arc<ZoneBlock>,
        parent: &BlockEntry,
        reexecute: bool,
    ) -> Result<BlockEntry, Verdict> {
        let h = b.header();
        let head = chain.head_state();
        let Some(zone) = head.zones.get(&self.z).cloned() else { return Err(Verdict::Retry("zone unknown".into())) };
        let e_b = slot_of(ctx, &zone, h.slot);
        let Some((ahash, a)) = chain.anchor(e_b) else { return Err(Verdict::Retry("anchor unknown".into())) };
        if h.anchor_hash != ahash {
            return Err(Verdict::Retry("anchor mismatch (beacon view differs)".into()));
        }
        let bad = |s: String| Err(Verdict::Invalid(s, None));
        if h.from_slot != parent.end || h.slot < h.from_slot {
            return bad(format!("slots {}..={} do not continue parent end {}", h.from_slot, h.slot, parent.end));
        }
        if h.prev_state_root != parent.state_root {
            return bad("prev_state_root".into());
        }
        if h.params_hash != zone.params_hash {
            return bad("params_hash".into());
        }
        if !self.valid_under(&parent.hash, &a, chain) {
            return bad("parent is discarded under the block's anchor".into());
        }
        let list = ranks::zone_ranks(&a, self.z, h.slot);
        let Some(rank) = ranks::rank_of(&list, &h.minter) else { return bad("minter not in rank list".into()) };
        if rank > ctx.p.max_zone_rank {
            return bad(format!("minter rank {rank} above R"));
        }
        if !ctx.verify(&h.minter, &b.hash(), &b.signed.sig) {
            return bad("signature".into());
        }
        if h.input_count as usize != b.inputs.len()
            || b.attachments.len() != b.inputs.len()
            || exec::inputs_commitment(&b.inputs) != h.inputs_commitment
        {
            return bad("inputs_commitment".into());
        }
        // Inputs: cheap checks.
        let links = a.links_at(self.z, e_b);
        let mut last = (*parent.last_inputs).clone();
        let mut prev_link = None;
        for (i, (r, att)) in b.inputs.iter().zip(&b.attachments).enumerate() {
            let fault = |why: &str| {
                Err(Verdict::Invalid(
                    format!("input {i}: {why}"),
                    Some(FaultBody::InvalidInputRef {
                        block: (**b).clone(),
                        input_index: i as u32,
                        parent: parent.block.as_ref().map(|p| (**p).clone()),
                    }),
                ))
            };
            if prev_link.is_some_and(|l| l >= r.link_id) {
                return bad("inputs not in link order".into());
            }
            prev_link = Some(r.link_id);
            let Some(l) = links.iter().find(|l| l.link_id == r.link_id) else { return fault("unknown link") };
            if l.from_zone != r.src_zone {
                return fault("wrong source zone");
            }
            let Some(pz) = head.zones.get(&r.src_zone) else { return fault("unknown producer") };
            if (r.src_slot + 1) * pz.slot_ticks > h.from_slot * zone.slot_ticks {
                return fault("non-causal");
            }
            if let Some((prev, _)) = last.get(&r.link_id) {
                if r.src_slot < prev.src_slot {
                    return fault("non-monotonic");
                }
                if h.from_slot < h.slot && prev != r {
                    return bad("multi-slot blocks must hold the last inputs".into());
                }
            }
            if exec::check_attachment(r, att, |s| ctx.verify(&s.header.minter, &s.hash(), &s.sig)).is_err() {
                return fault("attachment");
            }
            if from_new_input(&last, r) && producer_doomed(chain, &a, &att.producer.header) {
                return bad(format!("input {i}: producer block discarded by a rollback"));
            }
            match check_producer(ctx, &chain.view(), head, &att.producer) {
                Ok(()) => {}
                Err(e) if e.0.contains("anchor") => return Err(Verdict::Retry(e.0)),
                Err(_) => return fault("illegitimate producer minter"),
            }
            last.insert(r.link_id, (r.clone(), att.clone()));
        }
        // Votes.
        if h.vote_count as usize != b.votes.len() || exec::votes_root(&b.votes) != h.votes_root {
            return bad("votes_root".into());
        }
        let mut voted = (*parent.voted).clone();
        for v in &b.votes {
            if let Err(e) = check_vote(ctx, &a, self.z, v) {
                return bad(e);
            }
            if !voted.insert((v.vote.proposal_id, v.vote.voter)) {
                return bad("second vote in this chain".into());
            }
        }
        // Re-execution.
        let (state, slot_roots) = if reexecute {
            let Some(pstate) = parent.state.as_ref() else { return Err(Verdict::Retry("parent state unknown".into())) };
            let inst = ctx
                .instance(&zone.task_id, &zone.params, &zone.params_hash)
                .map_err(|e| Verdict::Retry(format!("instance: {e}")))?;
            match exec::reexecute_block(inst.as_ref(), pstate, b) {
                Ok(ex) if exec::matches_header(&ex, h) => (Some(Arc::new(ex.state)), ex.slots),
                _ => {
                    return Err(Verdict::Invalid(
                        "re-execution mismatch".into(),
                        Some(FaultBody::InvalidTransition { block: (**b).clone(), parent_state: (**pstate).clone() }),
                    ));
                }
            }
        } else {
            (None, vec![(h.slot, h.state_root, h.outputs_root)])
        };
        Ok(BlockEntry {
            hash: b.hash(),
            parent: parent.hash,
            block: Some(b.clone()),
            root: false,
            from_slot: h.from_slot,
            end: h.slot + 1,
            height: parent.height + 1,
            rank,
            rollback_e: a.rollback.get(&self.z).map(|r| r.e_record).unwrap_or(0),
            anchor: (e_b, ahash),
            prev_root: h.prev_state_root,
            state_root: h.state_root,
            outputs_root: h.outputs_root,
            state,
            slot_roots,
            last_inputs: Arc::new(last),
            voted: Arc::new(voted),
        })
    }

    // ------------------------------------------------------------ minting

    /// Mints the block for slot `j` on the best tip. Variants other than
    /// `Honest` exist for adversary testing and are not stored locally.
    pub fn mint(
        &mut self,
        ctx: &Ctx,
        chain: &BeaconChain,
        key: &SecretKey,
        inputs: &InputStore,
        votes: &[SignedVote],
        j: u64,
        variant: MintVariant,
    ) -> Option<(Arc<ZoneBlock>, Exec)> {
        let corrupt = variant == MintVariant::CorruptState;
        let forge_input = variant == MintVariant::ForgeInput;
        let head = chain.head_state().clone();
        let zone = head.zones.get(&self.z)?.clone();
        let e_b = slot_of(ctx, &zone, j);
        let (ahash, a) = chain.anchor(e_b)?;
        let rank = ranks::rank_of(&ranks::zone_ranks(&a, self.z, j), &key.node_id())?;
        let tip = self.best_tip(&a, chain)?.clone();
        if tip.end > j || j < zone.j_start {
            return None;
        }
        let from = tip.end;
        let links = a.links_at(self.z, e_b);
        let mut chosen: Vec<Candidate> = Vec::new();
        for l in &links {
            let last = tip.last_inputs.get(&l.link_id);
            let pick = if from < j || variant == MintVariant::HoldInputs {
                last.cloned()
            } else {
                let pz = head.zones.get(&l.from_zone)?;
                // freshest causal producer output: (j_p + 1)·S_p ≤ from·S_c
                let hi = (from * zone.slot_ticks / pz.slot_ticks).checked_sub(1);
                let lo = last.map(|(r, _)| r.src_slot).unwrap_or(0);
                let fresh = hi.and_then(|hi| {
                    inputs
                        .range(l.from_zone, lo, hi)
                        .into_iter()
                        .filter(|bnd| {
                            let h = &bnd.producer.header;
                            !head.discarded.contains_key(&bnd.producer.hash())
                                && !producer_doomed(chain, &a, h)
                                && !producer_doomed(chain, &head, h)
                                // never a block the producer's committed chain does not contain
                                && head.committed_block_at(h.zone, h.slot).is_none_or(|c| c == Some(bnd.producer.hash()))
                        })
                        .filter_map(|bnd| {
                            // Same-slot candidates: the one the producer's fork choice
                            // keeps (lower minter rank, then lower hash).
                            let h = &bnd.producer.header;
                            let rank = chain
                                .anchor(ctx.timing(pz.slot_ticks).beacon_slot_of(h.slot))
                                .and_then(|(_, pa)| ranks::rank_of(&ranks::zone_ranks(&pa, h.zone, h.slot), &h.minter))
                                .unwrap_or(u32::MAX);
                            InputStore::candidate(bnd, l.link_id, l.from_port).map(|c| (c, rank))
                        })
                        .max_by(|(x, xr), (y, yr)| {
                            x.0.src_slot
                                .cmp(&y.0.src_slot)
                                .then(yr.cmp(xr))
                                .then(y.0.src_block_hash.cmp(&x.0.src_block_hash))
                        })
                        .map(|(c, _)| c)
                });
                match (fresh, last) {
                    (Some(f), Some(l)) if f.0.src_slot < l.0.src_slot => Some(l.clone()),
                    (Some(f), _) => Some(f),
                    (None, l) => l.cloned(),
                }
            };
            if let Some(mut c) = pick {
                if forge_input && from == j {
                    // Claim a producer slot from the future: non-causal.
                    c.0.src_slot += 1000;
                }
                chosen.push(c);
            }
        }
        let inst = ctx.instance(&zone.task_id, &zone.params, &zone.params_hash).ok()?;
        let input_refs: Vec<InputRef> = chosen.iter().map(|c| c.0.clone()).collect();
        let attachments: Vec<InputAttachment> = chosen.iter().map(|c| c.1.clone()).collect();
        let pstate = tip.state.clone()?;
        let ex = exec::execute(inst.as_ref(), &pstate, &exec::input_map(&input_refs, &attachments), from, j).ok()?;
        let block_votes: Vec<SignedVote> = votes
            .iter()
            .filter(|v| {
                !tip.voted.contains(&(v.vote.proposal_id, v.vote.voter)) && check_vote(ctx, &a, self.z, v).is_ok()
            })
            .take(16)
            .cloned()
            .collect();
        let mut state_root = ex.state_root();
        if corrupt {
            state_root = umis_core::keccak256_concat(&[b"corrupt", &state_root.0]);
        }
        let header = ZoneBlockHeader {
            zone: self.z,
            from_slot: from,
            slot: j,
            input_count: input_refs.len() as u16,
            vote_count: block_votes.len() as u16,
            parent: tip.hash,
            anchor_hash: ahash,
            params_hash: zone.params_hash,
            prev_state_root: tip.state_root,
            state_root,
            outputs_root: ex.outputs_root(),
            inputs_commitment: exec::inputs_commitment(&input_refs),
            votes_root: exec::votes_root(&block_votes),
            minter: key.node_id(),
        };
        let sig = key.sign(&header.address());
        let block = Arc::new(ZoneBlock {
            signed: SignedHeader { header, sig },
            inputs: input_refs,
            attachments,
            votes: block_votes,
        });
        self.minted.insert(j);
        // A corrupt block is kept by its (faulty) minter, which builds on and
        // commits it; the other adversarial variants are only published.
        if matches!(variant, MintVariant::Honest | MintVariant::CorruptState) {
            let mut last = (*tip.last_inputs).clone();
            for c in chosen {
                last.insert(c.0.link_id, c);
            }
            let mut voted = (*tip.voted).clone();
            for v in &block.votes {
                voted.insert((v.vote.proposal_id, v.vote.voter));
            }
            self.insert_entry(BlockEntry {
                hash: block.hash(),
                parent: tip.hash,
                block: Some(block.clone()),
                root: false,
                from_slot: from,
                end: j + 1,
                height: tip.height + 1,
                rank,
                rollback_e: a.rollback.get(&self.z).map(|r| r.e_record).unwrap_or(0),
                anchor: (e_b, ahash),
                prev_root: tip.state_root,
                state_root: block.header().state_root,
                outputs_root: ex.outputs_root(),
                state: Some(Arc::new(ex.state.clone())),
                slot_roots: ex.slots.clone(),
                last_inputs: Arc::new(last),
                voted: Arc::new(voted),
            });
        }
        Some((block, ex))
    }

    // ------------------------------------------------------------ proofs

    /// Proofs this node owes for an accepted block (it is the slot's priority
    /// prover, or `all`): ready shares for inline backends, jobs for async
    /// ones (the caller runs them and hands the proof to [`Self::job_done`]).
    pub fn make_shares(
        &mut self,
        ctx: &Ctx,
        chain: &BeaconChain,
        key: &SecretKey,
        h: &Hash,
        all: bool,
        next_job: &mut u64,
    ) -> Vec<ProveOut> {
        let Some(e) = self.entries.get(h).cloned() else { return Vec::new() };
        let Some(b) = e.block.clone() else { return Vec::new() };
        let head = chain.head_state();
        let Some(zone) = head.zones.get(&self.z).cloned() else { return Vec::new() };
        let Some((_, a)) = chain.anchor(slot_of(ctx, &zone, b.header().slot)) else { return Vec::new() };
        let Ok(inst) = ctx.instance(&zone.task_id, &zone.params, &zone.params_hash) else { return Vec::new() };
        let Some(mut state) = self.entries.get(&e.parent).and_then(|p| p.state.clone()).map(|s| (*s).clone()) else {
            return Vec::new();
        };
        let inputs = exec::input_map(&b.inputs, &b.attachments);
        let mut out = Vec::new();
        let mut prev = e.prev_root;
        for sr in &e.slot_roots {
            let mine = all || ranks::priority_prover(&a, self.z, sr.0, &b.header().minter) == Some(key.node_id());
            let stmt = exec::slot_statement(b.header(), prev, *sr);
            prev = sr.1;
            let wanted = mine && !self.shares.contains_key(&(e.hash, sr.0)) && !self.proving.contains(&(e.hash, sr.0));
            if wanted {
                if ctx.backend.is_async() {
                    *next_job += 1;
                    self.proving.insert((e.hash, sr.0));
                    out.push(ProveOut::Job(Box::new(umis_proof::ProveJob {
                        id: *next_job,
                        block_hash: e.hash,
                        stmt,
                        task_id: zone.task_id.clone(),
                        params: zone.params.clone(),
                        genesis_seed: ctx.genesis.seed,
                        prev_state: state.clone(),
                        inputs: inputs.clone(),
                    })));
                } else {
                    let w = umis_proof::Witness {
                        instance: inst.as_ref(),
                        params: &zone.params,
                        genesis_seed: ctx.genesis.seed,
                        prev_state: &state,
                        inputs: &inputs,
                    };
                    if let Ok(proof) = ctx.backend.prove(&stmt, &w, key) {
                        let share =
                            ProofShare { leaf: umis_proof::proof_leaf(&stmt, &proof), proof, block_hash: e.hash };
                        self.shares.insert((e.hash, sr.0), share.clone());
                        out.push(ProveOut::Share(share));
                    }
                }
            }
            // The next slot of a multi-slot block starts from this slot's end state.
            if e.slot_roots.len() > 1 {
                match inst.step(&state, &inputs, sr.0) {
                    Ok(s) => state = s,
                    Err(_) => break,
                }
            }
        }
        out
    }

    /// A finished proving job: the proof becomes a share.
    pub fn job_done(&mut self, job: &umis_proof::ProveJob, proof: Bytes) -> ProofShare {
        let slot = job.stmt.from_slot;
        self.proving.remove(&(job.block_hash, slot));
        let share = ProofShare { leaf: umis_proof::proof_leaf(&job.stmt, &proof), proof, block_hash: job.block_hash };
        self.shares.insert((job.block_hash, slot), share.clone());
        share
    }

    /// Stores a received proof share if it verifies.
    pub fn on_share(&mut self, ctx: &Ctx, chain: &BeaconChain, s: ProofShare) -> bool {
        let Some(e) = self.entries.get(&s.block_hash) else { return false };
        let slot = s.leaf.from_slot;
        if self.shares.contains_key(&(s.block_hash, slot)) {
            return false;
        }
        let Some(sr) = e.slot_roots.iter().find(|r| r.0 == slot) else { return false };
        if s.leaf.state_root != sr.1
            || s.leaf.outputs_root != sr.2
            || s.leaf.proof_address != ctx.proof_address(&s.proof)
        {
            return false;
        }
        let stmt = umis_proof::statement_of(&s.leaf);
        let ok = if ctx.backend.is_async() {
            // Verified where this node holds the states (single-slot blocks);
            // the beacon chain verifies every proof of a proof commitment anyway.
            let zone = chain.head_state().zones.get(&self.z).cloned();
            let inputs = e.block.as_ref().map(|b| exec::input_map(&b.inputs, &b.attachments)).unwrap_or_default();
            match zone.as_ref().and_then(|z| self.share_public(ctx, z, e, &inputs)) {
                Some(pd) => ctx.verify_proof(&stmt, &pd, &s.proof).is_ok(),
                None => true,
            }
        } else {
            ctx.verify_proof(&stmt, &umis_proof::PublicData::default(), &s.proof).is_ok()
        };
        if !ok {
            return false;
        }
        self.shares.insert((s.block_hash, slot), s);
        true
    }

    // ------------------------------------------------------------ commitments

    /// The covered chain from the head state's commit base to a block ending
    /// at `j*(e)`, if this node has it.
    fn commit_path(&self, head: &BeaconState, chain: &BeaconChain, to_slot: u64) -> Option<Vec<BlockEntry>> {
        let (base_end, _, base_tip, _, _) = head.commit_base(self.z);
        let tip = self
            .entries
            .values()
            .filter(|e| e.end == to_slot && e.block.is_some() && self.valid_under(&e.hash, head, chain))
            .min_by(|x, y| x.rank.cmp(&y.rank).then(x.hash.cmp(&y.hash)))?;
        let mut path = vec![tip.clone()];
        let mut cur = tip.parent;
        while cur != base_tip {
            let e = self.entries.get(&cur)?;
            if e.end <= base_end || e.root {
                return None;
            }
            path.push(e.clone());
            cur = e.parent;
        }
        path.reverse();
        Some(path)
    }

    /// Builds the zone commitment for commit slot `e` (and the checkpoint).
    /// `head` is the beacon state the commitment must apply to (normally the
    /// head with pending commitments applied, see `BeaconState::with_pending`).
    pub fn build_commitment(
        &self,
        ctx: &Ctx,
        chain: &BeaconChain,
        head: &BeaconState,
        key: &SecretKey,
        e: u64,
    ) -> Option<(ZoneCommitEnvelope, Bytes)> {
        let zone = head.zones.get(&self.z)?;
        let t = ctx.timing(zone.slot_ticks);
        if !t.is_commit_slot(e)
            || head.commits[&self.z].iter().any(|c| c.e >= e && c.status != crate::beacon::CommitStatus::Invalid)
        {
            return None;
        }
        let path = self.commit_path(head, chain, t.j_star(e))?;
        let (_, base_root, _, base_last, _) = head.commit_base(self.z);
        let mut last = base_last;
        let mut all_inputs = Vec::new();
        let mut blocks = Vec::new();
        for p in &path {
            let b = p.block.as_ref()?;
            for r in &b.inputs {
                last.insert(r.link_id, r.clone());
            }
            all_inputs.extend(b.inputs.iter().cloned());
            blocks.push(CommitBlock { signed: b.signed.clone(), inputs: b.inputs.clone(), votes: b.votes.clone() });
        }
        let deps: Vec<Dep> = last
            .values()
            .map(|r| Dep {
                link_id: r.link_id,
                src_zone: r.src_zone,
                src_slot: r.src_slot,
                src_block_hash: r.src_block_hash,
            })
            .collect();
        let tip = path.last()?;
        let mut env = ZoneCommitEnvelope {
            commitment: ZoneCommitment {
                zone: self.z,
                beacon_slot: e,
                dep_count: deps.len() as u16,
                decision_count: 0,
                tip_block_hash: tip.hash,
                prev_state_root: base_root,
                state_root: tip.state_root,
                outputs_root: tip.outputs_root,
                inputs_root: list_root_of(&all_inputs),
                deps_root: list_root_of(&deps),
                decisions_root: list_root_of::<ZoneDecision>(&[]),
                signer: key.node_id(),
            },
            sig: Default::default(),
            deps,
            decisions: Vec::new(),
            blocks,
        };
        // Decisions: recount exactly as the beacon state transition will.
        let decisions = head.preview_decisions(ctx, &chain.view(), &env).ok()?;
        env.commitment.decision_count = decisions.len() as u16;
        env.commitment.decisions_root = list_root_of(&decisions);
        env.decisions = decisions;
        env.sig = key.sign(&env.address());
        Some((env, (**tip.state.as_ref()?).clone()))
    }

    /// Builds the proof commitment for a commitment, self-proving missing
    /// slots if `self_prove`.
    pub fn build_proof_commitment(
        &mut self,
        ctx: &Ctx,
        chain: &BeaconChain,
        key: &SecretKey,
        zc: &ZoneCommitEnvelope,
        self_prove: bool,
    ) -> Option<ProofCommitEnvelope> {
        let mut leaves = Vec::new();
        let mut proofs = Vec::new();
        for cb in &zc.blocks {
            let h = cb.signed.hash();
            let hdr = &cb.signed.header;
            for j in hdr.from_slot..=hdr.slot {
                if !self.shares.contains_key(&(h, j)) {
                    if !self_prove || ctx.backend.is_async() {
                        return None;
                    }
                    let mut n = 0;
                    self.make_shares(ctx, chain, key, &h, true, &mut n);
                }
                let s = self.shares.get(&(h, j))?;
                leaves.push(s.leaf.clone());
                proofs.push(s.proof.clone());
            }
        }
        let pc = ProofCommitment {
            zone: self.z,
            beacon_slot: zc.commitment.beacon_slot,
            leaf_count: leaves.len() as u32,
            proof_bmt_root: umis_proof::proof_bmt_root(&leaves),
            signer: key.node_id(),
        };
        let sig = key.sign(&pc.address());
        let data = if ctx.backend.is_async() { self.leaf_data(ctx, chain, zc)? } else { Default::default() };
        Some(ProofCommitEnvelope { pc, sig, leaves, proofs, data })
    }

    /// States at every covered slot boundary and each slot's inputs.
    fn leaf_data(&self, ctx: &Ctx, chain: &BeaconChain, zc: &ZoneCommitEnvelope) -> Option<umis_proof::LeafData> {
        let zone = chain.head_state().zones.get(&self.z)?.clone();
        let inst = ctx.instance(&zone.task_id, &zone.params, &zone.params_hash).ok()?;
        let mut data = umis_proof::LeafData::default();
        for cb in &zc.blocks {
            let e = self.entries.get(&cb.signed.hash())?;
            let b = e.block.as_ref()?;
            let mut st = (**self.entries.get(&e.parent)?.state.as_ref()?).clone();
            if data.states.is_empty() {
                data.states.push(st.clone());
            }
            let inputs = exec::input_map(&b.inputs, &b.attachments);
            let h = b.header();
            for j in h.from_slot..=h.slot {
                st = if j == h.slot { (**e.state.as_ref()?).clone() } else { inst.step(&st, &inputs, j).ok()? };
                data.states.push(st.clone());
                data.inputs.push(inputs.clone().into_iter().collect());
            }
        }
        Some(data)
    }

    /// Public data to verify a share of a single-slot block (members hold the states).
    fn share_public<'a>(
        &'a self,
        ctx: &'a Ctx,
        zone: &'a crate::beacon::ZoneEntry,
        e: &'a BlockEntry,
        inputs: &'a BTreeMap<u32, Bytes>,
    ) -> Option<umis_proof::PublicData<'a>> {
        let b = e.block.as_ref()?;
        if b.header().from_slot != b.header().slot {
            return None;
        }
        Some(umis_proof::PublicData {
            params: Some(&zone.params),
            genesis_seed: Some(ctx.genesis.seed),
            prev_state: Some(self.entries.get(&e.parent)?.state.as_ref()?),
            next_state: Some(e.state.as_ref()?),
            inputs: Some(inputs),
            input_refs: Some(&b.inputs),
        })
    }

    /// Canonical blocks covering slots `from..=to` (for sync responses).
    pub fn blocks_in(&self, a: &BeaconState, chain: &BeaconChain, from: u64, to: u64) -> Vec<ZoneBlock> {
        let Some(tip) = self.best_tip(a, chain) else { return Vec::new() };
        let mut out = Vec::new();
        let mut cur = Some(tip.clone());
        while let Some(e) = cur {
            if e.end <= from {
                break;
            }
            if let Some(b) = &e.block
                && b.header().from_slot <= to
            {
                out.push((**b).clone());
            }
            cur = self.entries.get(&e.parent).cloned();
        }
        out.reverse();
        out
    }

    /// Drops entries far behind the head state's commit base.
    pub fn prune(&mut self, head: &BeaconState, keep_slots: u64) {
        let Some(list) = head.commits.get(&self.z) else { return };
        let keep_tips: BTreeSet<Hash> = list.iter().map(|c| c.tip).collect();
        let floor = list.first().map(|c| c.from_slot).unwrap_or(0).saturating_sub(keep_slots);
        let (_, _, base_tip, _, _) = head.commit_base(self.z);
        let drop: Vec<Hash> = self
            .entries
            .values()
            .filter(|e| e.end < floor && !keep_tips.contains(&e.hash) && e.hash != base_tip)
            .map(|e| e.hash)
            .collect();
        for h in drop {
            self.entries.remove(&h);
            self.children.remove(&h);
        }
        self.seen_headers.retain(|(s, _), _| s.saturating_add(keep_slots.saturating_mul(4)) >= floor);
        let entries = &self.entries;
        self.shares.retain(|(h, _), _| entries.contains_key(h));
        self.minted.retain(|s| s.saturating_add(keep_slots.saturating_mul(4)) >= floor);
    }
}

/// A proof made inline, or a job for an async backend.
#[derive(Debug, Clone)]
pub enum ProveOut {
    Share(ProofShare),
    Job(Box<umis_proof::ProveJob>),
}

/// How to mint (everything but `Honest` is adversarial, for tests).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MintVariant {
    Honest,
    /// Claim a wrong state root (invalid transition).
    CorruptState,
    /// Record a non-causal InputRef.
    ForgeInput,
    /// Hold every input instead of taking fresh ones (a second, different
    /// but valid block for equivocation).
    HoldInputs,
}

#[derive(Debug)]
enum Verdict {
    Retry(String),
    Invalid(String, Option<FaultBody>),
}

/// Whether `r` is a newly consumed input (not the held last value).
fn from_new_input(last: &BTreeMap<u32, Candidate>, r: &InputRef) -> bool {
    last.get(&r.link_id).is_none_or(|(prev, _)| prev != r)
}

/// Whether a producer block is discarded under anchor state `a`: faulty, or
/// covering slots the producer rolled back and minted before the rollback.
pub fn producer_doomed(chain: &BeaconChain, a: &BeaconState, h: &ZoneBlockHeader) -> bool {
    if a.discarded.contains_key(&h.address()) {
        return true;
    }
    let Some(r) = a.rollback.get(&h.zone) else { return false };
    if h.slot < r.j_r {
        return false;
    }
    let rollback_e = chain
        .get(&h.anchor_hash)
        .and_then(|e| e.state.as_ref())
        .and_then(|s| s.rollback.get(&h.zone).map(|x| x.e_record))
        .unwrap_or(0);
    rollback_e < r.e_record
}

/// A vote may be recorded under anchor state `a`.
pub fn check_vote(ctx: &Ctx, a: &BeaconState, z: ZoneId, v: &SignedVote) -> Result<(), String> {
    if v.vote.zone != z {
        return Err("vote for another zone".into());
    }
    let Some(p) = a.proposals.get(&v.vote.proposal_id) else { return Err("proposal not in anchor".into()) };
    if !p.is_pending() {
        return Err("proposal not in Voting".into());
    }
    if !p.voters.get(&z).is_some_and(|vs| vs.contains(&v.vote.voter)) {
        return Err("voter not in voters_a".into());
    }
    if !ctx.verify(&v.vote.voter, &v.vote.address(), &v.sig) {
        return Err("vote signature".into());
    }
    Ok(())
}
