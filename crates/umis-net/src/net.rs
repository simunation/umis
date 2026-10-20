//! `Net`: owns the swarm and runs the peering policy (bootstrap dialing, relay
//! reservations, rendezvous registration and discovery, connection targets per
//! namespace). Everything app-level surfaces as a [`NetEvent`].
//!
//! `next_event` is cancellation safe, so a runtime can `select!` over it.

use crate::behaviour::{Behaviour, BehaviourEvent};
use crate::config::{NetConfig, Role, Want};
use crate::swarm::build_swarm;
use futures::StreamExt;
use futures_timer::Delay;
use libp2p::gossipsub::{self, IdentTopic, MessageAcceptance, MessageId};
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, OutboundRequestId, ResponseChannel};
use libp2p::swarm::SwarmEvent;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::{Multiaddr, PeerId, Swarm, identify, relay, rendezvous};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::time::Duration;
use web_time::Instant;

const TICK: Duration = Duration::from_millis(1000);
const REDIAL_BOOTSTRAP: Duration = Duration::from_secs(5);
const REDIAL_PEER: Duration = Duration::from_secs(15);
const DISCOVER_FAST: Duration = Duration::from_secs(3);
const DISCOVER_SLOW: Duration = Duration::from_secs(10);
const FAST_PHASE: Duration = Duration::from_secs(40);

#[derive(Debug, Clone)]
pub enum NetEvent {
    Listening(Multiaddr),
    PeerConnected {
        peer: PeerId,
        addr: Multiaddr,
        relayed: bool,
        outbound: bool,
    },
    PeerDisconnected {
        peer: PeerId,
    },
    Gossip {
        topic: String,
        data: Vec<u8>,
        id: MessageId,
        /// The peer that forwarded the message to us.
        from: PeerId,
        /// The original publisher.
        source: Option<PeerId>,
    },
    Request {
        peer: PeerId,
        id: u64,
        data: Vec<u8>,
    },
    Response {
        peer: PeerId,
        id: OutboundRequestId,
        data: Vec<u8>,
    },
    RequestFailed {
        peer: PeerId,
        id: OutboundRequestId,
        error: String,
    },
    Discovered {
        namespace: String,
        peer: PeerId,
        addrs: Vec<Multiaddr>,
    },
    Registered {
        namespace: String,
    },
    ReservationAccepted {
        relay: PeerId,
    },
    TopicPeer {
        peer: PeerId,
        topic: String,
        subscribed: bool,
    },
}

#[derive(Debug, Clone)]
pub struct PeerConn {
    pub addr: Multiaddr,
    pub relayed: bool,
    pub outbound: bool,
    conns: usize,
}

#[derive(Debug, Default, Clone)]
struct KnownPeer {
    addrs: Vec<Multiaddr>,
    namespaces: BTreeSet<String>,
    last_dial: Option<Instant>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NetStatus {
    pub peer_id: String,
    pub role: Role,
    pub listen: Vec<String>,
    pub external: Vec<String>,
    pub peers: Vec<(String, String, bool)>,
    pub topics: Vec<String>,
}

pub struct Net {
    swarm: Swarm<Behaviour>,
    cfg: NetConfig,
    local: PeerId,
    bootstrap: HashMap<PeerId, Vec<Multiaddr>>,
    bootstrap_last_dial: HashMap<PeerId, Instant>,
    circuits_requested: HashSet<PeerId>,
    connected: HashMap<PeerId, PeerConn>,
    known: HashMap<PeerId, KnownPeer>,
    /// (rendezvous node, namespace) -> time the registration must be refreshed.
    registered: HashMap<(PeerId, String), Instant>,
    last_discover: Option<Instant>,
    responses: HashMap<u64, ResponseChannel<Vec<u8>>>,
    next_inbound: u64,
    subscribed: BTreeSet<String>,
    tick: Delay,
    queue: VecDeque<NetEvent>,
    started: Instant,
}

fn peer_of(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().last().and_then(|p| match p {
        Protocol::P2p(id) => Some(id),
        _ => None,
    })
}

fn is_circuit(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| matches!(p, Protocol::P2pCircuit))
}

fn is_webrtc_direct(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| matches!(p, Protocol::WebRTCDirect))
}

fn is_ws(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| matches!(p, Protocol::Ws(_) | Protocol::Wss(_)))
}

fn is_quic(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| matches!(p, Protocol::QuicV1))
}

fn is_plain_tcp(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| matches!(p, Protocol::Tcp(_))) && !is_ws(addr)
}

fn is_unspecified(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| match p {
        Protocol::Ip4(ip) => ip.is_unspecified(),
        Protocol::Ip6(ip) => ip.is_unspecified(),
        _ => false,
    })
}

/// The best class of addresses this role can dial, in preference order:
/// direct transports first, relay circuits last. Browsers prefer WebSocket
/// unless `prefer_webrtc` (see `NetConfig::prefer_webrtc`).
pub fn dialable_with(role: Role, prefer_webrtc: bool, addrs: &[Multiaddr]) -> Vec<Multiaddr> {
    if role == Role::Browser && !prefer_webrtc {
        let classes: &[fn(&Multiaddr) -> bool] = &[
            |a| is_ws(a) && !is_circuit(a),
            |a| is_webrtc_direct(a) && !is_circuit(a),
            |a| is_ws(a) && is_circuit(a),
            |a| is_webrtc_direct(a) && is_circuit(a),
        ];
        for class in classes {
            let picked: Vec<Multiaddr> = addrs.iter().filter(|a| class(a)).cloned().collect();
            if !picked.is_empty() {
                return picked;
            }
        }
        return Vec::new();
    }
    dialable(role, addrs)
}

/// [`dialable_with`] preferring WebRTC-direct for browsers.
pub fn dialable(role: Role, addrs: &[Multiaddr]) -> Vec<Multiaddr> {
    let classes: &[fn(&Multiaddr) -> bool] = match role {
        // A relay advertises a circuit address per relay transport; dial only
        // the one we would also pick to reach the relay itself.
        Role::Browser => &[
            |a| is_webrtc_direct(a) && !is_circuit(a),
            |a| is_ws(a) && !is_circuit(a),
            |a| is_webrtc_direct(a) && is_circuit(a),
            |a| is_ws(a) && is_circuit(a),
        ],
        Role::Node | Role::Bootstrap => &[
            |a| is_quic(a) && !is_circuit(a),
            |a| is_plain_tcp(a) && !is_circuit(a),
            |a| is_webrtc_direct(a) && !is_circuit(a),
            |a| is_quic(a) && is_circuit(a),
            |a| is_plain_tcp(a) && is_circuit(a),
            is_circuit,
        ],
    };
    for class in classes {
        let picked: Vec<Multiaddr> = addrs.iter().filter(|a| class(a)).cloned().collect();
        if !picked.is_empty() {
            return picked;
        }
    }
    Vec::new()
}

impl Net {
    pub async fn new(cfg: NetConfig) -> anyhow::Result<Self> {
        let mut swarm = build_swarm(&cfg).await?;
        let local = *swarm.local_peer_id();
        for addr in &cfg.listen {
            swarm.listen_on(addr.clone())?;
        }
        for addr in &cfg.announce {
            swarm.add_external_address(addr.clone());
        }
        let mut bootstrap = HashMap::new();
        for addr in &cfg.bootstrap {
            match peer_of(addr) {
                Some(p) if p != local => {
                    bootstrap.entry(p).or_insert_with(Vec::new).push(addr.clone());
                }
                Some(_) => {}
                None => tracing::warn!(%addr, "bootstrap address without /p2p/<peer id>, ignored"),
            }
        }
        let mut net = Self {
            swarm,
            cfg,
            local,
            bootstrap,
            bootstrap_last_dial: HashMap::new(),
            circuits_requested: HashSet::new(),
            connected: HashMap::new(),
            known: HashMap::new(),
            registered: HashMap::new(),
            last_discover: None,
            responses: HashMap::new(),
            next_inbound: 0,
            subscribed: BTreeSet::new(),
            tick: Delay::new(Duration::from_millis(50)),
            queue: VecDeque::new(),
            started: Instant::now(),
        };
        net.dial_bootstrap();
        Ok(net)
    }

    pub fn local_peer_id(&self) -> PeerId {
        self.local
    }

    pub fn role(&self) -> Role {
        self.cfg.role
    }

    pub fn swarm(&mut self) -> &mut Swarm<Behaviour> {
        &mut self.swarm
    }

    pub fn is_bootstrap_peer(&self, p: &PeerId) -> bool {
        self.bootstrap.contains_key(p)
    }

    pub fn connected_peers(&self) -> impl Iterator<Item = (&PeerId, &PeerConn)> {
        self.connected.iter()
    }

    pub fn is_connected(&self, p: &PeerId) -> bool {
        self.connected.contains_key(p)
    }

    /// Connected peers known (from rendezvous) to be registered in `ns`.
    pub fn peers_in(&self, ns: &str) -> Vec<PeerId> {
        self.connected
            .keys()
            .filter(|p| self.known.get(p).is_some_and(|k| k.namespaces.contains(ns)))
            .copied()
            .collect()
    }

    pub fn listen_addrs(&self) -> Vec<Multiaddr> {
        self.swarm.listeners().cloned().collect()
    }

    pub fn external_addrs(&self) -> Vec<Multiaddr> {
        self.swarm.external_addresses().cloned().collect()
    }

    pub fn status(&self) -> NetStatus {
        NetStatus {
            peer_id: self.local.to_string(),
            role: self.cfg.role,
            listen: self.listen_addrs().iter().map(|a| a.to_string()).collect(),
            external: self.external_addrs().iter().map(|a| a.to_string()).collect(),
            peers: self.connected.iter().map(|(p, c)| (p.to_string(), c.addr.to_string(), c.relayed)).collect(),
            topics: self.subscribed.iter().cloned().collect(),
        }
    }

    pub fn subscribe(&mut self, topic: &str) {
        if self.subscribed.insert(topic.to_string())
            && let Err(e) = self.swarm.behaviour_mut().gossipsub.subscribe(&IdentTopic::new(topic))
        {
            tracing::warn!(topic, "subscribe failed: {e}");
        }
    }

    pub fn unsubscribe(&mut self, topic: &str) {
        if self.subscribed.remove(topic) {
            self.swarm.behaviour_mut().gossipsub.unsubscribe(&IdentTopic::new(topic));
        }
    }

    pub fn subscriptions(&self) -> &BTreeSet<String> {
        &self.subscribed
    }

    /// Publishes; returns false if nobody is subscribed yet or the payload is a
    /// duplicate (both are normal and not errors for the protocol).
    pub fn publish(&mut self, topic: &str, data: Vec<u8>) -> bool {
        match self.swarm.behaviour_mut().gossipsub.publish(IdentTopic::new(topic), data) {
            Ok(_) => true,
            Err(gossipsub::PublishError::Duplicate) => false,
            Err(gossipsub::PublishError::NoPeersSubscribedToTopic) => {
                tracing::warn!(topic, "publish: no peer subscribed to the topic");
                false
            }
            Err(e) => {
                tracing::debug!(topic, "publish failed: {e}");
                false
            }
        }
    }

    /// Reports the app-level validation result for a received gossip message.
    pub fn report(&mut self, id: &MessageId, from: &PeerId, accept: MessageAcceptance) {
        self.swarm.behaviour_mut().gossipsub.report_message_validation_result(id, from, accept);
    }

    pub fn request(&mut self, peer: &PeerId, data: Vec<u8>) -> OutboundRequestId {
        self.swarm.behaviour_mut().sync.send_request(peer, data)
    }

    pub fn respond(&mut self, id: u64, data: Vec<u8>) {
        if let Some(ch) = self.responses.remove(&id)
            && self.swarm.behaviour_mut().sync.send_response(ch, data).is_err()
        {
            tracing::debug!(id, "response channel closed");
        }
    }

    pub fn dial(&mut self, addr: Multiaddr) {
        if let Err(e) = self.swarm.dial(addr.clone()) {
            tracing::debug!(%addr, "dial failed: {e}");
        }
    }

    /// Replaces the namespaces we register under.
    pub fn set_register(&mut self, namespaces: Vec<String>) {
        let keep: HashSet<&String> = namespaces.iter().collect();
        let mut drop = Vec::new();
        for (rv, ns) in self.registered.keys() {
            if !keep.contains(ns) {
                drop.push((*rv, ns.clone()));
            }
        }
        for (rv, ns) in drop {
            self.registered.remove(&(rv, ns.clone()));
            if let Ok(n) = rendezvous::Namespace::new(ns) {
                self.swarm.behaviour_mut().rendezvous_client.unregister(n, rv);
            }
        }
        self.cfg.register = namespaces;
    }

    /// Replaces the namespaces we want connections into.
    pub fn set_want(&mut self, want: Vec<Want>) {
        if want != self.cfg.want {
            self.cfg.want = want;
            self.last_discover = None;
        }
    }

    pub async fn next_event(&mut self) -> NetEvent {
        loop {
            if let Some(ev) = self.queue.pop_front() {
                return ev;
            }
            futures::select! {
                ev = self.swarm.select_next_some() => self.on_swarm(ev),
                _ = futures::FutureExt::fuse(&mut self.tick) => {
                    self.tick.reset(TICK);
                    self.on_tick();
                }
            }
        }
    }

    fn dial_bootstrap(&mut self) {
        let now = Instant::now();
        let targets: Vec<(PeerId, Vec<Multiaddr>)> = self
            .bootstrap
            .iter()
            .filter(|(p, _)| !self.connected.contains_key(p))
            .filter(|(p, _)| self.bootstrap_last_dial.get(p).is_none_or(|t| now.duration_since(*t) >= REDIAL_BOOTSTRAP))
            .map(|(p, a)| (*p, dialable_with(self.cfg.role, self.cfg.prefer_webrtc, a)))
            .collect();
        for (p, addrs) in targets {
            self.bootstrap_last_dial.insert(p, now);
            if addrs.is_empty() {
                tracing::warn!(peer = %p, "no dialable bootstrap address for this role");
                continue;
            }
            if let Err(e) = self.swarm.dial(DialOpts::peer_id(p).addresses(addrs).build()) {
                tracing::debug!(peer = %p, "bootstrap dial failed: {e}");
            }
        }
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        self.dial_bootstrap();

        // Rendezvous registration (needs at least one external address).
        if self.swarm.external_addresses().next().is_some() {
            let rvs: Vec<PeerId> = self.bootstrap.keys().filter(|p| self.connected.contains_key(p)).copied().collect();
            for rv in rvs {
                for ns in self.cfg.register.clone() {
                    let key = (rv, ns.clone());
                    if self.registered.get(&key).is_some_and(|due| *due > now) {
                        continue;
                    }
                    let Ok(n) = rendezvous::Namespace::new(ns) else {
                        continue;
                    };
                    if let Err(e) =
                        self.swarm.behaviour_mut().rendezvous_client.register(n, rv, Some(self.cfg.register_ttl))
                    {
                        tracing::debug!("register failed: {e}");
                        continue;
                    }
                    // Retry soon unless the Registered event pushes this out.
                    self.registered.insert(key, now + Duration::from_secs(5));
                }
            }
        }

        // Discovery.
        let period = if now.duration_since(self.started) < FAST_PHASE { DISCOVER_FAST } else { DISCOVER_SLOW };
        if !self.cfg.want.is_empty() && self.last_discover.is_none_or(|t| now.duration_since(t) >= period) {
            self.last_discover = Some(now);
            let rvs: Vec<PeerId> = self.bootstrap.keys().filter(|p| self.connected.contains_key(p)).copied().collect();
            for rv in rvs {
                for w in self.cfg.want.clone() {
                    if let Ok(n) = rendezvous::Namespace::new(w.namespace) {
                        self.swarm.behaviour_mut().rendezvous_client.discover(Some(n), None, None, rv);
                    }
                }
            }
        }

        self.maintain_connections(now);
    }

    fn non_bootstrap_connected(&self) -> usize {
        self.connected.keys().filter(|p| !self.bootstrap.contains_key(p)).count()
    }

    fn maintain_connections(&mut self, now: Instant) {
        for w in self.cfg.want.clone() {
            let have = self.peers_in(&w.namespace).len();
            if have >= w.target {
                continue;
            }
            let mut need = w.target - have;
            let mut candidates: Vec<PeerId> = self
                .known
                .iter()
                .filter(|(p, k)| {
                    k.namespaces.contains(&w.namespace)
                        && !self.connected.contains_key(p)
                        && **p != self.local
                        && k.last_dial.is_none_or(|t| now.duration_since(t) >= REDIAL_PEER)
                })
                .map(|(p, _)| *p)
                .collect();
            // Spread load: order candidates by a hash of (us, them).
            candidates.sort_by_key(|p| {
                let mut b = self.local.to_bytes();
                b.extend(p.to_bytes());
                crate::keccak(&b)
            });
            for p in candidates {
                if need == 0 || self.non_bootstrap_connected() >= self.cfg.max_peers {
                    break;
                }
                let addrs = self
                    .known
                    .get(&p)
                    .map(|k| dialable_with(self.cfg.role, self.cfg.prefer_webrtc, &k.addrs))
                    .unwrap_or_default();
                if let Some(k) = self.known.get_mut(&p) {
                    k.last_dial = Some(now);
                }
                if addrs.is_empty() {
                    continue;
                }
                let opts = DialOpts::peer_id(p).addresses(addrs).build();
                if let Err(e) = self.swarm.dial(opts) {
                    tracing::debug!(peer = %p, "dial failed: {e}");
                }
                need -= 1;
            }
        }
    }

    fn on_swarm(&mut self, ev: SwarmEvent<BehaviourEvent>) {
        match ev {
            SwarmEvent::NewListenAddr { address, .. } => {
                if is_circuit(&address) || (self.cfg.announce_listen && !is_unspecified(&address)) {
                    self.swarm.add_external_address(address.clone());
                    // New reachability: refresh registrations.
                    self.registered.clear();
                }
                self.queue.push_back(NetEvent::Listening(address));
            }
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, num_established, .. } => {
                let addr = endpoint.get_remote_address().clone();
                // Inbound circuits show only `/p2p/<src>` as remote address;
                // their local (listen) address carries the `/p2p-circuit`.
                let relayed = is_circuit(&addr)
                    || matches!(&endpoint, libp2p::core::ConnectedPoint::Listener { local_addr, .. } if is_circuit(local_addr));
                let outbound = endpoint.is_dialer();
                let entry = self.connected.entry(peer_id).or_insert(PeerConn {
                    addr: addr.clone(),
                    relayed,
                    outbound,
                    conns: 0,
                });
                entry.conns = num_established.get() as usize;
                // Prefer reporting a direct connection over a relayed one.
                if entry.relayed && !relayed {
                    entry.addr = addr.clone();
                    entry.relayed = false;
                }
                if self.bootstrap.contains_key(&peer_id) {
                    self.on_bootstrap_connected(peer_id, addr.clone());
                }
                if relayed {
                    self.try_upgrade_direct(peer_id);
                }
                if num_established.get() == 1 {
                    self.queue.push_back(NetEvent::PeerConnected { peer: peer_id, addr, relayed, outbound });
                }
            }
            SwarmEvent::ConnectionClosed { peer_id, num_established, .. } => {
                if num_established == 0 {
                    self.connected.remove(&peer_id);
                    if self.bootstrap.contains_key(&peer_id) {
                        self.circuits_requested.remove(&peer_id);
                        self.registered.retain(|(rv, _), _| *rv != peer_id);
                    }
                    self.queue.push_back(NetEvent::PeerDisconnected { peer: peer_id });
                } else if let Some(c) = self.connected.get_mut(&peer_id) {
                    c.conns = num_established as usize;
                }
            }
            SwarmEvent::ExpiredListenAddr { address, .. } => {
                self.swarm.remove_external_address(&address);
            }
            SwarmEvent::ListenerClosed { addresses, .. } => {
                for a in addresses {
                    if is_circuit(&a)
                        && let Some(relay) = a.iter().find_map(|p| match p {
                            Protocol::P2p(id) => Some(id),
                            _ => None,
                        })
                    {
                        self.circuits_requested.remove(&relay);
                    }
                    self.swarm.remove_external_address(&a);
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                tracing::debug!(?peer_id, "outgoing connection error: {error}");
            }
            SwarmEvent::Behaviour(b) => self.on_behaviour(b),
            _ => {}
        }
    }

    fn on_bootstrap_connected(&mut self, relay: PeerId, rv_addr: Multiaddr) {
        // Browsers (and NATed nodes) listen through the relay.
        if self.cfg.role != Role::Bootstrap && !self.circuits_requested.contains(&relay) {
            let mut circuit = rv_addr;
            if peer_of(&circuit) != Some(relay) {
                circuit.push(Protocol::P2p(relay));
            }
            circuit.push(Protocol::P2pCircuit);
            match self.swarm.listen_on(circuit.clone()) {
                Ok(_) => {
                    self.circuits_requested.insert(relay);
                }
                Err(e) => tracing::warn!(%circuit, "relay listen failed: {e}"),
            }
        }
        // Register/discover right away instead of waiting for the tick.
        self.last_discover = None;
        self.tick.reset(Duration::from_millis(100));
    }

    /// Replaces a relayed connection with a direct one when the peer has a
    /// direct address we can dial (e.g. a native node seen through the relay).
    fn try_upgrade_direct(&mut self, peer: PeerId) {
        let Some(k) = self.known.get(&peer) else {
            return;
        };
        let direct: Vec<Multiaddr> = dialable_with(self.cfg.role, self.cfg.prefer_webrtc, &k.addrs)
            .into_iter()
            .filter(|a| !is_circuit(a))
            .collect();
        if direct.is_empty() {
            return;
        }
        let opts = DialOpts::peer_id(peer)
            .condition(libp2p::swarm::dial_opts::PeerCondition::Always)
            .addresses(direct)
            .build();
        let _ = self.swarm.dial(opts);
    }

    fn on_behaviour(&mut self, ev: BehaviourEvent) {
        match ev {
            BehaviourEvent::Gossipsub(gossipsub::Event::Message { propagation_source, message_id, message }) => {
                self.queue.push_back(NetEvent::Gossip {
                    topic: message.topic.into_string(),
                    data: message.data,
                    id: message_id,
                    from: propagation_source,
                    source: message.source,
                });
            }
            BehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic, .. }) => {
                self.queue.push_back(NetEvent::TopicPeer {
                    peer: peer_id,
                    topic: topic.into_string(),
                    subscribed: true,
                });
            }
            BehaviourEvent::Gossipsub(gossipsub::Event::Unsubscribed { peer_id, topic }) => {
                self.queue.push_back(NetEvent::TopicPeer {
                    peer: peer_id,
                    topic: topic.into_string(),
                    subscribed: false,
                });
            }
            BehaviourEvent::Sync(request_response::Event::Message { peer, message, .. }) => match message {
                request_response::Message::Request { request, channel, .. } => {
                    let id = self.next_inbound;
                    self.next_inbound += 1;
                    self.responses.insert(id, channel);
                    self.queue.push_back(NetEvent::Request { peer, id, data: request });
                }
                request_response::Message::Response { request_id, response } => {
                    self.queue.push_back(NetEvent::Response { peer, id: request_id, data: response });
                }
            },
            BehaviourEvent::Sync(request_response::Event::OutboundFailure { peer, request_id, error, .. }) => {
                self.queue.push_back(NetEvent::RequestFailed { peer, id: request_id, error: error.to_string() });
            }
            BehaviourEvent::Sync(request_response::Event::InboundFailure { peer, error, .. }) => {
                tracing::debug!(%peer, "inbound sync request failed: {error}");
            }
            BehaviourEvent::RendezvousClient(rendezvous::client::Event::Registered {
                namespace,
                ttl,
                rendezvous_node,
            }) => {
                let refresh = Instant::now() + Duration::from_secs((ttl / 2).max(5));
                self.registered.insert((rendezvous_node, namespace.to_string()), refresh);
                self.queue.push_back(NetEvent::Registered { namespace: namespace.to_string() });
            }
            BehaviourEvent::RendezvousClient(rendezvous::client::Event::RegisterFailed {
                namespace, error, ..
            }) => {
                tracing::debug!(%namespace, ?error, "rendezvous register failed");
            }
            BehaviourEvent::RendezvousClient(rendezvous::client::Event::Discovered { registrations, .. }) => {
                for r in registrations {
                    let peer = r.record.peer_id();
                    if peer == self.local {
                        continue;
                    }
                    let ns = r.namespace.to_string();
                    let addrs: Vec<Multiaddr> = r.record.addresses().to_vec();
                    let k = self.known.entry(peer).or_default();
                    k.namespaces.insert(ns.clone());
                    for a in &addrs {
                        if !k.addrs.contains(a) {
                            k.addrs.push(a.clone());
                        }
                    }
                    self.queue.push_back(NetEvent::Discovered { namespace: ns, peer, addrs });
                }
                self.maintain_connections(Instant::now());
            }
            BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. }) => {
                let k = self.known.entry(peer_id).or_default();
                for a in info.listen_addrs {
                    if !is_unspecified(&a) && !k.addrs.contains(&a) {
                        k.addrs.push(a);
                    }
                }
                if self.connected.get(&peer_id).is_some_and(|c| c.relayed) {
                    self.try_upgrade_direct(peer_id);
                }
            }
            BehaviourEvent::RelayClient(relay::client::Event::ReservationReqAccepted { relay_peer_id, .. }) => {
                self.queue.push_back(NetEvent::ReservationAccepted { relay: relay_peer_id });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dial_preference() {
        let wrtc: Multiaddr =
            "/ip4/127.0.0.1/udp/9000/webrtc-direct/certhash/uEiDDq4_xNyDorZBH3TlGazyJdOWSwvo4PUo5YHFMrvDE8g"
                .parse()
                .unwrap();
        let ws: Multiaddr = "/ip4/127.0.0.1/tcp/9001/ws".parse().unwrap();
        let quic: Multiaddr = "/ip4/127.0.0.1/udp/9002/quic-v1".parse().unwrap();
        let tcp: Multiaddr = "/ip4/127.0.0.1/tcp/9003".parse().unwrap();
        let circ: Multiaddr =
            "/ip4/127.0.0.1/tcp/9001/ws/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN/p2p-circuit"
                .parse()
                .unwrap();
        let all = vec![circ.clone(), ws.clone(), wrtc.clone(), quic.clone(), tcp.clone()];
        assert_eq!(dialable(Role::Browser, &all), vec![wrtc]);
        assert_eq!(dialable(Role::Browser, &[circ.clone(), ws.clone()]), vec![ws]);
        assert_eq!(dialable(Role::Browser, std::slice::from_ref(&circ)), vec![circ.clone()]);
        let quic_circ: Multiaddr =
            "/ip4/127.0.0.1/udp/9002/quic-v1/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN/p2p-circuit"
                .parse()
                .unwrap();
        assert_eq!(dialable(Role::Browser, &[quic_circ.clone(), circ.clone()]), vec![circ.clone()]);
        assert_eq!(dialable(Role::Node, &[circ.clone(), quic_circ.clone()]), vec![quic_circ]);
        assert_eq!(dialable(Role::Node, &all), vec![quic]);
        assert_eq!(dialable(Role::Node, &[tcp.clone(), circ.clone()]), vec![tcp]);
    }
}
