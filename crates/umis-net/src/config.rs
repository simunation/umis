use libp2p::{Multiaddr, identity};

/// What a node is in the network; selects transports and behaviours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Role {
    /// Native relay + rendezvous server, also a gossip participant.
    Bootstrap,
    /// Native full node.
    Node,
    /// Browser (wasm) node.
    Browser,
}

/// A rendezvous namespace the node wants peers from, and how many connections
/// to peers registered in it it should keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub namespace: String,
    pub target: usize,
}

#[derive(Clone)]
pub struct NetConfig {
    pub role: Role,
    pub keypair: identity::Keypair,
    /// Addresses to listen on (native only; browsers listen on relay circuits).
    pub listen: Vec<Multiaddr>,
    /// Relay + rendezvous servers, each with a trailing `/p2p/<peer id>`.
    pub bootstrap: Vec<Multiaddr>,
    /// Extra external addresses to announce (e.g. a public IP).
    pub announce: Vec<Multiaddr>,
    /// Announce our own concrete listen addresses as external addresses.
    pub announce_listen: bool,
    /// Namespaces we register ourselves under.
    pub register: Vec<String>,
    /// Namespaces we discover and dial peers from.
    pub want: Vec<Want>,
    /// Hard cap on connected peers (bootstrap peers excluded).
    pub max_peers: usize,
    /// Persisted WebRTC-direct certificate (PEM), so the certhash stays stable.
    pub webrtc_cert_pem: Option<String>,
    /// Rendezvous registration TTL in seconds.
    pub register_ttl: u64,
    /// Browsers: dial native nodes over WebRTC-direct before WebSocket.
    /// Off by default: with libp2p-webrtc 0.10-alpha / webrtc-websys 0.5,
    /// long-lived gossipsub streams over WebRTC break under load (see
    /// docs/ARCHITECTURE.md, "Known issues").
    pub prefer_webrtc: bool,
}

impl NetConfig {
    pub fn new(role: Role, keypair: identity::Keypair) -> Self {
        Self {
            role,
            keypair,
            listen: Vec::new(),
            bootstrap: Vec::new(),
            announce: Vec::new(),
            announce_listen: true,
            register: Vec::new(),
            want: Vec::new(),
            max_peers: match role {
                Role::Browser => 8,
                Role::Node => 64,
                Role::Bootstrap => 1024,
            },
            webrtc_cert_pem: None,
            register_ttl: 120,
            prefer_webrtc: false,
        }
    }
}
