//! The runtime: drives the sans-IO `umis_protocol::Node` with the libp2p
//! adapter (`umis_net::Net`), the wall clock and wake-up timers. The same
//! loop runs natively (tokio) and in the browser (wasm-bindgen-futures); it
//! only uses `futures`, `futures-timer` and `web-time`.

use borsh::BorshDeserialize;
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use std::collections::HashMap;
use std::time::Duration;
use umis_core::NodeId;
use umis_net::libp2p::gossipsub::{MessageAcceptance, MessageId};
use umis_net::libp2p::request_response::OutboundRequestId;
use umis_net::libp2p::{PeerId, identity};
use umis_net::{Net, NetEvent, Topics, Want};
use umis_protocol::node::snapshot::Snapshot;
use umis_protocol::types::{SyncRequest, SyncResponse, Topic};
use umis_protocol::{Action, Command, Event, Node, UiEvent, Validation};

/// Wall clock in Unix ms.
pub fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn peer_of(n: &NodeId) -> Option<PeerId> {
    let pk = identity::ed25519::PublicKey::try_from_bytes(&n.0).ok()?;
    Some(identity::PublicKey::from(pk).to_peer_id())
}

pub fn node_of(p: &PeerId) -> Option<NodeId> {
    let mh = p.as_ref();
    let pk = identity::PublicKey::try_decode_protobuf(mh.digest()).ok()?;
    let ed = pk.try_into_ed25519().ok()?;
    Some(NodeId(ed.to_bytes()))
}

/// Requests from the owner of the runtime (CLI, HTTP handlers, the web UI).
pub enum Control {
    Command(Command),
    Snapshot(oneshot::Sender<Snapshot>),
    Render(u32, oneshot::Sender<Option<umis_task::RenderGrid>>),
    NetStatus(oneshot::Sender<umis_net::NetStatus>),
    SetVotePolicy(umis_protocol::VotePolicy),
    Lattice(u64, u64, oneshot::Sender<umis_protocol::node::snapshot::LatticeView>),
    Stop,
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Control>,
}

impl Handle {
    pub fn command(&self, c: Command) {
        let _ = self.tx.unbounded_send(Control::Command(c));
    }

    pub async fn snapshot(&self) -> Option<Snapshot> {
        let (tx, rx) = oneshot::channel();
        self.tx.unbounded_send(Control::Snapshot(tx)).ok()?;
        rx.await.ok()
    }

    pub async fn render(&self, zone: u32) -> Option<umis_task::RenderGrid> {
        let (tx, rx) = oneshot::channel();
        self.tx.unbounded_send(Control::Render(zone, tx)).ok()?;
        rx.await.ok().flatten()
    }

    pub async fn net_status(&self) -> Option<umis_net::NetStatus> {
        let (tx, rx) = oneshot::channel();
        self.tx.unbounded_send(Control::NetStatus(tx)).ok()?;
        rx.await.ok()
    }

    pub async fn lattice(&self, from: u64, to: u64) -> Option<umis_protocol::node::snapshot::LatticeView> {
        let (tx, rx) = oneshot::channel();
        self.tx.unbounded_send(Control::Lattice(from, to, tx)).ok()?;
        rx.await.ok()
    }

    pub fn set_vote_policy(&self, p: umis_protocol::VotePolicy) {
        let _ = self.tx.unbounded_send(Control::SetVotePolicy(p));
    }

    pub fn stop(&self) {
        let _ = self.tx.unbounded_send(Control::Stop);
    }
}

/// Runs proving jobs off the event loop and reports through `done`.
pub trait Executor {
    fn submit(&self, job: umis_proof::ProveJob, done: mpsc::UnboundedSender<(u64, Result<Vec<u8>, String>)>);
}

/// Runs jobs on a small dedicated rayon pool (native). The prover is
/// data-parallel; giving it every core would starve the event loop (and,
/// on a one-machine devnet, the other nodes), making minters miss their
/// windows. `UMIS_PROVER_THREADS` sets the pool size (default 2).
#[cfg(not(target_arch = "wasm32"))]
pub struct ThreadExecutor {
    runner: std::sync::Arc<umis_proof::JobRunner>,
    pool: std::sync::Arc<rayon::ThreadPool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ThreadExecutor {
    pub fn new(
        tasks: umis_task::TaskRegistry,
        backend: std::sync::Arc<dyn umis_proof::ProofBackend>,
        key: umis_core::SecretKey,
    ) -> Self {
        let threads = std::env::var("UMIS_PROVER_THREADS").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("prover pool");
        let runner = std::sync::Arc::new(umis_proof::JobRunner::new(tasks, backend, key));
        Self { runner, pool: std::sync::Arc::new(pool) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Executor for ThreadExecutor {
    fn submit(&self, job: umis_proof::ProveJob, done: mpsc::UnboundedSender<(u64, Result<Vec<u8>, String>)>) {
        let runner = self.runner.clone();
        let queued = std::time::Instant::now();
        self.pool.spawn(move || {
            let started = std::time::Instant::now();
            let r = runner.run(&job).map_err(|e| e.to_string());
            tracing::debug!(
                job = job.id,
                slot = job.stmt.from_slot,
                wait_ms = (started - queued).as_millis() as u64,
                prove_ms = started.elapsed().as_millis() as u64,
                "proof job done"
            );
            let _ = done.unbounded_send((job.id, r));
        });
    }
}

pub struct Runtime {
    pub node: Node,
    pub net: Net,
    executor: Option<Box<dyn Executor>>,
    done_tx: mpsc::UnboundedSender<(u64, Result<Vec<u8>, String>)>,
    done_rx: mpsc::UnboundedReceiver<(u64, Result<Vec<u8>, String>)>,
    topics: Topics,
    pending: HashMap<OutboundRequestId, u64>,
    wake: u64,
    rx: mpsc::UnboundedReceiver<Control>,
    ui: Option<mpsc::UnboundedSender<UiEvent>>,
}

impl Runtime {
    pub fn new(node: Node, net: Net) -> (Self, Handle) {
        let (tx, rx) = mpsc::unbounded();
        let topics = Topics::new(node.ctx.genesis.genesis8());
        let (done_tx, done_rx) = mpsc::unbounded();
        let rt = Self {
            node,
            net,
            executor: None,
            done_tx,
            done_rx,
            topics,
            pending: HashMap::new(),
            wake: 0,
            rx,
            ui: None,
        };
        (rt, Handle { tx })
    }

    /// Where proving jobs run; without one they run inline (blocking the loop).
    pub fn set_executor(&mut self, e: Box<dyn Executor>) {
        self.executor = Some(e);
    }

    /// Receives the node's UI events.
    pub fn ui_events(&mut self) -> mpsc::UnboundedReceiver<UiEvent> {
        let (tx, rx) = mpsc::unbounded();
        self.ui = Some(tx);
        rx
    }

    fn full_topic(&self, t: Topic) -> String {
        format!("/simu/{}/{}", self.topics.genesis8(), t.path())
    }

    fn parse_topic(&self, s: &str) -> Option<Topic> {
        let prefix = format!("/simu/{}/", self.topics.genesis8());
        Topic::parse(s.strip_prefix(&prefix)?)
    }

    fn feed(&mut self, ev: Event) {
        let kind = event_kind(&ev);
        let t0 = now_ms();
        let actions = self.node.handle(t0, ev);
        let took = now_ms().saturating_sub(t0);
        if took >= 100 {
            tracing::debug!(event = kind, took_ms = took, "slow event");
        }
        self.apply(actions);
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Publish { topic, data } => {
                    let t = self.full_topic(topic);
                    self.net.publish(&t, data);
                }
                Action::Validate { msg_id, from, result } => {
                    let Some(p) = peer_of(&from) else { continue };
                    let acc = match result {
                        Validation::Accept => MessageAcceptance::Accept,
                        Validation::Ignore => MessageAcceptance::Ignore,
                        Validation::Reject => MessageAcceptance::Reject,
                    };
                    self.net.report(&MessageId::from(msg_id), &p, acc);
                }
                Action::Request { peer, id, req } => {
                    let Some(p) = peer_of(&peer) else { continue };
                    let rid = self.net.request(&p, borsh::to_vec(&req).expect("borsh"));
                    self.pending.insert(rid, id);
                }
                Action::Respond { id, resp } => self.net.respond(id, borsh::to_vec(&resp).expect("borsh")),
                Action::Subscribe(t) => {
                    let t = self.full_topic(t);
                    self.net.subscribe(&t);
                }
                Action::Unsubscribe(t) => {
                    let t = self.full_topic(t);
                    self.net.unsubscribe(&t);
                }
                Action::Peering { zones, consume } => {
                    let mut register = vec![self.topics.ns_all()];
                    let mut want = vec![Want { namespace: self.topics.ns_all(), target: 4 }];
                    for z in &zones {
                        register.push(self.topics.ns_zone(*z));
                        want.push(Want { namespace: self.topics.ns_zone(*z), target: 6 });
                    }
                    for z in &consume {
                        if !zones.contains(z) {
                            want.push(Want { namespace: self.topics.ns_zone(*z), target: 1 });
                        }
                    }
                    self.net.set_register(register);
                    self.net.set_want(want);
                }
                Action::WakeAt(t) => self.wake = t,
                Action::Compute(job) => match &self.executor {
                    Some(e) => e.submit(*job, self.done_tx.clone()),
                    None => {
                        let n = &self.node;
                        let r = umis_proof::run_job(&n.ctx.tasks, n.ctx.backend.as_ref(), &n.cfg.key, &job)
                            .map_err(|e| e.to_string());
                        let _ = self.done_tx.unbounded_send((job.id, r));
                    }
                },
                Action::Ui(u) => {
                    if let Some(tx) = &self.ui {
                        let _ = tx.unbounded_send(u);
                    }
                }
            }
        }
    }

    fn on_net(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Gossip { topic, data, id, from, .. } => {
                let (Some(t), Some(f)) = (self.parse_topic(&topic), node_of(&from)) else {
                    self.net.report(&id, &from, MessageAcceptance::Ignore);
                    return;
                };
                self.feed(Event::Gossip { topic: t, data, from: f, msg_id: id.0 });
            }
            NetEvent::Request { peer, id, data } => {
                let Some(n) = node_of(&peer) else { return };
                match SyncRequest::try_from_slice(&data) {
                    Ok(req) => self.feed(Event::Request { peer: n, id, req }),
                    Err(_) => {
                        self.net.respond(id, borsh::to_vec(&SyncResponse::Error("bad request".into())).expect("borsh"))
                    }
                }
            }
            NetEvent::Response { peer, id, data } => {
                let (Some(local), Some(n)) = (self.pending.remove(&id), node_of(&peer)) else { return };
                if let Ok(resp) = SyncResponse::try_from_slice(&data) {
                    self.feed(Event::Response { peer: n, id: local, resp });
                }
            }
            NetEvent::RequestFailed { peer, id, error } => {
                tracing::warn!(%peer, "sync request failed: {error}");
                if let (Some(local), Some(n)) = (self.pending.remove(&id), node_of(&peer)) {
                    self.feed(Event::RequestFailed { peer: n, id: local });
                }
            }
            NetEvent::PeerConnected { peer, .. } => {
                if let Some(n) = node_of(&peer) {
                    self.feed(Event::PeerConnected(n));
                }
            }
            NetEvent::PeerDisconnected { peer } => {
                if let Some(n) = node_of(&peer) {
                    self.feed(Event::PeerDisconnected(n));
                }
            }
            _ => {}
        }
    }

    /// Runs until `Control::Stop` or the handle is dropped.
    pub async fn run(mut self) {
        self.feed(Event::Tick);
        loop {
            let now = now_ms();
            let delay = Duration::from_millis(self.wake.saturating_sub(now).clamp(1, 1000));
            let mut timer = futures_timer::Delay::new(delay);
            // Biased: queued network events and finished jobs before timers, so a
            // busy node never acts on a stale view (e.g. fallback-mints a slot
            // whose block is already waiting in the queue).
            futures::select_biased! {
                ev = futures::FutureExt::fuse(self.net.next_event()) => self.on_net(ev),
                d = self.done_rx.next() => if let Some((id, result)) = d {
                    self.feed(Event::ComputeDone { id, result });
                },
                c = self.rx.next() => match c {
                    Some(Control::Command(c)) => self.feed(Event::Command(c)),
                    Some(Control::Snapshot(tx)) => { let _ = tx.send(self.node.snapshot(now_ms())); }
                    Some(Control::Render(z, tx)) => { let _ = tx.send(self.node.zone_render(now_ms(), z)); }
                    Some(Control::NetStatus(tx)) => { let _ = tx.send(self.net.status()); }
                    Some(Control::SetVotePolicy(p)) => self.node.cfg.vote_policy = p,
                    Some(Control::Lattice(a, b, tx)) => { let _ = tx.send(self.node.lattice(a, b)); }
                    Some(Control::Stop) | None => break,
                },
                _ = futures::FutureExt::fuse(&mut timer) => self.feed(Event::Tick),
            }
        }
    }
}

fn event_kind(ev: &Event) -> String {
    match ev {
        Event::Gossip { topic, .. } => format!("gossip {topic:?}"),
        Event::Tick => "tick".into(),
        Event::Request { .. } => "request".into(),
        Event::Response { .. } => "response".into(),
        Event::RequestFailed { .. } => "request failed".into(),
        Event::PeerConnected(_) | Event::PeerDisconnected(_) => "peer".into(),
        Event::Command(_) => "command".into(),
        Event::ComputeDone { .. } => "compute done".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_peer_round_trip() {
        let k = umis_core::SecretKey::from_label("x");
        let p = peer_of(&k.node_id()).unwrap();
        assert_eq!(node_of(&p), Some(k.node_id()));
        let mut b = k.to_bytes();
        let kp = identity::Keypair::ed25519_from_bytes(&mut b).unwrap();
        assert_eq!(kp.public().to_peer_id(), p, "the protocol key is the libp2p identity");
    }
}
