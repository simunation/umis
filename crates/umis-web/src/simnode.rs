//! `SimNode`: the browser node class.

use js_sys::{Function, Promise};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};
use umis_core::genesis::Genesis;
use umis_core::{Hash, SecretKey};
use umis_net::libp2p::{Multiaddr, identity};
use umis_net::{Net, NetConfig, Role, Topics, Want};
use umis_node::{Handle, Runtime};
use umis_protocol::{Command, Ctx, Node, NodeConfig, VotePolicy};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

static INIT: Once = Once::new();

pub(crate) fn init() {
    INIT.call_once(|| {
        console_error_panic_hook::set_once();
        tracing_wasm::set_as_global_default_with_config(
            tracing_wasm::WASMLayerConfigBuilder::new().set_max_level(tracing::Level::WARN).build(),
        );
    });
}

pub(crate) fn registry() -> umis_task::TaskRegistry {
    let mut r = umis_task::TaskRegistry::new();
    task_water3d::register(&mut r);
    r
}

pub(crate) fn js_err(e: impl std::fmt::Display) -> JsError {
    JsError::new(&e.to_string())
}

fn emit(cb: &Rc<RefCell<Option<Function>>>, v: &serde_json::Value) {
    if let Some(f) = cb.borrow().as_ref()
        && let Ok(js) = serde_wasm_bindgen::to_value(v)
    {
        let _ = f.call1(&JsValue::NULL, &js);
    }
}

type Slot = Rc<RefCell<Option<Handle>>>;

#[wasm_bindgen]
pub struct SimNode {
    key: SecretKey,
    genesis: Genesis,
    handle: Slot,
    started: bool,
    prefer_webrtc: bool,
    prover: Option<Function>,
    callback: Rc<RefCell<Option<Function>>>,
}

#[wasm_bindgen]
impl SimNode {
    /// `genesis_json`: the devnet genesis; `secret_hex`: a persisted key, or
    /// `undefined` for a fresh one.
    #[wasm_bindgen(constructor)]
    pub fn new(genesis_json: &str, secret_hex: Option<String>) -> Result<SimNode, JsError> {
        init();
        let genesis = Genesis::from_json(genesis_json).map_err(js_err)?;
        let key = match secret_hex {
            Some(h) => SecretKey::from_bytes(&Hash::from_hex(&h).map_err(js_err)?.0),
            None => {
                let kp = identity::Keypair::generate_ed25519();
                let ed = kp.try_into_ed25519().map_err(js_err)?;
                let mut s = [0u8; 32];
                s.copy_from_slice(&ed.secret().as_ref()[..32]);
                SecretKey::from_bytes(&s)
            }
        };
        Ok(SimNode {
            key,
            genesis,
            handle: Rc::new(RefCell::new(None)),
            started: false,
            prefer_webrtc: false,
            prover: None,
            callback: Rc::new(RefCell::new(None)),
        })
    }

    #[wasm_bindgen(getter, js_name = nodeId)]
    pub fn node_id(&self) -> String {
        self.key.node_id().to_hex()
    }

    #[wasm_bindgen(getter, js_name = peerId)]
    pub fn peer_id(&self) -> String {
        umis_node::peer_of(&self.key.node_id()).map(|p| p.to_string()).unwrap_or_default()
    }

    /// The secret key (hex), for the page to persist.
    #[wasm_bindgen(getter, js_name = secretHex)]
    pub fn secret_hex(&self) -> String {
        hex::encode(self.key.to_bytes())
    }

    #[wasm_bindgen(getter)]
    pub fn genesis8(&self) -> String {
        self.genesis.genesis8()
    }

    #[wasm_bindgen(getter)]
    pub fn ready(&self) -> bool {
        self.handle.borrow().is_some()
    }

    /// Receives UI events (`{kind: ...}` objects).
    #[wasm_bindgen(js_name = onEvent)]
    pub fn on_event(&self, cb: Function) {
        *self.callback.borrow_mut() = Some(cb);
    }

    /// Dial native nodes over WebRTC-direct instead of WebSocket (set before `start`).
    #[wasm_bindgen(js_name = preferWebrtc)]
    pub fn prefer_webrtc(&mut self, on: bool) {
        self.prefer_webrtc = on;
    }

    /// Proves STARK jobs off the page thread: `prover(jobBytes: Uint8Array)`
    /// returns a `Promise<Uint8Array>` of the proof, normally by posting the
    /// job to a Web Worker that calls `proveJob` (set before `start`).
    /// Without one, jobs are proven inline on the page thread.
    #[wasm_bindgen(js_name = setProver)]
    pub fn set_prover(&mut self, prover: Function) {
        self.prover = Some(prover);
    }

    /// Starts networking and the protocol; `bootstrap` holds multiaddrs.
    pub fn start(&mut self, bootstrap: Vec<String>) -> Result<(), JsError> {
        if self.started {
            return Err(JsError::new("already started"));
        }
        self.started = true;
        let addrs: Vec<Multiaddr> = bootstrap.iter().map(|s| s.parse()).collect::<Result<_, _>>().map_err(js_err)?;
        let backend = task_water3d::air::backend_for(&self.genesis).map_err(js_err)?;
        let ctx = Arc::new(Ctx::new(self.genesis.clone(), registry(), backend));
        let mut cfg = NodeConfig::new(self.key.clone());
        cfg.keep_blocks = false;
        if self.prover.is_some() {
            cfg.max_jobs = 1;
        }
        let node = Node::new(ctx, cfg).map_err(js_err)?;
        let prover = self.prover.clone();
        let mut b = self.key.to_bytes();
        let kp = identity::Keypair::ed25519_from_bytes(&mut b).map_err(js_err)?;
        let mut nc = NetConfig::new(Role::Browser, kp);
        nc.bootstrap = addrs;
        nc.prefer_webrtc = self.prefer_webrtc;
        let topics = Topics::new(self.genesis.genesis8());
        nc.register = vec![topics.ns_all()];
        nc.want = vec![Want { namespace: topics.ns_all(), target: 4 }];
        let cb = self.callback.clone();
        let slot = self.handle.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let net = match Net::new(nc).await {
                Ok(n) => n,
                Err(e) => {
                    emit(&cb, &serde_json::json!({ "kind": "Error", "message": e.to_string() }));
                    return;
                }
            };
            let (mut rt, handle) = Runtime::new(node, net);
            if let Some(prover) = prover {
                rt.set_executor(Box::new(crate::prover::JsExecutor { prover }));
            }
            let mut ui = rt.ui_events();
            *slot.borrow_mut() = Some(handle);
            emit(&cb, &serde_json::json!({ "kind": "Started" }));
            let cb2 = cb.clone();
            wasm_bindgen_futures::spawn_local(async move {
                use futures::StreamExt;
                while let Some(ev) = ui.next().await {
                    if let Ok(v) = serde_json::to_value(&ev) {
                        emit(&cb2, &v);
                    }
                }
            });
            rt.run().await;
            emit(&cb, &serde_json::json!({ "kind": "Stopped" }));
        });
        Ok(())
    }

    pub fn stop(&self) {
        if let Some(h) = self.handle.borrow_mut().take() {
            h.stop();
        }
    }

    fn h(&self) -> Result<Handle, JsError> {
        self.handle.borrow().clone().ok_or_else(|| JsError::new("node not started yet"))
    }

    /// Promise of the node snapshot (JSON).
    pub fn status(&self) -> Result<Promise, JsError> {
        let h = self.h()?;
        Ok(future_to_promise(async move {
            let s = h.snapshot().await.ok_or_else(|| JsValue::from_str("stopped"))?;
            serde_wasm_bindgen::to_value(&s).map_err(|e| JsValue::from_str(&e.to_string()))
        }))
    }

    /// Promise of the network status (JSON).
    #[wasm_bindgen(js_name = netStatus)]
    pub fn net_status(&self) -> Result<Promise, JsError> {
        let h = self.h()?;
        Ok(future_to_promise(async move {
            let s = h.net_status().await.ok_or_else(|| JsValue::from_str("stopped"))?;
            serde_wasm_bindgen::to_value(&s).map_err(|e| JsValue::from_str(&e.to_string()))
        }))
    }

    /// Promise of `{dims, origin, channels, data: Float32Array}` for a zone
    /// this node follows (display only; fixed point converted to floats).
    #[wasm_bindgen(js_name = zoneState)]
    pub fn zone_state(&self, zone: u32) -> Result<Promise, JsError> {
        let h = self.h()?;
        Ok(future_to_promise(async move {
            let Some(r) = h.render(zone).await else { return Ok(JsValue::NULL) };
            let o = js_sys::Object::new();
            let set = |k: &str, v: &JsValue| js_sys::Reflect::set(&o, &JsValue::from_str(k), v);
            set("dims", &serde_wasm_bindgen::to_value(&r.dims).unwrap_or(JsValue::NULL))?;
            set("origin", &serde_wasm_bindgen::to_value(&r.origin).unwrap_or(JsValue::NULL))?;
            set("channels", &JsValue::from(r.channels))?;
            set("data", &js_sys::Float32Array::from(r.data.as_slice()).into())?;
            Ok(o.into())
        }))
    }

    /// Promise of the canonical beacon blocks in `[from, to]` with their
    /// commitments, slashes, rollbacks and free zone locations.
    pub fn lattice(&self, from: u64, to: u64) -> Result<Promise, JsError> {
        let h = self.h()?;
        Ok(future_to_promise(async move {
            let l = h.lattice(from, to).await.ok_or_else(|| JsValue::from_str("stopped"))?;
            serde_wasm_bindgen::to_value(&l).map_err(|e| JsValue::from_str(&e.to_string()))
        }))
    }

    /// Lock `stake` and join as a zone member (the beacon chain assigns the zone).
    pub fn join(&self, stake: u64) -> Result<(), JsError> {
        self.h()?.command(Command::Join { stake, full_node: false });
        Ok(())
    }

    pub fn leave(&self) -> Result<(), JsError> {
        self.h()?.command(Command::Leave);
        Ok(())
    }

    /// `auto` | `manual` | `reject_all`.
    #[wasm_bindgen(js_name = setVotePolicy)]
    pub fn set_vote_policy(&self, policy: &str) -> Result<(), JsError> {
        let p = match policy {
            "auto" => VotePolicy::Auto,
            "manual" => VotePolicy::Manual,
            "reject_all" => VotePolicy::RejectAll,
            _ => return Err(JsError::new("policy: auto | manual | reject_all")),
        };
        self.h()?.set_vote_policy(p);
        Ok(())
    }

    pub fn vote(&self, proposal_id: &str, accept: bool) -> Result<(), JsError> {
        let id = Hash::from_hex(proposal_id).map_err(js_err)?;
        self.h()?.command(Command::Vote { proposal_id: id, accept });
        Ok(())
    }

    /// Proposes a water zone at `point` with slot length `slot_ticks`;
    /// `founder_secrets` are the hex keys of founder nodes in this page
    /// (each co-signs). Returns the proposal id.
    #[wasm_bindgen(js_name = proposeZone)]
    pub fn propose_zone(
        &self,
        point: Vec<i32>,
        slot_ticks: u32,
        founder_secrets: Vec<String>,
    ) -> Result<String, JsError> {
        if point.len() != 3 {
            return Err(JsError::new("point must have 3 coordinates"));
        }
        let founders: Vec<SecretKey> = founder_secrets
            .iter()
            .map(|h| Hash::from_hex(h).map(|b| SecretKey::from_bytes(&b.0)))
            .collect::<Result<_, _>>()
            .map_err(js_err)?;
        let n_loc = self.genesis.zones.first().and_then(|z| z.params["n_loc"].as_u64()).unwrap_or(16) as u32;
        let p = task_water3d::devnet::zone_params([point[0], point[1], point[2]], slot_ticks as u64, n_loc);
        let params = borsh::to_vec(&p).map_err(js_err)?;
        let refs: Vec<&SecretKey> = founders.iter().collect();
        let tx = umis_protocol::types::build_propose_tx(
            self.key.node_id(),
            &refs,
            [point[0], point[1], point[2]],
            task_water3d::LAYER,
            task_water3d::TASK_ID,
            params,
            slot_ticks as u64,
        );
        let id = tx.proposal_id().to_hex();
        self.h()?.command(Command::Propose(Box::new(tx)));
        Ok(id)
    }
}

/// First 8 hex characters of a genesis file's hash.
#[wasm_bindgen(js_name = genesisId)]
pub fn genesis_id(genesis_json: &str) -> Result<String, JsError> {
    Ok(Genesis::from_json(genesis_json).map_err(js_err)?.genesis8())
}
