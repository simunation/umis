//! The beacon block tree: validation on insert, fork choice, finality,
//! anchors.
//!
//! - fork choice: the chain with the most blocks wins; ties go to the lower
//!   proposer rank at the first divergence, then the lower hash;
//! - finality: `DepthK(K_f)`: a block is final once it has `K_f` canonical
//!   descendants; the head only moves off it for a strictly longer chain
//!   (a healed partition), which is counted in `finality_reverts`;
//! - anchor of beacon slot `e`: the newest block with slot ≤ `e − D`.

use super::state::{BeaconState, ChainView, Reject, VResult};
use crate::ctx::Ctx;
use crate::ranks;
use crate::types::BeaconBlock;
use std::collections::HashMap;
use std::sync::Arc;
use umis_core::Hash;

/// Pluggable finality rule (a BFT gadget can replace depth finality later).
pub trait FinalityRule: Send + Sync {
    /// Height of the newest final block given the head height.
    fn finalized_height(&self, head_height: u64) -> u64;
}

pub struct DepthK(pub u64);

impl FinalityRule for DepthK {
    fn finalized_height(&self, head_height: u64) -> u64 {
        head_height.saturating_sub(self.0)
    }
}

#[derive(Debug, Clone)]
pub struct BeaconEntry {
    pub hash: Hash,
    pub parent: Hash,
    pub slot: Option<u64>,
    /// Blocks since genesis (genesis = 0).
    pub height: u64,
    pub block: Option<Arc<BeaconBlock>>,
    /// State after this block; `None` once pruned.
    pub state: Option<Arc<BeaconState>>,
    pub rank: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsertOutcome {
    pub new_head: bool,
    pub reorg: bool,
}

pub struct BeaconChain {
    pub genesis: Hash,
    entries: HashMap<Hash, BeaconEntry>,
    head: Hash,
    /// Canonical hashes by height.
    canonical: Vec<Hash>,
    finality: Box<dyn FinalityRule>,
    anchor_depth: u64,
    /// Times the head moved off a locally finalized block (partition heal).
    pub finality_reverts: u64,
}

/// Anchors on the branch ending at `tip`.
pub struct BranchView<'a> {
    pub chain: &'a BeaconChain,
    pub tip: Hash,
}

impl ChainView for BranchView<'_> {
    fn anchor(&self, e: u64) -> Option<(Hash, Arc<BeaconState>)> {
        self.chain.anchor_on(&self.tip, e)
    }
}

impl BeaconChain {
    pub fn new(ctx: &Ctx) -> Result<Self, String> {
        let state = BeaconState::genesis(ctx)?;
        let genesis = ctx.genesis_hash;
        let mut entries = HashMap::new();
        entries.insert(
            genesis,
            BeaconEntry {
                hash: genesis,
                parent: Hash::ZERO,
                slot: None,
                height: 0,
                block: None,
                state: Some(Arc::new(state)),
                rank: 0,
            },
        );
        Ok(Self {
            genesis,
            entries,
            head: genesis,
            canonical: vec![genesis],
            finality: Box::new(DepthK(ctx.p.finality_depth)),
            anchor_depth: ctx.p.anchor_depth,
            finality_reverts: 0,
        })
    }

    pub fn head(&self) -> &BeaconEntry {
        &self.entries[&self.head]
    }

    pub fn head_state(&self) -> &Arc<BeaconState> {
        self.head().state.as_ref().expect("head state kept")
    }

    pub fn get(&self, h: &Hash) -> Option<&BeaconEntry> {
        self.entries.get(h)
    }

    pub fn contains(&self, h: &Hash) -> bool {
        self.entries.contains_key(h)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn finalized(&self) -> &BeaconEntry {
        let h = self.finality.finalized_height(self.head().height) as usize;
        &self.entries[&self.canonical[h.min(self.canonical.len() - 1)]]
    }

    pub fn canonical_at_height(&self, h: u64) -> Option<&BeaconEntry> {
        self.canonical.get(h as usize).and_then(|x| self.entries.get(x))
    }

    pub fn canonical(&self) -> impl Iterator<Item = &BeaconEntry> {
        self.canonical.iter().filter_map(|h| self.entries.get(h))
    }

    /// The canonical block of beacon slot `e`, if one exists.
    pub fn canonical_by_slot(&self, e: u64) -> Option<&BeaconEntry> {
        self.canonical().find(|x| x.slot == Some(e))
    }

    pub fn is_canonical(&self, h: &Hash) -> bool {
        self.entries.get(h).is_some_and(|x| self.canonical.get(x.height as usize) == Some(h))
    }

    /// Newest block on the branch ending at `tip` with slot ≤ `e − D`.
    pub fn anchor_on(&self, tip: &Hash, e: u64) -> Option<(Hash, Arc<BeaconState>)> {
        let limit = e.checked_sub(self.anchor_depth);
        let mut cur = self.entries.get(tip)?;
        loop {
            let ok = match (cur.slot, limit) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(s), Some(l)) => s <= l,
            };
            if ok {
                return cur.state.clone().map(|s| (cur.hash, s));
            }
            cur = self.entries.get(&cur.parent)?;
        }
    }

    /// Anchor of beacon slot `e` on the canonical chain.
    pub fn anchor(&self, e: u64) -> Option<(Hash, Arc<BeaconState>)> {
        self.anchor_on(&self.head, e)
    }

    pub fn view(&self) -> BranchView<'_> {
        BranchView { chain: self, tip: self.head }
    }

    /// Validates `b` against its parent and inserts it.
    pub fn insert(&mut self, ctx: &Ctx, b: BeaconBlock) -> VResult<InsertOutcome> {
        let hash = b.hash();
        if self.entries.contains_key(&hash) {
            return Ok(InsertOutcome { new_head: false, reorg: false });
        }
        let parent_hash = b.header.parent;
        let Some(parent) = self.entries.get(&parent_hash) else { return Err(Reject("unknown parent".into())) };
        let Some(pstate) = parent.state.clone() else { return Err(Reject("parent state pruned".into())) };
        let height = parent.height + 1;
        let view = BranchView { chain: self, tip: parent_hash };
        let state = pstate.apply_block(ctx, &view, parent_hash, &b)?;
        let rank = view
            .anchor(b.slot())
            .and_then(|(_, a)| ranks::rank_of(&ranks::beacon_ranks(&a, b.slot()), &b.header.proposer))
            .unwrap_or(u32::MAX);
        self.entries.insert(
            hash,
            BeaconEntry {
                hash,
                parent: parent_hash,
                slot: Some(b.slot()),
                height,
                block: Some(Arc::new(b)),
                state: Some(Arc::new(state)),
                rank,
            },
        );
        Ok(self.fork_choice(hash))
    }

    /// Whether `a` beats `b` (both known).
    fn better(&self, a: &Hash, b: &Hash) -> bool {
        let (ea, eb) = (&self.entries[a], &self.entries[b]);
        if ea.height != eb.height {
            return ea.height > eb.height;
        }
        // Walk both back to the children of the common ancestor.
        let (mut x, mut y) = (ea, eb);
        while x.parent != y.parent {
            match (self.entries.get(&x.parent), self.entries.get(&y.parent)) {
                (Some(px), Some(py)) => {
                    x = px;
                    y = py;
                }
                _ => break,
            }
        }
        (x.rank, x.hash) < (y.rank, y.hash)
    }

    fn descends_from(&self, h: &Hash, ancestor: &Hash) -> bool {
        let target = &self.entries[ancestor];
        let mut cur = self.entries.get(h);
        while let Some(c) = cur {
            if c.height == target.height {
                return c.hash == target.hash;
            }
            if c.height < target.height {
                return false;
            }
            cur = self.entries.get(&c.parent);
        }
        false
    }

    fn fork_choice(&mut self, candidate: Hash) -> InsertOutcome {
        // The best leaf among the candidate and its known descendants.
        let mut best = candidate;
        for h in self.entries.keys() {
            if self.descends_from(h, &candidate) && self.better(h, &best) {
                best = *h;
            }
        }
        if best == self.head || !self.better(&best, &self.head) {
            return InsertOutcome { new_head: false, reorg: false };
        }
        // Depth finality is not partition safe: after a partition heals, a node
        // follows a strictly longer chain even past its local finality (and
        // counts it), otherwise it would be stuck on the minority fork.
        let fin = self.finalized().hash;
        if !self.descends_from(&best, &fin) {
            if self.entries[&best].height <= self.head().height {
                return InsertOutcome { new_head: false, reorg: false };
            }
            self.finality_reverts += 1;
        }
        let reorg = !self.descends_from(&best, &self.head);
        self.head = best;
        // Rebuild the canonical index.
        let mut path = Vec::new();
        let mut cur = &self.entries[&best];
        loop {
            path.push(cur.hash);
            if self.canonical.get(cur.height as usize) == Some(&cur.hash) && cur.hash != best {
                break;
            }
            match self.entries.get(&cur.parent) {
                Some(p) => cur = p,
                None => break,
            }
        }
        path.reverse();
        let start = self.entries[&path[0]].height as usize;
        self.canonical.truncate(start);
        self.canonical.extend(path);
        InsertOutcome { new_head: true, reorg }
    }

    /// Drops states (and side blocks) older than `keep_slots` before the
    /// finalized block; canonical entries stay as hash links, with their
    /// blocks if `keep_blocks` (to serve syncing peers).
    pub fn prune(&mut self, keep_slots: u64, keep_blocks: bool) {
        let fin = self.finalized();
        let Some(fslot) = fin.slot else { return };
        let cut = fslot.saturating_sub(keep_slots);
        let fin_height = fin.height;
        let canonical: std::collections::HashSet<Hash> = self.canonical.iter().copied().collect();
        self.entries.retain(|h, e| {
            let old = e.slot.is_some_and(|s| s < cut);
            if !old {
                return true;
            }
            // side branches below finality can never become canonical
            canonical.contains(h) || e.height > fin_height
        });
        for e in self.entries.values_mut() {
            if e.slot.is_some_and(|s| s < cut) && canonical.contains(&e.hash) {
                e.state = None;
                if !keep_blocks {
                    e.block = None;
                }
            }
        }
    }
}
