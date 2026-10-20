//! A bootstrap (relay + rendezvous) and two native nodes on localhost: the
//! nodes discover each other through rendezvous, connect directly over QUIC,
//! exchange gossip on a topic the bootstrap does not subscribe to, and answer
//! a sync request.

use std::time::Duration;
use umis_net::libp2p::{Multiaddr, PeerId, gossipsub::MessageAcceptance, identity, multiaddr::Protocol};
use umis_net::{Net, NetConfig, NetEvent, Role, Want};

async fn bootstrap() -> (Net, Multiaddr) {
    let mut cfg = NetConfig::new(Role::Bootstrap, identity::Keypair::generate_ed25519());
    cfg.listen = vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()];
    let mut net = Net::new(cfg).await.unwrap();
    let addr = loop {
        if let NetEvent::Listening(a) = net.next_event().await {
            break a;
        }
    };
    let id = net.local_peer_id();
    (net, addr.with(Protocol::P2p(id)))
}

async fn node(boot: &Multiaddr) -> Net {
    let mut cfg = NetConfig::new(Role::Node, identity::Keypair::generate_ed25519());
    cfg.listen = vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()];
    cfg.bootstrap = vec![boot.clone()];
    cfg.register = vec!["simu/test/all".into()];
    cfg.want = vec![Want { namespace: "simu/test/all".into(), target: 4 }];
    let mut n = Net::new(cfg).await.unwrap();
    n.subscribe("/simu/test/spike");
    n
}

#[tokio::test(flavor = "multi_thread")]
async fn two_nodes_find_each_other_and_gossip() {
    let (mut boot, boot_addr) = bootstrap().await;
    let mut a = node(&boot_addr).await;
    let mut b = node(&boot_addr).await;
    let b_id = b.local_peer_id();
    let a_id = a.local_peer_id();

    tokio::spawn(async move {
        loop {
            boot.next_event().await;
        }
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut a_got = false;
    let mut b_got = false;
    let mut b_answered = false;
    let mut a_asked = false;
    let mut published = 0u32;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    while !(a_got && b_got && b_answered) {
        assert!(tokio::time::Instant::now() < deadline, "timed out: a_got={a_got} b_got={b_got} answered={b_answered}");
        tokio::select! {
            ev = a.next_event() => match ev {
                NetEvent::Gossip { data, id, from, source, .. } => {
                    a.report(&id, &from, MessageAcceptance::Accept);
                    if source == Some(b_id) && data.starts_with(b"from-b") { a_got = true; }
                }
                NetEvent::PeerConnected { peer, relayed, .. } if peer == b_id && !a_asked => {
                    assert!(!relayed, "native nodes must connect directly");
                    a.request(&b_id, b"ping".to_vec());
                    a_asked = true;
                }
                NetEvent::Response { peer, data, .. } if peer == b_id => {
                    assert_eq!(data, b"pong");
                    b_answered = true;
                }
                _ => {}
            },
            ev = b.next_event() => match ev {
                NetEvent::Gossip { data, id, from, source, .. } => {
                    b.report(&id, &from, MessageAcceptance::Accept);
                    if source == Some(a_id) && data.starts_with(b"from-a") { b_got = true; }
                }
                NetEvent::Request { id, data, .. } => {
                    assert_eq!(data, b"ping");
                    b.respond(id, b"pong".to_vec());
                }
                _ => {}
            },
            _ = tick.tick() => {
                published += 1;
                a.publish("/simu/test/spike", format!("from-a {published}").into_bytes());
                b.publish("/simu/test/spike", format!("from-b {published}").into_bytes());
            }
        }
    }
    let peers: Vec<PeerId> = a.connected_peers().map(|(p, _)| *p).collect();
    assert!(peers.contains(&b_id));
}
