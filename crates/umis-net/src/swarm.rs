//! Swarm construction per target.
//!
//! - native: QUIC, WebRTC-direct (for browsers), TCP and WebSocket (fallback),
//!   relay client; plus relay/rendezvous servers on the bootstrap role.
//! - wasm: WebRTC-direct (`webrtc-websys`) and WebSocket (`websocket-websys`)
//!   to native nodes, relay client for browser↔browser circuits.

use crate::behaviour::Behaviour;
use crate::config::NetConfig;
use libp2p::Swarm;
use std::time::Duration;

const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(not(target_arch = "wasm32"))]
pub async fn build_swarm(cfg: &NetConfig) -> anyhow::Result<Swarm<Behaviour>> {
    use libp2p::core::muxing::StreamMuxerBox;
    use libp2p::core::transport::Transport as _;
    use libp2p::{noise, tcp, yamux};

    // webrtc-rs and QUIC both pull rustls with several crypto backends enabled,
    // so the process-wide provider has to be chosen explicitly.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cert = match &cfg.webrtc_cert_pem {
        Some(pem) => libp2p_webrtc::tokio::Certificate::from_pem(pem)?,
        None => libp2p_webrtc::tokio::Certificate::generate(&mut rand::rng())?,
    };
    let role = cfg.role;
    let swarm = libp2p::SwarmBuilder::with_existing_identity(cfg.keypair.clone())
        .with_tokio()
        .with_tcp(tcp::Config::default().nodelay(true), noise::Config::new, yamux::Config::default)?
        .with_quic()
        .with_other_transport(|key| {
            libp2p_webrtc::tokio::Transport::new(key.clone(), cert)
                .map(|(peer, conn), _| (peer, StreamMuxerBox::new(conn)))
        })?
        .with_dns()?
        .with_websocket(noise::Config::new, yamux::Config::default)
        .await?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_client| Behaviour::new(role, key, relay_client))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(IDLE_TIMEOUT))
        .build();
    Ok(swarm)
}

#[cfg(target_arch = "wasm32")]
pub async fn build_swarm(cfg: &NetConfig) -> anyhow::Result<Swarm<Behaviour>> {
    use libp2p::core::muxing::StreamMuxerBox;
    use libp2p::core::transport::{Transport as _, upgrade::Version};
    use libp2p::{noise, webrtc_websys, websocket_websys, yamux};

    let role = cfg.role;
    let swarm = libp2p::SwarmBuilder::with_existing_identity(cfg.keypair.clone())
        .with_wasm_bindgen()
        .with_other_transport(|key| {
            webrtc_websys::Transport::new(webrtc_websys::Config::new(key))
                .map(|(peer, conn), _| (peer, StreamMuxerBox::new(conn)))
        })?
        .with_other_transport(|key| {
            websocket_websys::Transport::default()
                .upgrade(Version::V1Lazy)
                .authenticate(noise::Config::new(key).expect("noise config from an ed25519 key"))
                .multiplex(yamux::Config::default())
                .map(|(peer, conn), _| (peer, StreamMuxerBox::new(conn)))
        })?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_client| Behaviour::new(role, key, relay_client))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(IDLE_TIMEOUT))
        .build();
    Ok(swarm)
}

/// Generates a fresh WebRTC-direct certificate as PEM (native only).
#[cfg(not(target_arch = "wasm32"))]
pub fn generate_webrtc_cert_pem() -> anyhow::Result<String> {
    Ok(libp2p_webrtc::tokio::Certificate::generate(&mut rand::rng())?.serialize_pem())
}
