//! The simulated network: virtual clock, message queue with seeded
//! latencies and drops, partitions, offline nodes. Gossip goes from the
//! publisher to every reachable subscriber (an idealized mesh).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};
use std::sync::Arc;
use umis_core::NodeId;
use umis_protocol::types::{SyncRequest, SyncResponse};
use umis_protocol::{Action, Event, Node, Topic};

/// xorshift64*: tiny, deterministic.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo + 1)
    }
}

#[derive(Debug, Clone)]
pub struct NetModel {
    pub latency_ms: (u64, u64),
    /// Per-delivery drop probability in per mille.
    pub drop_permille: u64,
}

impl Default for NetModel {
    fn default() -> Self {
        Self { latency_ms: (20, 120), drop_permille: 0 }
    }
}

#[derive(Debug, Clone)]
enum Msg {
    Gossip { topic: Topic, data: Arc<Vec<u8>>, from: NodeId, id: u64 },
    Request { from: usize, gid: u64, req: SyncRequest },
    Response { from: NodeId, local_id: u64, resp: SyncResponse },
    Failed { peer: NodeId, local_id: u64 },
    Computed { id: u64, result: Result<Vec<u8>, String> },
}

pub struct SimNode {
    pub node: Node,
    pub online: bool,
    pub group: u32,
    pub wake: u64,
    pub subs: BTreeSet<Topic>,
    /// Clock skew (ms) of this node.
    pub skew: i64,
    pub label: String,
}

/// Extra delay for messages matching (sender, topic) — used to stage races.
#[derive(Debug, Clone)]
pub struct DelayRule {
    pub from: NodeId,
    pub topic: Topic,
    pub extra_ms: u64,
    pub until_ms: u64,
}

pub struct SimNet {
    pub now: u64,
    pub nodes: Vec<SimNode>,
    pub index: HashMap<NodeId, usize>,
    pub model: NetModel,
    pub rng: Rng,
    pub delays: Vec<DelayRule>,
    queue: BinaryHeap<Reverse<(u64, u64, usize)>>,
    msgs: HashMap<u64, Msg>,
    seq: u64,
    gid: u64,
    requests: HashMap<u64, (usize, u64)>,
    pub delivered: u64,
    pub events: u64,
    /// Simulated proving time of async backends.
    pub prove_delay_ms: u64,
    /// Wall time spent per event kind (profiling).
    pub cost: std::collections::BTreeMap<String, (u64, std::time::Duration)>,
}

impl SimNet {
    pub fn new(seed: u64, model: NetModel) -> Self {
        Self {
            now: 0,
            nodes: Vec::new(),
            index: HashMap::new(),
            model,
            rng: Rng::new(seed),
            delays: Vec::new(),
            queue: BinaryHeap::new(),
            msgs: HashMap::new(),
            seq: 0,
            gid: 0,
            requests: HashMap::new(),
            delivered: 0,
            events: 0,
            prove_delay_ms: 500,
            cost: Default::default(),
        }
    }

    pub fn add(&mut self, node: Node, label: String) -> usize {
        let i = self.nodes.len();
        self.index.insert(node.me, i);
        let skew = (self.rng.range(0, 60) as i64) - 30;
        self.nodes.push(SimNode { node, online: true, group: 0, wake: 0, subs: BTreeSet::new(), skew, label });
        i
    }

    fn reachable(&self, a: usize, b: usize) -> bool {
        a != b && self.nodes[a].online && self.nodes[b].online && self.nodes[a].group == self.nodes[b].group
    }

    fn local_time(&self, i: usize) -> u64 {
        (self.now as i64 + self.nodes[i].skew).max(0) as u64
    }

    fn enqueue(&mut self, at: u64, to: usize, m: Msg) {
        self.seq += 1;
        self.msgs.insert(self.seq, m);
        self.queue.push(Reverse((at, self.seq, to)));
    }

    fn latency(&mut self) -> u64 {
        let (lo, hi) = self.model.latency_ms;
        self.rng.range(lo, hi)
    }

    /// Connects every pair of reachable nodes (start, partition changes).
    pub fn connect_all(&mut self) {
        let n = self.nodes.len();
        for i in 0..n {
            for j in 0..n {
                if self.reachable(i, j) {
                    let peer = self.nodes[j].node.me;
                    self.deliver(i, Event::PeerConnected(peer));
                }
            }
        }
    }

    pub fn set_online(&mut self, i: usize, online: bool) {
        if self.nodes[i].online == online {
            return;
        }
        let me = self.nodes[i].node.me;
        if !online {
            for j in 0..self.nodes.len() {
                if self.reachable(i, j) {
                    self.deliver(j, Event::PeerDisconnected(me));
                }
            }
            self.nodes[i].online = false;
        } else {
            self.nodes[i].online = true;
            for j in 0..self.nodes.len() {
                if self.reachable(i, j) {
                    let pj = self.nodes[j].node.me;
                    self.deliver(i, Event::PeerConnected(pj));
                    self.deliver(j, Event::PeerConnected(me));
                }
            }
        }
    }

    /// Moves nodes into partition groups; cross-group links drop.
    pub fn partition(&mut self, groups: &BTreeMap<usize, u32>) {
        let n = self.nodes.len();
        let before: Vec<Vec<bool>> = (0..n).map(|i| (0..n).map(|j| self.reachable(i, j)).collect()).collect();
        for (i, g) in groups {
            self.nodes[*i].group = *g;
        }
        for i in 0..n {
            for j in 0..n {
                let now = self.reachable(i, j);
                if before[i][j] && !now && self.nodes[i].online {
                    let pj = self.nodes[j].node.me;
                    self.deliver(i, Event::PeerDisconnected(pj));
                } else if !before[i][j] && now {
                    let pj = self.nodes[j].node.me;
                    self.deliver(i, Event::PeerConnected(pj));
                }
            }
        }
    }

    /// Calls the node and executes its actions.
    pub fn deliver(&mut self, i: usize, ev: Event) {
        if !self.nodes[i].online {
            return;
        }
        self.events += 1;
        let t = self.local_time(i);
        let kind = match &ev {
            Event::Gossip { topic, .. } => format!("gossip {}", topic.path()),
            Event::Tick => "tick".into(),
            Event::Request { .. } => "request".into(),
            Event::Response { .. } => "response".into(),
            _ => "other".into(),
        };
        let t0 = std::time::Instant::now();
        let actions = self.nodes[i].node.handle(t, ev);
        let c = self.cost.entry(kind).or_default();
        c.0 += 1;
        c.1 += t0.elapsed();
        self.run_actions(i, actions);
    }

    fn run_actions(&mut self, i: usize, actions: Vec<Action>) {
        let me = self.nodes[i].node.me;
        for a in actions {
            match a {
                Action::Publish { topic, data } => {
                    let data = Arc::new(data);
                    self.gid += 1;
                    let id = self.gid;
                    let extra: u64 = self
                        .delays
                        .iter()
                        .filter(|d| d.from == me && d.topic == topic && self.now < d.until_ms)
                        .map(|d| d.extra_ms)
                        .sum();
                    for j in 0..self.nodes.len() {
                        if !self.reachable(i, j) || !self.nodes[j].subs.contains(&topic) {
                            continue;
                        }
                        if self.model.drop_permille > 0 && self.rng.range(0, 999) < self.model.drop_permille {
                            continue;
                        }
                        let at = self.now + self.latency() + extra;
                        self.enqueue(at, j, Msg::Gossip { topic, data: data.clone(), from: me, id });
                    }
                }
                Action::Request { peer, id, req } => {
                    let Some(&j) = self.index.get(&peer) else { continue };
                    let at = self.now + self.latency();
                    if self.reachable(i, j) {
                        self.gid += 1;
                        let gid = self.gid;
                        self.requests.insert(gid, (i, id));
                        self.enqueue(at, j, Msg::Request { from: i, gid, req });
                    } else {
                        self.enqueue(at, i, Msg::Failed { peer, local_id: id });
                    }
                }
                Action::Respond { id, resp } => {
                    if let Some((src, local_id)) = self.requests.remove(&id) {
                        let at = self.now + self.latency();
                        if self.reachable(i, src) {
                            self.enqueue(at, src, Msg::Response { from: me, local_id, resp });
                        }
                    }
                }
                Action::Subscribe(t) => {
                    self.nodes[i].subs.insert(t);
                }
                Action::Unsubscribe(t) => {
                    self.nodes[i].subs.remove(&t);
                }
                Action::WakeAt(t) => {
                    // node time → sim time
                    self.nodes[i].wake = (t as i64 - self.nodes[i].skew).max(0) as u64;
                }
                Action::Compute(job) => {
                    // Proving runs "in a worker": inline here, reported after a delay.
                    let n = &self.nodes[i].node;
                    let result = umis_proof::run_job(&n.ctx.tasks, n.ctx.backend.as_ref(), &n.cfg.key, &job)
                        .map_err(|e| e.to_string());
                    let at = self.now + self.prove_delay_ms;
                    self.enqueue(at, i, Msg::Computed { id: job.id, result });
                }
                Action::Validate { .. } | Action::Peering { .. } | Action::Ui(_) => {}
            }
        }
    }

    /// Runs until simulated time `t_end`.
    pub fn run_until(&mut self, t_end: u64) {
        loop {
            let next_msg = self.queue.peek().map(|Reverse((t, _, _))| *t);
            let next_wake = self.nodes.iter().enumerate().filter(|(_, n)| n.online).map(|(i, n)| (n.wake, i)).min();
            let t_msg = next_msg.unwrap_or(u64::MAX);
            let t_wake = next_wake.map(|w| w.0).unwrap_or(u64::MAX);
            let t = t_msg.min(t_wake);
            if t > t_end {
                self.now = t_end;
                return;
            }
            self.now = self.now.max(t);
            if t_msg <= t_wake {
                let Reverse((_, seq, to)) = self.queue.pop().expect("peeked");
                let m = self.msgs.remove(&seq).expect("message");
                if !self.nodes[to].online {
                    continue;
                }
                self.delivered += 1;
                match m {
                    Msg::Gossip { topic, data, from, id } => {
                        let data = Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone());
                        self.deliver(to, Event::Gossip { topic, data, from, msg_id: id.to_be_bytes().to_vec() });
                    }
                    Msg::Request { from, gid, req } => {
                        let peer = self.nodes[from].node.me;
                        self.deliver(to, Event::Request { peer, id: gid, req });
                    }
                    Msg::Response { from, local_id, resp } => {
                        self.deliver(to, Event::Response { peer: from, id: local_id, resp })
                    }
                    Msg::Failed { peer, local_id } => self.deliver(to, Event::RequestFailed { peer, id: local_id }),
                    Msg::Computed { id, result } => self.deliver(to, Event::ComputeDone { id, result }),
                }
            } else {
                let i = next_wake.expect("wake").1;
                // Guard against a node asking to be woken in the past forever.
                self.nodes[i].wake = u64::MAX;
                self.deliver(i, Event::Tick);
                if self.nodes[i].wake <= self.now {
                    self.nodes[i].wake = self.now + 1;
                }
            }
        }
    }
}
