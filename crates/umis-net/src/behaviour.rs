//! The composed libp2p behaviour. Every role uses the same struct; parts a role
//! does not need are switched off with `Toggle`.

use crate::codec::{BytesCodec, SYNC_PROTOCOL};
use crate::config::Role;
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::{PeerId, dcutr, gossipsub, identify, identity, ping, relay, rendezvous, request_response};
use std::time::Duration;

pub const IDENTIFY_PROTOCOL: &str = "/simu/id/1.0.0";

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub relay_client: relay::client::Behaviour,
    pub relay_server: Toggle<relay::Behaviour>,
    pub rendezvous_client: rendezvous::client::Behaviour,
    pub rendezvous_server: Toggle<rendezvous::server::Behaviour>,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub dcutr: Toggle<dcutr::Behaviour>,
    pub gossipsub: gossipsub::Behaviour,
    pub sync: request_response::Behaviour<BytesCodec>,
}

/// gossipsub message id: keccak256 of the data, so the same payload published
/// by two peers is deduplicated.
fn message_id(m: &gossipsub::Message) -> gossipsub::MessageId {
    use tiny_keccak::{Hasher, Keccak};
    let mut k = Keccak::v256();
    k.update(&m.data);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    gossipsub::MessageId::from(out.to_vec())
}

pub fn gossipsub_config() -> gossipsub::Config {
    gossipsub::ConfigBuilder::default()
        // Beacon blocks carry STARK proofs and their public data.
        .max_transmit_size(32 << 20)
        .message_id_fn(message_id)
        .validate_messages()
        .validation_mode(gossipsub::ValidationMode::Strict)
        .heartbeat_interval(Duration::from_millis(700))
        .mesh_n(4)
        .mesh_n_low(2)
        .mesh_n_high(8)
        .mesh_outbound_min(1)
        .gossip_lazy(4)
        .history_length(6)
        .history_gossip(3)
        .duplicate_cache_time(Duration::from_secs(120))
        .flood_publish(true)
        .build()
        .expect("valid gossipsub config")
}

/// Relay limits are raised far above the libp2p defaults (2 min, 128 KiB per
/// circuit): browser↔browser traffic runs through the relay for the whole session.
pub fn relay_server_config() -> relay::Config {
    relay::Config {
        max_reservations: 4096,
        max_reservations_per_peer: 16,
        reservation_duration: Duration::from_secs(3600),
        reservation_rate_limiters: Vec::new(),
        max_circuits: 4096,
        max_circuits_per_peer: 256,
        max_circuit_duration: Duration::from_secs(24 * 3600),
        max_circuit_bytes: 1 << 40,
        circuit_src_rate_limiters: Vec::new(),
    }
}

impl Behaviour {
    pub fn new(role: Role, key: &identity::Keypair, relay_client: relay::client::Behaviour) -> Self {
        let peer_id = PeerId::from(key.public());
        let is_bootstrap = role == Role::Bootstrap;
        let gossipsub =
            gossipsub::Behaviour::new(gossipsub::MessageAuthenticity::Signed(key.clone()), gossipsub_config())
                .expect("gossipsub behaviour");
        let sync = request_response::Behaviour::new(
            [(SYNC_PROTOCOL, request_response::ProtocolSupport::Full)],
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(30))
                .with_max_concurrent_streams(256),
        );
        Self {
            relay_client,
            relay_server: Toggle::from(is_bootstrap.then(|| relay::Behaviour::new(peer_id, relay_server_config()))),
            rendezvous_client: rendezvous::client::Behaviour::new(key.clone()),
            rendezvous_server: Toggle::from(is_bootstrap.then(|| {
                rendezvous::server::Behaviour::new(
                    rendezvous::server::Config::default().with_min_ttl(30).with_max_registration_per_peer(64),
                )
            })),
            identify: identify::Behaviour::new(
                identify::Config::new(IDENTIFY_PROTOCOL.to_string(), key.public())
                    .with_push_listen_addr_updates(true)
                    .with_interval(Duration::from_secs(60)),
            ),
            ping: ping::Behaviour::new(ping::Config::new().with_interval(Duration::from_secs(10))),
            dcutr: Toggle::from((role == Role::Node).then(|| dcutr::Behaviour::new(peer_id))),
            gossipsub,
            sync,
        }
    }
}
