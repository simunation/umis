//! libp2p adapter for simunation.
//!
//! The protocol core is sans-IO; this crate only moves bytes: gossip topics,
//! the `/simu/sync/1` request-response protocol, and the peering policy
//! (relay reservations, rendezvous registration/discovery, per-namespace
//! connection targets). Transports are chosen per role and target, see
//! [`swarm`].

pub mod behaviour;
pub mod codec;
pub mod config;
pub mod net;
pub mod swarm;
pub mod topics;

pub use config::{NetConfig, Role, Want};
pub use libp2p;
pub use net::{Net, NetEvent, NetStatus, PeerConn, dialable};
pub use topics::Topics;

pub(crate) fn keccak(data: &[u8]) -> [u8; 32] {
    use tiny_keccak::{Hasher, Keccak};
    let mut k = Keccak::v256();
    k.update(data);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}
