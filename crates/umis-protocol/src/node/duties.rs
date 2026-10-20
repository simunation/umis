//! Block handling and time-driven duties of a node.

use super::*;
use crate::beacon::BranchView;
use crate::beacon::state::body_roots;

const TX_TTL_SLOTS: u64 = 12;

impl Node {
    // ------------------------------------------------------------ beacon blocks

    pub(super) fn on_beacon_block(
        &mut self,
        now: u64,
        b: BeaconBlock,
        from: Option<NodeId>,
        out: &mut Vec<Action>,
    ) -> Validation {
        let hash = b.hash();
        if self.chain.contains(&hash) {
            return Validation::Ignore;
        }
        if let Some(e) = self.e_now(now)
            && b.slot() > e + 1
        {
            return Validation::Ignore;
        }
        let parent = b.header.parent;
        if !self.chain.contains(&parent) {
            if !self.ctx.verify(&b.header.proposer, &hash, &b.sig) {
                return Validation::Reject;
            }
            let v = self.beacon_orphans.entry(parent).or_default();
            if v.iter().all(|x| x.hash() != hash) && v.len() < 8 {
                v.push(b);
            }
            if let Some(p) = from
                && !self.pending.values().any(|r| r.req == SyncRequest::BeaconBlockByHash(parent))
            {
                self.request(now, p, SyncRequest::BeaconBlockByHash(parent), out);
            }
            return Validation::Accept;
        }
        let mut stack = vec![b];
        let mut result = Validation::Accept;
        let mut first = true;
        let mut new_head = false;
        while let Some(b) = stack.pop() {
            let h = b.hash();
            match self.chain.insert(&self.ctx, b) {
                Ok(o) => {
                    new_head |= o.new_head;
                    if let Some(children) = self.beacon_orphans.remove(&h) {
                        stack.extend(children);
                    }
                }
                Err(e) => {
                    self.stats.rejected_beacon += 1;
                    tracing::debug!(node = %self.me.short(), "beacon block rejected: {e}");
                    if first {
                        result = Validation::Reject;
                    }
                }
            }
            first = false;
        }
        if new_head {
            self.on_new_head(now, out);
        }
        result
    }

    pub(super) fn on_new_head(&mut self, now: u64, out: &mut Vec<Action>) {
        let head = self.chain.head().clone();
        // Drop pooled items the canonical chain already includes.
        let mut cur = Some(head.clone());
        let mut steps = 0;
        while let Some(e) = cur {
            if e.hash == self.last_head || steps > 8 {
                break;
            }
            if let Some(b) = &e.block {
                for t in &b.body.txs {
                    self.tx_pool.remove(&t.tx.address());
                }
            }
            steps += 1;
            cur = self.chain.get(&e.parent).cloned();
        }
        self.last_head = head.hash;
        let e = head.slot.unwrap_or(0);
        self.tx_pool.retain(|_, (at, _)| *at + TX_TTL_SLOTS >= e);
        let st = self.head_state().clone();
        // Commitments that could not be included within a few slots are stuck
        // (e.g. built on a base that lost); dropping them lets members rebuild.
        let stale: Vec<(ZoneId, u64)> = self.zc_pool.keys().filter(|(_, ce)| ce + 4 <= e).copied().collect();
        for k in stale {
            self.zc_pool.remove(&k);
            self.pc_pool.remove(&k);
            if let Some(eng) = self.engines.get_mut(&k.0) {
                eng.sent_commit.remove(&k.1);
                eng.sent_proof.remove(&k.1);
            }
        }
        self.zc_pool.retain(|(z, ce), _| {
            st.commits.get(z).is_some_and(|l| !l.iter().any(|c| c.e >= *ce && c.status != CommitStatus::Invalid))
        });
        self.pc_pool.retain(|(z, ce), _| {
            st.commits.get(z).is_some_and(|l| {
                !l.iter().any(|c| c.e == *ce && c.status != CommitStatus::Optimistic)
                    && !l.iter().any(|c| c.e > *ce && c.status != CommitStatus::Invalid)
            })
        });
        self.refresh_following(now, out);
        out.push(Action::Ui(UiEvent::BeaconHead { slot: e, height: head.height, hash: head.hash.short() }));
        if head.height % 8 == 0 {
            self.chain.prune(self.ctx.p.unbonding + 4 * self.ctx.p.anchor_depth + 8, self.cfg.keep_blocks);
            for eng in self.engines.values_mut() {
                eng.prune(&st, self.cfg.zone_keep_slots);
            }
            let keep_from = e.saturating_sub(4);
            self.proposed.retain(|s| *s >= keep_from);
            self.heartbeat_sent.retain(|s| *s >= keep_from);
            for pool in self.vote_pool.values_mut() {
                pool.retain(|(id, _), _| st.proposals.get(id).is_some_and(|p| p.is_pending()));
            }
        }
    }

    /// Which zones to follow, topics to subscribe and peers to want.
    pub(super) fn refresh_following(&mut self, now: u64, out: &mut Vec<Action>) {
        let head = self.head_state().clone();
        let follow: Vec<ZoneId> = if self.is_full_node() || self.cfg.follow_all {
            head.zones.keys().copied().collect()
        } else {
            self.my_zone().into_iter().collect()
        };
        for z in &follow {
            if self.engines.contains_key(z) {
                continue;
            }
            let mut eng = ZoneEngine::new(*z);
            let zone = &head.zones[z];
            if head.last_valid(*z).is_none()
                && !head.rollback.contains_key(z)
                && let Err(e) = eng.install_start(&self.ctx, zone)
            {
                tracing::warn!("zone {z}: {e}");
            }
            self.engines.insert(*z, eng);
        }
        self.engines.retain(|z, _| follow.contains(z));
        let mut producers = BTreeSet::new();
        for z in &follow {
            for l in &head.zones[z].inputs {
                producers.insert(l.from_zone);
            }
        }
        let mut want: BTreeSet<Topic> = [Topic::BeaconBlock, Topic::BeaconTx, Topic::Commit].into_iter().collect();
        for z in &follow {
            want.insert(Topic::ZoneBlock(*z));
            want.insert(Topic::ZoneProof(*z));
            want.insert(Topic::ZoneVote(*z));
        }
        for p in &producers {
            want.insert(Topic::ZoneOut(*p));
        }
        for t in want.difference(&self.subscribed) {
            out.push(Action::Subscribe(*t));
        }
        for t in self.subscribed.difference(&want) {
            out.push(Action::Unsubscribe(*t));
        }
        self.subscribed = want;
        let peering = (follow, producers.into_iter().collect::<Vec<_>>());
        if peering != self.peering {
            self.peering = peering.clone();
            out.push(Action::Peering { zones: peering.0, consume: peering.1 });
        }
        let _ = now;
    }

    // ------------------------------------------------------------ zone blocks

    pub(super) fn on_zone_block(
        &mut self,
        now: u64,
        b: Arc<ZoneBlock>,
        from: Option<NodeId>,
        out: &mut Vec<Action>,
    ) -> Validation {
        let z = b.header().zone;
        let Some(engine) = self.engines.get_mut(&z) else {
            return if b.signed.verify_sig() { Validation::Accept } else { Validation::Reject };
        };
        if let Some(prev) = engine.note_header(&b.signed) {
            self.pending_faults.push(FaultBody::Equivocation { a: prev, b: b.signed.clone() });
        }
        let mut queue = vec![b];
        let mut result = None;
        while let Some(b) = queue.pop() {
            let hash = b.hash();
            let engine = self.engines.get_mut(&z).expect("engine");
            let outcome = engine.on_block(&self.ctx, &self.chain, b.clone(), true);
            let r = match outcome {
                BlockOutcome::Accepted => {
                    self.after_accept(now, z, hash, out);
                    let engine = self.engines.get_mut(&z).expect("engine");
                    queue.extend(engine.take_orphans(&hash));
                    Validation::Accept
                }
                BlockOutcome::Duplicate => Validation::Ignore,
                BlockOutcome::Orphan { .. } => {
                    if let Some(p) = from {
                        self.request_zone_blocks(now, p, z, b.header().slot, out);
                    }
                    Validation::Accept
                }
                BlockOutcome::Retry(_) => {
                    if self.zone_retry.len() < 64 {
                        self.zone_retry.push((z, b, now));
                    }
                    Validation::Ignore
                }
                BlockOutcome::Invalid { reason, fault } => {
                    self.stats.invalid_blocks += 1;
                    tracing::debug!(node = %self.me.short(), zone = z, "invalid zone block: {reason}");
                    if let Some(f) = fault {
                        self.pending_faults.push(f);
                    }
                    Validation::Reject
                }
            };
            result.get_or_insert(r);
        }
        result.unwrap_or(Validation::Ignore)
    }

    fn request_zone_blocks(&mut self, now: u64, peer: NodeId, z: ZoneId, upto: u64, out: &mut Vec<Action>) {
        let busy = self
            .pending
            .values()
            .any(|r| matches!(r.req, SyncRequest::ZoneBlocks { zone, .. } if zone == z) && r.at + 2000 > now);
        if busy {
            return;
        }
        let head = self.head_state().clone();
        let Some(e) = self.e_now(now) else { return };
        let from = match (self.engines.get(&z), self.chain.anchor(e)) {
            (Some(eng), Some((_, a))) => eng.best_tip(&a, &self.chain).map(|t| t.end).unwrap_or(0),
            _ => head.zones.get(&z).map(|zz| zz.j_start).unwrap_or(0),
        };
        self.request(now, peer, SyncRequest::ZoneBlocks { zone: z, from, to: upto }, out);
    }

    fn withholding(&self, z: ZoneId, j: u64) -> bool {
        let Some((wz, we)) = self.cfg.adversary.withhold_proofs else { return false };
        let Some(zone) = self.head_state().zones.get(&z) else { return false };
        wz == z && self.ctx.timing(zone.slot_ticks).commit_slot_of(j) == we
    }

    fn after_accept(&mut self, _now: u64, z: ZoneId, hash: Hash, out: &mut Vec<Action>) {
        self.stats.zone_blocks_accepted += 1;
        let Some(eng) = self.engines.get_mut(&z) else { return };
        let Some(entry) = eng.get(&hash).cloned() else { return };
        if eng
            .entries
            .values()
            .any(|e| e.hash != hash && e.block.is_some() && e.end == entry.end && e.from_slot == entry.from_slot)
        {
            self.stats.forks += 1;
        }
        if let Some(b) = &entry.block {
            let head = self.chain.head_state();
            let sc = head.zones.get(&z).map(|zz| zz.slot_ticks).unwrap_or(1);
            for r in &b.inputs {
                if let Some(pz) = head.zones.get(&r.src_zone) {
                    let lag = (b.header().from_slot * sc).saturating_sub((r.src_slot + 1) * pz.slot_ticks);
                    self.stats.input_staleness_sum += lag * self.ctx.p.tick_ms;
                    self.stats.input_staleness_n += 1;
                }
            }
        }
        if self.cfg.adversary.withhold_proofs.is_some() && self.withholding(z, entry.end - 1) {
            return;
        }
        let eng = self.engines.get_mut(&z).expect("engine");
        let outs = eng.make_shares(&self.ctx, &self.chain, &self.cfg.key, &hash, false, &mut self.next_job);
        self.dispatch_proofs(z, outs, out);
    }

    /// Publishes ready shares; hands jobs to the runtime.
    pub(super) fn dispatch_proofs(&mut self, z: ZoneId, outs: Vec<crate::zone::ProveOut>, out: &mut Vec<Action>) {
        for o in outs {
            match o {
                crate::zone::ProveOut::Share(s) => {
                    let g = Gossip::ProofShare(Box::new(s));
                    self.publish(Topic::ZoneProof(z), &g, out);
                    self.publish(Topic::ZoneOut(z), &g, out);
                }
                crate::zone::ProveOut::Job(j) => self.job_queue.push((z, *j)),
            }
        }
        self.pump_jobs(out);
    }

    /// Hands queued jobs to the executor while fewer than `max_jobs` run.
    /// Blocks of pooled commitments go first (their proofs have a deadline),
    /// then the zone's best chain, earliest slot first; jobs for blocks that
    /// are neither pooled nor valid any more are dropped.
    pub(super) fn pump_jobs(&mut self, out: &mut Vec<Action>) {
        if self.job_queue.is_empty() {
            return;
        }
        let head = self.chain.head_state().clone();
        let mut committed: BTreeSet<Hash> = BTreeSet::new();
        for (k, zc) in &self.zc_pool {
            if !self.pc_pool.contains_key(k) {
                committed.extend(zc.blocks.iter().map(|b| b.signed.hash()));
            }
        }
        let mut canonical: BTreeSet<Hash> = BTreeSet::new();
        for eng in self.engines.values() {
            let mut cur = eng.best_tip(&head, &self.chain).map(|e| e.hash);
            while let Some(h) = cur {
                if !canonical.insert(h) {
                    break;
                }
                cur = eng.get(&h).filter(|e| !e.root).map(|e| e.parent);
            }
        }
        let mut queue = Vec::new();
        for (z, j) in std::mem::take(&mut self.job_queue) {
            let Some(eng) = self.engines.get_mut(&z) else { continue };
            let h = j.block_hash;
            if committed.contains(&h) || (eng.get(&h).is_some() && eng.valid_under(&h, &head, &self.chain)) {
                queue.push((z, j));
            } else {
                eng.proving.remove(&(h, j.stmt.from_slot));
                self.stats.jobs_dropped += 1;
            }
        }
        queue.sort_by_key(|(z, j)| {
            (!committed.contains(&j.block_hash), !canonical.contains(&j.block_hash), j.stmt.from_slot, *z, j.id)
        });
        let mut queue = queue.into_iter();
        while self.jobs.len() < self.cfg.max_jobs {
            let Some((z, j)) = queue.next() else { break };
            self.jobs.insert(j.id, (z, j.clone()));
            out.push(Action::Compute(Box::new(j)));
        }
        self.job_queue = queue.collect();
    }

    pub(super) fn on_compute_done(&mut self, id: u64, result: Result<Bytes, String>, out: &mut Vec<Action>) {
        let Some((z, job)) = self.jobs.remove(&id) else { return };
        self.pump_jobs(out);
        let Some(eng) = self.engines.get_mut(&z) else { return };
        match result {
            Ok(proof) => {
                self.stats.proved += 1;
                let s = eng.job_done(&job, proof);
                let g = Gossip::ProofShare(Box::new(s));
                self.publish(Topic::ZoneProof(z), &g, out);
                self.publish(Topic::ZoneOut(z), &g, out);
            }
            Err(e) => {
                eng.proving.remove(&(job.block_hash, job.stmt.from_slot));
                tracing::warn!(zone = z, slot = job.stmt.from_slot, "proving failed: {e}");
            }
        }
    }

    // ------------------------------------------------------------ faults

    fn report_faults(&mut self, now: u64, out: &mut Vec<Action>) {
        let bodies = std::mem::take(&mut self.pending_faults);
        for body in bodies {
            let (minter, zone, slot) = match &body {
                FaultBody::Equivocation { a, .. } => (a.header.minter, a.header.zone, a.header.slot),
                FaultBody::InvalidTransition { block, .. } | FaultBody::InvalidInputRef { block, .. } => {
                    (block.header().minter, block.header().zone, block.header().slot)
                }
            };
            let key = (minter, zone, slot);
            if minter == self.me || self.reported.contains(&key) || self.head_state().punished.contains_key(&key) {
                continue;
            }
            self.reported.insert(key);
            let record = FaultEvidence {
                kind: body.kind(),
                zone,
                zone_slot: slot,
                minter,
                reporter: self.me,
                evidence_root: bmt::file_address(&borsh::to_vec(&body).expect("borsh")),
            };
            self.stats.faults_reported += 1;
            out.push(Action::Ui(UiEvent::Fault { kind: body.kind(), zone, slot, minter: minter.short() }));
            let t = self.sign_tx(Tx::Fault(FaultTx { record, body }));
            self.submit_tx(now, t, out);
        }
    }

    // ------------------------------------------------------------ duties

    pub(super) fn duties(&mut self, now: u64, out: &mut Vec<Action>) {
        let Some(e) = self.e_now(now) else { return };
        if !self.zone_retry.is_empty() {
            let retry = std::mem::take(&mut self.zone_retry);
            for (z, b, at) in retry {
                if at + 30_000 < now {
                    continue;
                }
                let before = self.zone_retry.len();
                self.on_zone_block(now, b, None, out);
                if let Some(last) = self.zone_retry.get_mut(before) {
                    last.2 = at;
                }
                let _ = z;
            }
        }
        self.report_faults(now, out);
        self.maybe_propose(now, e, out);
        self.maybe_heartbeat(now, e, out);
        let zones: Vec<ZoneId> = self.engines.keys().copied().collect();
        for z in zones {
            self.zone_duties(now, e, z, out);
        }
        self.vote_duties(e, out);
        self.sync_duties(now, e, out);
        self.pump_jobs(out);
    }

    fn maybe_heartbeat(&mut self, now: u64, e: u64, out: &mut Vec<Action>) {
        if !self.cfg.send_heartbeats || self.heartbeat_sent.contains(&e) || !self.head_state().is_active(&self.me) {
            return;
        }
        if now < self.heartbeat_time(e) {
            return;
        }
        self.heartbeat_sent.insert(e);
        let t = self.sign_tx(Tx::Heartbeat(Heartbeat { beacon_slot: e, node: self.me }));
        self.submit_tx(now, t, out);
    }

    fn heartbeat_time(&self, e: u64) -> u64 {
        let half = self.ctx.grid.beacon_slot_ms() / 2;
        let off = u64::from_be_bytes(self.me.0[..8].try_into().expect("8 bytes")) % half.max(1);
        self.ctx.grid.beacon_slot_start_ms(e) + off
    }

    fn my_beacon_rank(&self, e: u64) -> Option<u32> {
        let (_, a) = self.chain.anchor(e)?;
        ranks::rank_of(&ranks::beacon_ranks(&a, e), &self.me).filter(|r| *r <= self.ctx.p.max_beacon_rank)
    }

    fn maybe_propose(&mut self, now: u64, e: u64, out: &mut Vec<Action>) {
        if self.proposed.contains(&e) || self.chain.head().slot.is_some_and(|s| s >= e) {
            return;
        }
        let Some(r) = self.my_beacon_rank(e) else { return };
        if now < self.ctx.grid.beacon_rank_start_ms(e, r, self.ctx.p.max_beacon_rank) {
            return;
        }
        self.proposed.insert(e);
        let Some((block, refused)) = self.build_block(e) else { return };
        for h in refused {
            self.tx_pool.remove(&h);
        }
        let g = Gossip::BeaconBlock(Box::new(block.clone()));
        match self.chain.insert(&self.ctx, block) {
            Ok(o) => {
                self.stats.proposed += 1;
                self.publish(Topic::BeaconBlock, &g, out);
                if o.new_head {
                    self.on_new_head(now, out);
                }
            }
            Err(err) => tracing::warn!(node = %self.me.short(), "own block rejected: {err}"),
        }
    }

    fn tx_order(t: &Tx) -> u8 {
        match t {
            Tx::Fault(_) => 0,
            Tx::Grant(_) => 1,
            Tx::Join(_) => 2,
            Tx::Heartbeat(_) => 3,
            Tx::ProposeZone(_) => 4,
            Tx::Leave(_) => 5,
        }
    }

    /// Builds a block; also returns proposals that were refused (dropped
    /// from the pool: a refusal is final).
    fn build_block(&self, e: u64) -> Option<(BeaconBlock, Vec<Hash>)> {
        let parent = self.chain.head().clone();
        let pstate = parent.state.clone()?;
        let view = BranchView { chain: &self.chain, tip: parent.hash };
        let seed_sig = self.cfg.key.sign(&SeedMsg { slot: e }.address());
        let seed = umis_core::keccak256_concat(&[&pstate.seed.0, &seed_sig.0]);
        let ok = |body: &BeaconBody| pstate.apply_body(&self.ctx, &view, e, seed, body).is_ok();
        let mut body = BeaconBody::default();
        let mut txs: Vec<SignedTx> = self.tx_pool.values().map(|(_, t)| t.clone()).collect();
        txs.sort_by_key(|t| (Self::tx_order(&t.tx), t.tx.address()));
        let mut refused = Vec::new();
        body.txs = txs.clone();
        if !ok(&body) {
            body.txs.clear();
            for t in txs {
                body.txs.push(t);
                if !ok(&body) {
                    let t = body.txs.pop().expect("pushed");
                    if matches!(t.tx, Tx::ProposeZone(_)) {
                        refused.push(t.tx.address());
                    }
                }
            }
        }
        let need_pc = self.ctx.p.proof_lag == 0;
        for ((z, ce), zc) in &self.zc_pool {
            if *ce >= e {
                continue;
            }
            let pc = self.pc_pool.get(&(*z, *ce));
            if need_pc && pc.is_none() {
                continue;
            }
            body.zone_commitments.push(zc.clone());
            if let Some(pc) = pc {
                body.proof_commitments.push(pc.clone());
            }
            if let Err(err) = pstate.apply_body(&self.ctx, &view, e, seed, &body) {
                tracing::debug!(node = %self.me.short(), zone = z, commit = ce, "commitment not included: {err}");
                body.zone_commitments.pop();
                if pc.is_some() {
                    body.proof_commitments.pop();
                }
            }
        }
        // Proofs for commitments already on chain (proof lag L ≥ 1).
        let included: BTreeSet<(ZoneId, u64)> =
            body.proof_commitments.iter().map(|p| (p.pc.zone, p.pc.beacon_slot)).collect();
        for (key, pc) in &self.pc_pool {
            if included.contains(key) {
                continue;
            }
            let optimistic = pstate
                .commits
                .get(&key.0)
                .is_some_and(|l| l.iter().any(|c| c.e == key.1 && c.status == CommitStatus::Optimistic));
            if !optimistic {
                continue;
            }
            let mut trial = body.clone();
            trial.proof_commitments.push(pc.clone());
            trial.proof_commitments.sort_by_key(|p| (p.pc.zone, p.pc.beacon_slot));
            match pstate.apply_body(&self.ctx, &view, e, seed, &trial) {
                Ok(_) => body = trial,
                Err(err) => tracing::debug!(zone = key.0, commit = key.1, "proof commitment not included: {err}"),
            }
        }
        let state = pstate.apply_body(&self.ctx, &view, e, seed, &body).ok()?;
        let (txs_root, zones_root, proofs_root) = body_roots(&body);
        let header = BeaconBlockHeader {
            slot: e,
            tx_count: body.txs.len() as u32,
            zc_count: body.zone_commitments.len() as u16,
            pc_count: body.proof_commitments.len() as u16,
            parent: parent.hash,
            seed,
            proposer: self.me,
            txs_root,
            zones_root,
            proofs_root,
            state_root: state.hash(),
        };
        let sig = self.cfg.key.sign(&header.address());
        Some((BeaconBlock { header, sig, seed_sig, body }, refused))
    }

    fn zone_duties(&mut self, now: u64, e: u64, z: ZoneId, out: &mut Vec<Action>) {
        let head = self.head_state().clone();
        let Some(zone) = head.zones.get(&z).cloned() else { return };
        let t = self.ctx.timing(zone.slot_ticks);
        let Some(tick) = self.ctx.grid.tick_at(now) else { return };
        let j = tick / zone.slot_ticks;
        if j < zone.j_start || !self.engines.get(&z).is_some_and(|x| x.has_root()) {
            return;
        }
        // Minting.
        if let Some((_, a)) = self.chain.anchor(t.beacon_slot_of(j)) {
            let list = ranks::zone_ranks(&a, z, j);
            if let Some(r) = ranks::rank_of(&list, &self.me).filter(|r| *r <= self.ctx.p.max_zone_rank) {
                let at =
                    self.ctx.zone_slot_start(zone.slot_ticks, j) + r as u64 * self.ctx.zone_window_ms(zone.slot_ticks);
                let eng = &self.engines[&z];
                if now >= at && !eng.minted.contains(&j) && !eng.has_block_for(j, &a, &self.chain) {
                    self.mint(now, z, j, out);
                }
            }
        }
        // Commitment fallback for recent commit slots.
        for ce in e.saturating_sub(3)..e {
            if !t.is_commit_slot(ce) || ce < zone.active_from {
                continue;
            }
            if self.engines[&z].sent_commit.contains(&ce) || self.zc_pool.contains_key(&(z, ce)) {
                continue;
            }
            if head.commits[&z].iter().any(|c| c.e >= ce && c.status != CommitStatus::Invalid) {
                continue;
            }
            let last = t.j_star(ce) - 1;
            let Some((_, a)) = self.chain.anchor(t.beacon_slot_of(last)) else { continue };
            let pos = ranks::rank_of(&ranks::zone_ranks(&a, z, last), &self.me).unwrap_or(64) as u64;
            let slot_ms = self.ctx.grid.beacon_slot_ms();
            let at = self.ctx.grid.beacon_slot_start_ms(ce + 1) + slot_ms / 8 + pos * slot_ms / 16;
            if now >= at {
                self.commit_now(z, ce, out);
            }
        }
        // Proof commitments for pooled zone commitments.
        let keys: Vec<(ZoneId, u64)> =
            self.zc_pool.keys().filter(|(zz, _)| *zz == z).copied().filter(|k| !self.pc_pool.contains_key(k)).collect();
        for key in keys {
            let ce = key.1;
            if self.cfg.adversary.withhold_proofs == Some(key) {
                continue;
            }
            let zc = self.zc_pool[&key].clone();
            let mine = zc.commitment.signer == self.me;
            let slot_ms = self.ctx.grid.beacon_slot_ms();
            let t_next = self.ctx.grid.beacon_slot_start_ms(ce + 1);
            if !mine && now < t_next + slot_ms / 4 {
                continue;
            }
            let self_prove = now >= t_next + slot_ms / 8;
            let eng = self.engines.get_mut(&z).expect("engine");
            if let Some(pc) = eng.build_proof_commitment(&self.ctx, &self.chain, &self.cfg.key, &zc, self_prove) {
                eng.sent_proof.insert(ce);
                tracing::debug!(
                    zone = z,
                    ce,
                    leaves = pc.leaves.len(),
                    after_ms = now as i64 - t_next as i64,
                    "proof commitment"
                );
                self.pc_pool.insert(key, pc.clone());
                self.publish(Topic::Commit, &Gossip::ProofCommit(Box::new(pc)), out);
            } else if self_prove && self.ctx.backend.is_async() && mine {
                // Missing shares: prove them ourselves (jobs run off the loop).
                let mut outs = Vec::new();
                for cb in &zc.blocks {
                    outs.extend(eng.make_shares(
                        &self.ctx,
                        &self.chain,
                        &self.cfg.key,
                        &cb.signed.hash(),
                        true,
                        &mut self.next_job,
                    ));
                }
                self.dispatch_proofs(z, outs, out);
            }
        }
    }

    fn mint(&mut self, now: u64, z: ZoneId, j: u64, out: &mut Vec<Action>) {
        let adv = self.cfg.adversary.clone();
        let variant = if adv.corrupt_at == Some((z, j)) {
            MintVariant::CorruptState
        } else if adv.forge_input_at == Some((z, j)) {
            MintVariant::ForgeInput
        } else {
            MintVariant::Honest
        };
        let votes: Vec<SignedVote> = self.vote_pool.get(&z).map(|m| m.values().cloned().collect()).unwrap_or_default();
        let eng = self.engines.get_mut(&z).expect("engine");
        let Some((block, ex)) = eng.mint(&self.ctx, &self.chain, &self.cfg.key, &self.inputs, &votes, j, variant)
        else {
            return;
        };
        self.stats.minted += 1;
        out.push(Action::Ui(UiEvent::Minted { zone: z, slot: j }));
        let bundle = exec::output_bundle(&block.signed, &ex.outputs, &block.inputs);
        self.publish(Topic::ZoneBlock(z), &Gossip::ZoneBlock(Box::new((*block).clone())), out);
        self.publish(Topic::ZoneOut(z), &Gossip::Output(Box::new(bundle)), out);
        if adv.equivocate_at == Some((z, j)) {
            let eng = self.engines.get_mut(&z).expect("engine");
            eng.minted.remove(&j);
            // Rebuild on the same parent with held inputs: a second, different block.
            if let Some(entry) = eng.get(&block.hash()).cloned() {
                eng.entries.remove(&entry.hash);
            }
            if let Some((b2, ex2)) =
                eng.mint(&self.ctx, &self.chain, &self.cfg.key, &self.inputs, &[], j, MintVariant::HoldInputs)
                && b2.hash() != block.hash()
            {
                let bundle2 = exec::output_bundle(&b2.signed, &ex2.outputs, &b2.inputs);
                self.publish(Topic::ZoneBlock(z), &Gossip::ZoneBlock(Box::new((*b2).clone())), out);
                self.publish(Topic::ZoneOut(z), &Gossip::Output(Box::new(bundle2)), out);
            }
            // Keep the first block as ours.
            let eng = self.engines.get_mut(&z).expect("engine");
            let _ = eng.on_block(&self.ctx, &self.chain, block.clone(), true);
        }
        if variant == MintVariant::Honest && !self.withholding(z, j) {
            let eng = self.engines.get_mut(&z).expect("engine");
            let outs = eng.make_shares(&self.ctx, &self.chain, &self.cfg.key, &block.hash(), false, &mut self.next_job);
            self.dispatch_proofs(z, outs, out);
        }
        // The minter of a commit period's last slot is its delegate.
        let head = self.head_state().clone();
        let t = self.ctx.timing(head.zones[&z].slot_ticks);
        let ce = t.commit_slot_of(j);
        if t.j_star(ce) == j + 1 && variant != MintVariant::ForgeInput {
            self.commit_now(z, ce, out);
        }
        let _ = now;
    }

    fn commit_now(&mut self, z: ZoneId, ce: u64, out: &mut Vec<Action>) {
        if self.engines[&z].sent_commit.contains(&ce) {
            return;
        }
        let pending: Vec<(&ZoneCommitEnvelope, Option<&ProofCommitEnvelope>)> = self
            .zc_pool
            .iter()
            .filter(|((zz, e), _)| *zz == z && *e < ce)
            .map(|(k, v)| (v, self.pc_pool.get(k)))
            .collect();
        let e_next = self.chain.head().slot.map(|s| s + 1).unwrap_or(0).max(ce + 1);
        let projected = self.head_state().with_pending(&self.ctx, &self.chain.view(), e_next, &pending);
        let eng = self.engines.get_mut(&z).expect("engine");
        let Some((env, cp)) = eng.build_commitment(&self.ctx, &self.chain, &projected, &self.cfg.key, ce) else {
            return;
        };
        eng.sent_commit.insert(ce);
        self.zc_pool.entry((z, ce)).or_insert_with(|| env.clone());
        self.publish(Topic::Commit, &Gossip::ZoneCommit(Box::new(env), Some(cp)), out);
    }

    fn vote_duties(&mut self, e: u64, out: &mut Vec<Action>) {
        let Some(z) = self.my_zone() else { return };
        let Some((_, a)) = self.chain.anchor(e) else { return };
        let mut todo = Vec::new();
        for p in a.pending_proposals() {
            if self.voted.contains(&p.id) || !p.voters.get(&z).is_some_and(|v| v.contains(&self.me)) {
                continue;
            }
            match self.cfg.vote_policy {
                VotePolicy::Auto => todo.push((p.id, true)),
                VotePolicy::RejectAll => todo.push((p.id, false)),
                VotePolicy::Manual => {}
            }
        }
        for (id, accept) in todo {
            self.cast_vote(z, id, accept, out);
        }
        // Drop pooled votes that can no longer be recorded.
        if let Some(pool) = self.vote_pool.get_mut(&z) {
            pool.retain(|_, v| {
                check_vote(&self.ctx, &a, z, v).is_ok() || !a.proposals.contains_key(&v.vote.proposal_id)
            });
        }
    }

    pub(super) fn next_wake(&self, now: u64) -> u64 {
        let mut next = now + 1000;
        let g = &self.ctx.grid;
        let mut consider = |t: u64| {
            if t > now && t < next {
                next = t;
            }
        };
        let Some(e) = self.e_now(now) else {
            consider(g.genesis_ms);
            return next;
        };
        let slot_ms = g.beacon_slot_ms();
        for ee in [e, e + 1] {
            if let Some(r) = self.my_beacon_rank(ee) {
                consider(g.beacon_rank_start_ms(ee, r, self.ctx.p.max_beacon_rank));
            }
            consider(self.heartbeat_time(ee));
            let start = g.beacon_slot_start_ms(ee);
            for k in 0..8 {
                consider(start + slot_ms / 8 + k * slot_ms / 16);
            }
        }
        let head = self.head_state();
        for z in self.engines.keys() {
            let Some(zone) = head.zones.get(z) else { continue };
            let t = self.ctx.timing(zone.slot_ticks);
            let tick = g.tick_at(now).unwrap_or(0);
            let j = tick / zone.slot_ticks;
            for jj in [j, j + 1] {
                let start = self.ctx.zone_slot_start(zone.slot_ticks, jj);
                consider(start);
                if let Some((_, a)) = self.chain.anchor(t.beacon_slot_of(jj))
                    && let Some(r) = ranks::rank_of(&ranks::zone_ranks(&a, *z, jj), &self.me)
                    && r <= self.ctx.p.max_zone_rank
                {
                    consider(start + r as u64 * self.ctx.zone_window_ms(zone.slot_ticks));
                }
            }
        }
        next
    }
}
