//! Request-response over WebRTC-direct (the transport browsers use).

use std::time::Duration;
use umis_net::libp2p::{identity, multiaddr::Protocol};
use umis_net::{Net, NetConfig, NetEvent, Role};

#[tokio::test(flavor = "multi_thread")]
async fn request_response_over_webrtc_direct() {
    let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).try_init();
    let mut cfg = NetConfig::new(Role::Bootstrap, identity::Keypair::generate_ed25519());
    cfg.listen = vec!["/ip4/127.0.0.1/udp/0/webrtc-direct".parse().unwrap()];
    let mut srv = Net::new(cfg).await.unwrap();
    let addr = loop {
        if let NetEvent::Listening(a) = srv.next_event().await {
            break a;
        }
    };
    let srv_id = srv.local_peer_id();
    let mut ccfg = NetConfig::new(Role::Node, identity::Keypair::generate_ed25519());
    ccfg.bootstrap = vec![addr.with(Protocol::P2p(srv_id))];
    ccfg.listen = vec!["/ip4/127.0.0.1/udp/0/webrtc-direct".parse().unwrap()];
    let mut cli = Net::new(ccfg).await.unwrap();
    let big = vec![7u8; 100_000];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut asked = false;
    loop {
        assert!(tokio::time::Instant::now() < deadline, "timed out");
        tokio::select! {
            ev = srv.next_event() => match ev {
                NetEvent::Request { id, data, .. } => {
                    assert_eq!(data, b"hi");
                    srv.respond(id, big.clone());
                }
                other => eprintln!("server: {other:?}"),
            },
            ev = cli.next_event() => match ev {
                NetEvent::PeerConnected { peer, addr, .. } if peer == srv_id && !asked => {
                    assert!(addr.to_string().contains("webrtc-direct"), "{addr}");
                    cli.request(&srv_id, b"hi".to_vec());
                    asked = true;
                }
                NetEvent::Response { data, .. } => { assert_eq!(data.len(), big.len()); break; }
                NetEvent::RequestFailed { error, .. } => panic!("request failed: {error}"),
                other => eprintln!("client: {other:?}"),
            },
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}
