//! `/simu/sync/1`: serving requests and catching up.

use super::*;

const REQ_TIMEOUT_MS: u64 = 10_000;
const MAX_ZONE_BLOCKS: usize = 64;

impl Node {
    pub(super) fn on_request(&mut self, now: u64, _peer: NodeId, req: SyncRequest) -> SyncResponse {
        match req {
            SyncRequest::Status => SyncResponse::Status(self.status_info()),
            SyncRequest::Time => SyncResponse::Time(now),
            SyncRequest::BeaconBlocksBySlot { from, count } => {
                let mut v = Vec::new();
                let mut bytes = 0usize;
                for e in self.chain.canonical() {
                    if e.slot.is_some_and(|s| s >= from)
                        && let Some(b) = &e.block
                    {
                        bytes += borsh::to_vec(b.as_ref()).map(|x| x.len()).unwrap_or(0);
                        v.push((**b).clone());
                        if v.len() >= count.min(MAX_BATCH) as usize || bytes > 8 << 20 {
                            break;
                        }
                    }
                }
                SyncResponse::BeaconBlocks(v)
            }
            SyncRequest::BeaconBlockByHash(h) => SyncResponse::BeaconBlocks(
                self.chain.get(&h).and_then(|e| e.block.as_ref()).map(|b| vec![(**b).clone()]).unwrap_or_default(),
            ),
            SyncRequest::GetCheckpoint { zone, beacon_slot } => {
                SyncResponse::Checkpoint(self.checkpoint_for(zone, beacon_slot).map(Box::new))
            }
            SyncRequest::ZoneBlocks { zone, from, to } => {
                let (Some(eng), Some(e)) = (self.engines.get(&zone), self.e_now(now)) else {
                    return SyncResponse::ZoneBlocks(Vec::new());
                };
                let Some((_, a)) = self.chain.anchor(e) else { return SyncResponse::ZoneBlocks(Vec::new()) };
                let mut v = eng.blocks_in(&a, &self.chain, from, to);
                v.truncate(MAX_ZONE_BLOCKS);
                SyncResponse::ZoneBlocks(v)
            }
            SyncRequest::ZoneHeaders { zone, from, to } => {
                let (Some(eng), Some(e)) = (self.engines.get(&zone), self.e_now(now)) else {
                    return SyncResponse::ZoneHeaders(Vec::new());
                };
                let Some((_, a)) = self.chain.anchor(e) else { return SyncResponse::ZoneHeaders(Vec::new()) };
                SyncResponse::ZoneHeaders(
                    eng.blocks_in(&a, &self.chain, from, to).into_iter().map(|b| b.signed).collect(),
                )
            }
            SyncRequest::GetOutputs { zone, slot } => {
                let hi = slot.unwrap_or(u64::MAX);
                let v: Vec<OutputBundle> =
                    self.inputs.range(zone, 0, hi).into_iter().take(4).map(|b| (**b).clone()).collect();
                SyncResponse::Outputs(v)
            }
            SyncRequest::GetProofs { zone, from_slot, to_slot } => SyncResponse::Proofs(
                self.engines
                    .get(&zone)
                    .map(|e| {
                        e.shares
                            .iter()
                            .filter(|((_, s), _)| *s >= from_slot && *s < to_slot)
                            .map(|(_, v)| v.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
            ),
            SyncRequest::GetProposal(_) => SyncResponse::Proposal(None),
            SyncRequest::GetVotes { zone, proposal_id } => SyncResponse::Votes(
                self.vote_pool
                    .get(&zone)
                    .map(|m| m.iter().filter(|((p, _), _)| *p == proposal_id).map(|(_, v)| v.clone()).collect())
                    .unwrap_or_default(),
            ),
        }
    }

    /// The newest valid commitment of `zone` whose end state this node holds.
    fn checkpoint_for(&self, zone: ZoneId, beacon_slot: Option<u64>) -> Option<Checkpoint> {
        let head = self.head_state();
        let eng = self.engines.get(&zone)?;
        for c in head.commits.get(&zone)?.iter().rev() {
            if c.status == CommitStatus::Invalid || beacon_slot.is_some_and(|e| e != c.e) {
                continue;
            }
            let Some(entry) = eng.get(&c.tip) else { continue };
            let Some(state) = entry.state.as_ref() else { continue };
            return Some(Checkpoint {
                zone,
                beacon_slot: c.e,
                state: (**state).clone(),
                tip: entry.block.as_ref().map(|b| (**b).clone()),
                last_inputs: entry.last_inputs.values().cloned().collect(),
            });
        }
        None
    }

    pub(super) fn on_response(&mut self, now: u64, peer: NodeId, id: u64, resp: SyncResponse, out: &mut Vec<Action>) {
        let Some(req) = self.pending.remove(&id) else { return };
        match resp {
            SyncResponse::Status(s) => {
                self.peers.insert(peer, Some(s));
            }
            SyncResponse::BeaconBlocks(blocks) => {
                let n = blocks.len();
                for b in blocks {
                    self.on_beacon_block(now, b, Some(peer), out);
                }
                // Keep going while the peer is ahead.
                if let SyncRequest::BeaconBlocksBySlot { count, .. } = req.req
                    && n as u32 >= count.min(MAX_BATCH)
                {
                    let from = self.chain.head().slot.map(|s| s + 1).unwrap_or(0);
                    self.request(now, peer, SyncRequest::BeaconBlocksBySlot { from, count }, out);
                }
            }
            SyncResponse::Checkpoint(Some(cp)) => self.install_checkpoint(now, peer, *cp, out),
            SyncResponse::ZoneBlocks(blocks) => {
                for b in blocks {
                    self.on_zone_block(now, Arc::new(b), Some(peer), out);
                }
            }
            SyncResponse::Outputs(v) => {
                for b in v {
                    self.on_output(b);
                }
            }
            _ => {}
        }
    }

    fn install_checkpoint(&mut self, now: u64, peer: NodeId, cp: Checkpoint, out: &mut Vec<Action>) {
        let head = self.head_state().clone();
        let Some(info) = head.commits.get(&cp.zone).and_then(|l| l.iter().find(|c| c.e == cp.beacon_slot)) else {
            return;
        };
        if info.status == CommitStatus::Invalid || bmt::file_address(&cp.state) != info.state_root {
            return;
        }
        if cp.tip.as_ref().is_some_and(|t| t.hash() != info.tip) {
            return;
        }
        let Some(eng) = self.engines.get_mut(&cp.zone) else { return };
        if eng.has_root() {
            return;
        }
        eng.install_checkpoint(&cp, info);
        let to = self.ctx.grid.tick_at(now).unwrap_or(0) / head.zones[&cp.zone].slot_ticks;
        self.request(now, peer, SyncRequest::ZoneBlocks { zone: cp.zone, from: info.to_slot, to }, out);
    }

    /// Peers that follow zone `z` (members or full nodes), by our head state.
    fn peers_for(&self, z: ZoneId) -> Vec<NodeId> {
        let head = self.head_state();
        self.peers.keys().filter(|p| head.can_act_for(z, p)).copied().collect()
    }

    pub(super) fn sync_duties(&mut self, now: u64, e: u64, out: &mut Vec<Action>) {
        self.pending.retain(|_, r| r.at + REQ_TIMEOUT_MS > now);
        if self.peers.is_empty() {
            return;
        }
        // Status polls, one peer per beacon slot.
        if self.last_status_poll < e + 1 {
            self.last_status_poll = e + 1;
            let peers: Vec<NodeId> = self.peers.keys().copied().collect();
            let p = peers[(e as usize) % peers.len()];
            self.request(now, p, SyncRequest::Status, out);
        }
        // Beacon catch-up from a peer that is ahead.
        let my = self.chain.head().slot;
        let ahead = self
            .peers
            .iter()
            .filter_map(|(p, s)| s.as_ref().map(|s| (*p, s.head_slot)))
            .filter(|(_, hs)| hs.is_some_and(|h| my.is_none_or(|m| h > m + 1)))
            .map(|(p, _)| p)
            .next();
        let busy = self.pending.values().any(|r| matches!(r.req, SyncRequest::BeaconBlocksBySlot { .. }));
        if let Some(p) = ahead
            && !busy
        {
            let from = self.chain.finalized().slot.map(|s| s.saturating_sub(16)).unwrap_or(0);
            self.request(now, p, SyncRequest::BeaconBlocksBySlot { from, count: MAX_BATCH }, out);
        }
        // Zones we follow but hold no state for.
        let need: Vec<ZoneId> = self.engines.iter().filter(|(_, e)| !e.has_root()).map(|(z, _)| *z).collect();
        for z in need {
            let busy =
                self.pending.values().any(|r| matches!(r.req, SyncRequest::GetCheckpoint { zone, .. } if zone == z));
            if busy {
                continue;
            }
            let head = self.head_state().clone();
            if head.last_valid(z).is_none() {
                // Rolled back to the start, or not committed yet: the start is computable.
                let zone = head.zones[&z].clone();
                if let Some(eng) = self.engines.get_mut(&z) {
                    let _ = eng.install_start(&self.ctx, &zone);
                }
                continue;
            }
            let peers = self.peers_for(z);
            if let Some(p) = peers.get((e as usize) % peers.len().max(1)) {
                self.request(now, *p, SyncRequest::GetCheckpoint { zone: z, beacon_slot: None }, out);
            }
        }
    }
}
