//! The sans-IO node: `handle(now_ms, Event) -> Vec<Action>`.
//!
//! Every call processes the event, then runs the node's duties for the
//! current time (beacon proposals, zone minting, commitments, proofs,
//! heartbeats, votes, sync) and ends with a `WakeAt` action naming the next
//! time something is due.

use crate::beacon::BeaconChain;
use crate::beacon::state::{BeaconState, CommitStatus, NodeKind};
use crate::ctx::Ctx;
use crate::exec;
use crate::ranks;
use crate::types::*;
use crate::zone::{BlockOutcome, InputStore, MintVariant, ZoneEngine, check_vote};
use borsh::BorshDeserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use umis_core::records::*;
use umis_core::{Hash, NodeId, SecretKey, SegRecord, bmt};

mod duties;
pub mod snapshot;
mod sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VotePolicy {
    Auto,
    Manual,
    RejectAll,
}

/// Misbehaviour for adversary and fault scenarios.
#[derive(Debug, Clone, Default)]
pub struct Adversary {
    /// Mint a block with a wrong state root at `(zone, slot)`.
    pub corrupt_at: Option<(ZoneId, u64)>,
    /// Mint two different blocks at `(zone, slot)`.
    pub equivocate_at: Option<(ZoneId, u64)>,
    /// Record a non-causal InputRef at `(zone, slot)`.
    pub forge_input_at: Option<(ZoneId, u64)>,
    /// Never prove or submit proof commitments for `(zone, commit slot)`.
    pub withhold_proofs: Option<(ZoneId, u64)>,
}

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub key: SecretKey,
    pub vote_policy: VotePolicy,
    /// The devnet faucet key, if this node serves grants.
    pub faucet: Option<SecretKey>,
    pub adversary: Adversary,
    /// Keep old beacon blocks to serve syncing peers.
    pub keep_blocks: bool,
    pub send_heartbeats: bool,
    /// Zone slots of history kept behind the oldest retained commitment.
    pub zone_keep_slots: u64,
    /// Follow (validate) every zone even without being a member or full
    /// node — for observers such as the bootstrap serving the 3D view.
    pub follow_all: bool,
    /// Proving jobs handed to the executor at once; the rest wait in a
    /// queue that runs the most urgent first and drops stale ones.
    pub max_jobs: usize,
}

impl NodeConfig {
    pub fn new(key: SecretKey) -> Self {
        Self {
            key,
            vote_policy: VotePolicy::Auto,
            faucet: None,
            adversary: Adversary::default(),
            keep_blocks: true,
            send_heartbeats: true,
            zone_keep_slots: 64,
            follow_all: false,
            max_jobs: usize::MAX,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Lock `stake` and join as a zone member or a full node.
    Join {
        stake: u64,
        full_node: bool,
    },
    Leave,
    /// Faucet only: credit test stake.
    Grant {
        to: NodeId,
        amount: u64,
    },
    Propose(Box<ProposeZoneTx>),
    Vote {
        proposal_id: Hash,
        accept: bool,
    },
    /// Submit a tx signed elsewhere (e.g. a proposal built by the CLI).
    SubmitTx(Box<SignedTx>),
}

#[derive(Debug, Clone)]
pub enum Event {
    Tick,
    Gossip {
        topic: Topic,
        data: Vec<u8>,
        from: NodeId,
        msg_id: Vec<u8>,
    },
    Request {
        peer: NodeId,
        id: u64,
        req: SyncRequest,
    },
    Response {
        peer: NodeId,
        id: u64,
        resp: SyncResponse,
    },
    RequestFailed {
        peer: NodeId,
        id: u64,
    },
    PeerConnected(NodeId),
    PeerDisconnected(NodeId),
    Command(Command),
    /// A proving job finished (`Action::Compute`).
    ComputeDone {
        id: u64,
        result: Result<Bytes, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validation {
    Accept,
    Ignore,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum UiEvent {
    BeaconHead { slot: u64, height: u64, hash: String },
    Minted { zone: ZoneId, slot: u64 },
    Fault { kind: u8, zone: ZoneId, slot: u64, minter: String },
    Log(String),
}

#[derive(Debug, Clone)]
pub enum Action {
    Publish {
        topic: Topic,
        data: Vec<u8>,
    },
    Validate {
        msg_id: Vec<u8>,
        from: NodeId,
        result: Validation,
    },
    Request {
        peer: NodeId,
        id: u64,
        req: SyncRequest,
    },
    Respond {
        id: u64,
        resp: SyncResponse,
    },
    Subscribe(Topic),
    Unsubscribe(Topic),
    /// Rendezvous/peering hints: zones we serve and zones we consume from.
    Peering {
        zones: Vec<ZoneId>,
        consume: Vec<ZoneId>,
    },
    WakeAt(u64),
    Ui(UiEvent),
    /// Run a proving job off the event loop and report `Event::ComputeDone`.
    Compute(Box<umis_proof::ProveJob>),
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub minted: u64,
    pub proposed: u64,
    pub faults_reported: u64,
    pub invalid_blocks: u64,
    pub rejected_beacon: u64,
    pub zone_blocks_accepted: u64,
    pub input_staleness_sum: u64,
    pub input_staleness_n: u64,
    /// Blocks accepted for a zone slot that already had one (fallback races).
    pub forks: u64,
    /// Slot proofs this node made (async backends) and queued jobs dropped
    /// because their block went stale.
    pub proved: u64,
    pub jobs_dropped: u64,
    /// Gossip received per topic (diagnostics).
    pub gossip_in: BTreeMap<String, u64>,
}

#[derive(Debug, Clone)]
struct PendingReq {
    req: SyncRequest,
    at: u64,
}

pub struct Node {
    pub ctx: Arc<Ctx>,
    pub cfg: NodeConfig,
    pub me: NodeId,
    pub chain: BeaconChain,
    pub engines: BTreeMap<ZoneId, ZoneEngine>,
    pub inputs: InputStore,
    pub stats: Stats,
    beacon_orphans: BTreeMap<Hash, Vec<BeaconBlock>>,
    tx_pool: BTreeMap<Hash, (u64, SignedTx)>,
    zc_pool: BTreeMap<(ZoneId, u64), ZoneCommitEnvelope>,
    pc_pool: BTreeMap<(ZoneId, u64), ProofCommitEnvelope>,
    vote_pool: BTreeMap<ZoneId, BTreeMap<(Hash, NodeId), SignedVote>>,
    voted: BTreeSet<Hash>,
    reported: BTreeSet<(NodeId, ZoneId, u64)>,
    pending_faults: Vec<FaultBody>,
    jobs: BTreeMap<u64, (ZoneId, umis_proof::ProveJob)>,
    job_queue: Vec<(ZoneId, umis_proof::ProveJob)>,
    next_job: u64,
    subscribed: BTreeSet<Topic>,
    peering: (Vec<ZoneId>, Vec<ZoneId>),
    proposed: BTreeSet<u64>,
    heartbeat_sent: BTreeSet<u64>,
    peers: BTreeMap<NodeId, Option<StatusInfo>>,
    next_req: u64,
    pending: BTreeMap<u64, PendingReq>,
    zone_retry: Vec<(ZoneId, Arc<ZoneBlock>, u64)>,
    nonce: u64,
    last_head: Hash,
    last_status_poll: u64,
    started: bool,
}

const MAX_BATCH: u32 = 32;

impl Node {
    pub fn new(ctx: Arc<Ctx>, cfg: NodeConfig) -> Result<Self, String> {
        let chain = BeaconChain::new(&ctx)?;
        let me = cfg.key.node_id();
        Ok(Self {
            last_head: chain.head().hash,
            ctx,
            cfg,
            me,
            chain,
            engines: BTreeMap::new(),
            inputs: InputStore::default(),
            stats: Stats::default(),
            beacon_orphans: BTreeMap::new(),
            tx_pool: BTreeMap::new(),
            zc_pool: BTreeMap::new(),
            pc_pool: BTreeMap::new(),
            vote_pool: BTreeMap::new(),
            voted: BTreeSet::new(),
            reported: BTreeSet::new(),
            pending_faults: Vec::new(),
            jobs: BTreeMap::new(),
            job_queue: Vec::new(),
            next_job: 0,
            subscribed: BTreeSet::new(),
            peering: (Vec::new(), Vec::new()),
            proposed: BTreeSet::new(),
            heartbeat_sent: BTreeSet::new(),
            peers: BTreeMap::new(),
            next_req: 1,
            pending: BTreeMap::new(),
            zone_retry: Vec::new(),
            nonce: 0,
            last_status_poll: 0,
            started: false,
        })
    }

    pub fn head_state(&self) -> &Arc<BeaconState> {
        self.chain.head_state()
    }

    pub fn is_full_node(&self) -> bool {
        self.head_state()
            .nodes
            .get(&self.me)
            .is_some_and(|n| n.kind == NodeKind::FullNode && self.head_state().is_active(&self.me))
    }

    pub fn my_zone(&self) -> Option<ZoneId> {
        let s = self.head_state();
        s.nodes.get(&self.me).filter(|_| s.is_active(&self.me)).and_then(|n| n.zone)
    }

    /// Pooled zone / proof commitments (debugging and UI).
    pub fn zc_pool_keys(&self) -> Vec<(ZoneId, u64, bool)> {
        self.zc_pool.keys().map(|k| (k.0, k.1, self.pc_pool.contains_key(k))).collect()
    }

    pub fn tx_pool_len(&self) -> usize {
        self.tx_pool.len()
    }

    pub fn status_info(&self) -> StatusInfo {
        let h = self.chain.head();
        let f = self.chain.finalized();
        StatusInfo {
            head_slot: h.slot,
            head_hash: h.hash,
            finalized_slot: f.slot,
            finalized_hash: f.hash,
            zone: self.my_zone(),
            full_node: self.is_full_node(),
        }
    }

    fn e_now(&self, now: u64) -> Option<u64> {
        self.ctx.grid.beacon_slot_at(now)
    }

    // ------------------------------------------------------------ entry point

    pub fn handle(&mut self, now: u64, ev: Event) -> Vec<Action> {
        let mut out = Vec::new();
        if !self.started {
            self.started = true;
            self.refresh_following(now, &mut out);
        }
        match ev {
            Event::Tick => {}
            Event::Gossip { topic, data, from, msg_id } => {
                let result = self.on_gossip(now, topic, &data, from, &mut out);
                out.push(Action::Validate { msg_id, from, result });
            }
            Event::Request { peer, id, req } => {
                let resp = self.on_request(now, peer, req);
                out.push(Action::Respond { id, resp });
            }
            Event::Response { peer, id, resp } => self.on_response(now, peer, id, resp, &mut out),
            Event::RequestFailed { id, .. } => {
                self.pending.remove(&id);
            }
            Event::PeerConnected(p) => {
                self.peers.insert(p, None);
                self.request(now, p, SyncRequest::Status, &mut out);
            }
            Event::PeerDisconnected(p) => {
                self.peers.remove(&p);
            }
            Event::Command(c) => self.on_command(now, c, &mut out),
            Event::ComputeDone { id, result } => self.on_compute_done(id, result, &mut out),
        }
        self.duties(now, &mut out);
        out.push(Action::WakeAt(self.next_wake(now)));
        out
    }

    fn publish(&self, topic: Topic, g: &Gossip, out: &mut Vec<Action>) {
        out.push(Action::Publish { topic, data: borsh::to_vec(g).expect("borsh gossip") });
    }

    fn request(&mut self, now: u64, peer: NodeId, req: SyncRequest, out: &mut Vec<Action>) {
        let id = self.next_req;
        self.next_req += 1;
        self.pending.insert(id, PendingReq { req: req.clone(), at: now });
        out.push(Action::Request { peer, id, req });
    }

    fn sign_tx(&self, tx: Tx) -> SignedTx {
        let sig = self.cfg.key.sign(&tx.address());
        SignedTx { tx, sig }
    }

    fn submit_tx(&mut self, now: u64, t: SignedTx, out: &mut Vec<Action>) {
        let e = self.e_now(now).unwrap_or(0);
        self.tx_pool.insert(t.tx.address(), (e, t.clone()));
        self.publish(Topic::BeaconTx, &Gossip::Tx(Box::new(t)), out);
    }

    // ------------------------------------------------------------ commands

    fn on_command(&mut self, now: u64, c: Command, out: &mut Vec<Action>) {
        match c {
            Command::Join { stake, full_node } => {
                self.nonce += 1;
                let tx = Tx::Join(Join { stake, kind: full_node as u8, nonce: self.nonce, node: self.me });
                let t = self.sign_tx(tx);
                self.submit_tx(now, t, out);
            }
            Command::Leave => {
                self.nonce += 1;
                let t = self.sign_tx(Tx::Leave(Leave { nonce: self.nonce, node: self.me }));
                self.submit_tx(now, t, out);
            }
            Command::Grant { to, amount } => {
                let Some(f) = self.cfg.faucet.clone() else { return };
                self.nonce += 1;
                let tx = Tx::Grant(Grant { amount, nonce: now * 1000 + self.nonce, to });
                let sig = f.sign(&tx.address());
                self.submit_tx(now, SignedTx { tx, sig }, out);
            }
            Command::Propose(p) => {
                let t = self.sign_tx(Tx::ProposeZone(*p));
                if self.on_tx(now, t.clone()) == Validation::Accept {
                    self.publish(Topic::BeaconTx, &Gossip::Tx(Box::new(t)), out);
                } else {
                    out.push(Action::Ui(UiEvent::Log("zone proposal refused".into())));
                }
            }
            Command::SubmitTx(t) => {
                if self.on_tx(now, (*t).clone()) == Validation::Accept {
                    self.publish(Topic::BeaconTx, &Gossip::Tx(t), out);
                } else {
                    out.push(Action::Ui(UiEvent::Log("submitted tx refused".into())));
                }
            }
            Command::Vote { proposal_id, accept } => {
                if let Some(z) = self.my_zone() {
                    self.cast_vote(z, proposal_id, accept, out);
                }
            }
        }
    }

    fn cast_vote(&mut self, z: ZoneId, proposal_id: Hash, accept: bool, out: &mut Vec<Action>) {
        if !self.voted.insert(proposal_id) {
            return;
        }
        let vote = ZoneVote { zone: z, accept, proposal_id, voter: self.me };
        let sv = SignedVote { sig: self.cfg.key.sign(&vote.address()), vote };
        self.vote_pool.entry(z).or_default().insert((proposal_id, self.me), sv.clone());
        self.publish(Topic::ZoneVote(z), &Gossip::Vote(Box::new(sv)), out);
    }

    // ------------------------------------------------------------ gossip

    fn on_gossip(&mut self, now: u64, topic: Topic, data: &[u8], from: NodeId, out: &mut Vec<Action>) -> Validation {
        *self.stats.gossip_in.entry(topic.path()).or_default() += 1;
        let Ok(g) = Gossip::try_from_slice(data) else { return Validation::Reject };
        match (topic, g) {
            (Topic::BeaconBlock, Gossip::BeaconBlock(b)) => self.on_beacon_block(now, *b, Some(from), out),
            (Topic::BeaconTx, Gossip::Tx(t)) => self.on_tx(now, *t),
            (Topic::Commit, Gossip::ZoneCommit(zc, _cp)) => self.on_zone_commit(*zc),
            (Topic::Commit, Gossip::ProofCommit(pc)) => self.on_proof_commit(*pc),
            (Topic::ZoneBlock(z), Gossip::ZoneBlock(b)) if b.header().zone == z => {
                self.on_zone_block(now, Arc::new(*b), Some(from), out)
            }
            (Topic::ZoneProof(z) | Topic::ZoneOut(z), Gossip::ProofShare(s)) if s.leaf.zone == z => {
                match self.engines.get_mut(&z) {
                    Some(e) => {
                        if e.on_share(&self.ctx, &self.chain, *s) {
                            Validation::Accept
                        } else {
                            Validation::Ignore
                        }
                    }
                    None => Validation::Accept,
                }
            }
            (Topic::ZoneOut(z), Gossip::Output(b)) if b.producer.header.zone == z => self.on_output(*b),
            (Topic::ZoneVote(z), Gossip::Vote(v)) if v.vote.zone == z => {
                if !v.verify_sig() {
                    return Validation::Reject;
                }
                self.vote_pool.entry(z).or_default().insert((v.vote.proposal_id, v.vote.voter), *v);
                Validation::Accept
            }
            _ => Validation::Reject,
        }
    }

    fn on_tx(&mut self, now: u64, t: SignedTx) -> Validation {
        let signer = t.tx.signer(&self.ctx.genesis.faucet);
        if !self.ctx.verify(&signer, &t.tx.address(), &t.sig) {
            return Validation::Reject;
        }
        if matches!(t.tx, Tx::ProposeZone(_)) {
            // A refused proposal stays refused: check it now, never pool it.
            let head = self.chain.head();
            let e_next = head.slot.map(|s| s + 1).unwrap_or(0).max(self.e_now(now).unwrap_or(0));
            let body = BeaconBody { txs: vec![t.clone()], ..Default::default() };
            let st = self.head_state().clone();
            if st.apply_body(&self.ctx, &self.chain.view(), e_next, st.seed, &body).is_err() {
                return Validation::Reject;
            }
        }
        let e = self.e_now(now).unwrap_or(0);
        if self.tx_pool.insert(t.tx.address(), (e, t)).is_some() {
            return Validation::Ignore;
        }
        Validation::Accept
    }

    fn on_zone_commit(&mut self, zc: ZoneCommitEnvelope) -> Validation {
        let c = &zc.commitment;
        if !self.ctx.verify(&c.signer, &zc.address(), &zc.sig) {
            return Validation::Reject;
        }
        let key = (c.zone, c.beacon_slot);
        if self.zc_pool.contains_key(&key) {
            return Validation::Ignore;
        }
        self.zc_pool.insert(key, zc);
        Validation::Accept
    }

    fn on_proof_commit(&mut self, pc: ProofCommitEnvelope) -> Validation {
        if !self.ctx.verify(&pc.pc.signer, &pc.pc.address(), &pc.sig) {
            return Validation::Reject;
        }
        let key = (pc.pc.zone, pc.pc.beacon_slot);
        if self.pc_pool.contains_key(&key) {
            return Validation::Ignore;
        }
        self.pc_pool.insert(key, pc);
        Validation::Accept
    }

    fn on_output(&mut self, b: OutputBundle) -> Validation {
        let h = &b.producer.header;
        let head = self.head_state().clone();
        if crate::beacon::state::check_producer(&self.ctx, &self.chain.view(), &head, &b.producer).is_err() {
            return Validation::Ignore;
        }
        for (i, p) in b.ports.iter().enumerate() {
            if !p.proof.verify(&h.outputs_root, &bmt::file_address(&p.payload).0, i) {
                return Validation::Reject;
            }
        }
        if exec::inputs_commitment(&b.inputs) != h.inputs_commitment {
            return Validation::Reject;
        }
        if let Some(e) = self.engines.get_mut(&h.zone)
            && let Some(prev) = e.note_header(&b.producer)
        {
            let body = FaultBody::Equivocation { a: prev, b: b.producer.clone() };
            self.pending_faults.push(body);
        }
        if self.inputs.insert(b) { Validation::Accept } else { Validation::Ignore }
    }
}
