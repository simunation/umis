//! M1 connectivity spike: a browser node that dials the bootstrap, reserves a
//! relay circuit, registers in rendezvous, discovers and dials the other
//! browser nodes through the relay, and gossips on a topic the bootstrap does
//! not subscribe to (so every received message proves a browser↔browser path).

use futures::StreamExt;
use futures::channel::mpsc;
use serde::Serialize;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use umis_net::libp2p::{Multiaddr, gossipsub::MessageAcceptance, identity};
use umis_net::{Net, NetConfig, NetEvent, Role, Want};
use wasm_bindgen::prelude::*;

const NS: &str = "simu/spike/all";
pub const SPIKE_TOPIC: &str = "/simu/spike/chat";

enum Cmd {
    Publish(String),
}

#[derive(Serialize)]
#[serde(tag = "kind")]
enum UiEvent<'a> {
    Listening { addr: String },
    Connected { peer: String, addr: String, relayed: bool },
    Disconnected { peer: String },
    Message { from: String, text: String },
    Registered { namespace: &'a str },
    Reservation { relay: String },
    Discovered { peer: String },
    Error { message: String },
}

#[wasm_bindgen]
pub struct SpikeNode {
    peer_id: String,
    keypair: identity::Keypair,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    cmd_rx: Rc<RefCell<Option<mpsc::UnboundedReceiver<Cmd>>>>,
    callback: Rc<RefCell<Option<js_sys::Function>>>,
    status: Rc<RefCell<String>>,
}

fn emit(cb: &Rc<RefCell<Option<js_sys::Function>>>, ev: &UiEvent<'_>) {
    if let Some(f) = cb.borrow().as_ref()
        && let Ok(v) = serde_wasm_bindgen::to_value(ev)
    {
        let _ = f.call1(&JsValue::NULL, &v);
    }
}

#[wasm_bindgen]
impl SpikeNode {
    #[wasm_bindgen(constructor)]
    pub fn new() -> SpikeNode {
        console_error_panic_hook::set_once();
        let keypair = identity::Keypair::generate_ed25519();
        let (cmd_tx, cmd_rx) = mpsc::unbounded();
        SpikeNode {
            peer_id: keypair.public().to_peer_id().to_string(),
            keypair,
            cmd_tx,
            cmd_rx: Rc::new(RefCell::new(Some(cmd_rx))),
            callback: Rc::new(RefCell::new(None)),
            status: Rc::new(RefCell::new("{}".into())),
        }
    }

    #[wasm_bindgen(getter, js_name = peerId)]
    pub fn peer_id(&self) -> String {
        self.peer_id.clone()
    }

    #[wasm_bindgen(js_name = onEvent)]
    pub fn on_event(&self, cb: js_sys::Function) {
        *self.callback.borrow_mut() = Some(cb);
    }

    /// JSON snapshot of the network status.
    pub fn status(&self) -> String {
        self.status.borrow().clone()
    }

    pub fn publish(&self, text: String) {
        let _ = self.cmd_tx.unbounded_send(Cmd::Publish(text));
    }

    /// Starts the node; `bootstrap` holds the bootstrap multiaddrs.
    pub fn start(&self, bootstrap: Vec<String>) -> Result<(), JsError> {
        let mut rx = self.cmd_rx.borrow_mut().take().ok_or_else(|| JsError::new("already started"))?;
        let mut cfg = NetConfig::new(Role::Browser, self.keypair.clone());
        cfg.bootstrap = bootstrap
            .iter()
            .map(|s| s.parse::<Multiaddr>())
            .collect::<Result<_, _>>()
            .map_err(|e| JsError::new(&format!("bad multiaddr: {e}")))?;
        cfg.register = vec![NS.into()];
        cfg.want = vec![Want { namespace: NS.into(), target: 8 }];
        let cb = self.callback.clone();
        let status = self.status.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut net = match Net::new(cfg).await {
                Ok(n) => n,
                Err(e) => {
                    emit(&cb, &UiEvent::Error { message: e.to_string() });
                    return;
                }
            };
            net.subscribe(SPIKE_TOPIC);
            let mut status_tick = futures_timer::Delay::new(Duration::from_millis(500));
            loop {
                futures::select! {
                    ev = futures::FutureExt::fuse(net.next_event()) => match ev {
                        NetEvent::Listening(a) => emit(&cb, &UiEvent::Listening { addr: a.to_string() }),
                        NetEvent::PeerConnected { peer, addr, relayed, .. } => emit(&cb, &UiEvent::Connected {
                            peer: peer.to_string(), addr: addr.to_string(), relayed,
                        }),
                        NetEvent::PeerDisconnected { peer } => emit(&cb, &UiEvent::Disconnected { peer: peer.to_string() }),
                        NetEvent::Gossip { data, id, from, source, .. } => {
                            net.report(&id, &from, MessageAcceptance::Accept);
                            emit(&cb, &UiEvent::Message {
                                from: source.map(|s| s.to_string()).unwrap_or_default(),
                                text: String::from_utf8_lossy(&data).into_owned(),
                            });
                        }
                        NetEvent::Registered { .. } => emit(&cb, &UiEvent::Registered { namespace: NS }),
                        NetEvent::ReservationAccepted { relay } => emit(&cb, &UiEvent::Reservation { relay: relay.to_string() }),
                        NetEvent::Discovered { peer, .. } => emit(&cb, &UiEvent::Discovered { peer: peer.to_string() }),
                        _ => {}
                    },
                    cmd = rx.next() => match cmd {
                        Some(Cmd::Publish(t)) => { net.publish(SPIKE_TOPIC, t.into_bytes()); }
                        None => break,
                    },
                    _ = futures::FutureExt::fuse(&mut status_tick) => {
                        status_tick.reset(Duration::from_millis(500));
                        *status.borrow_mut() = serde_json::to_string(&net.status()).unwrap_or_default();
                    }
                }
            }
        });
        Ok(())
    }
}

impl Default for SpikeNode {
    fn default() -> Self {
        Self::new()
    }
}
