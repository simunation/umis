//! `umis-cli bootstrap`: relay + rendezvous server, an observer protocol node
//! holding the devnet faucet key, and HTTP endpoints:
//!
//! - `/bootstrap.json`: its dialable multiaddrs (browsers start here);
//! - `/genesis.json`;
//! - `/status`, `/net`, `/zone/<z>`: the embedded node's view;
//! - `POST /faucet {"node_id": hex}`: grants test stake (rate limited);
//! - everything else: the built web UI (`--serve-web`).

use crate::genesis_cmd::load_genesis;
use crate::keys;
use crate::node_cmd::{self, Started};
use axum::extract::{ConnectInfo, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use umis_core::NodeId;
use umis_net::Role;
use umis_net::libp2p::{Multiaddr, multiaddr::Protocol};
use umis_node::Handle;
use umis_protocol::{Command, NodeConfig};

#[derive(clap::Args, Debug, Clone)]
pub struct BootstrapArgs {
    #[arg(long, default_value = "genesis/devnet.json")]
    pub genesis: PathBuf,
    /// Directory for the node key and WebRTC certificate.
    #[arg(long, default_value = ".devnet/bootstrap")]
    pub data_dir: PathBuf,
    /// The faucet key named in genesis (from `umis-cli genesis`).
    #[arg(long, default_value = ".devnet/keys/faucet.key")]
    pub faucet_key: PathBuf,
    /// libp2p listen addresses.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "/ip4/127.0.0.1/udp/9090/quic-v1,/ip4/127.0.0.1/udp/9091/webrtc-direct,/ip4/127.0.0.1/tcp/9092/ws"
    )]
    pub listen: Vec<Multiaddr>,
    /// HTTP address for bootstrap.json, genesis.json, status, faucet and the web UI.
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub http: SocketAddr,
    /// Directory with the built web UI to serve at `/`.
    #[arg(long)]
    pub serve_web: Option<PathBuf>,
    /// Test stake per faucet grant.
    #[arg(long, default_value_t = 500)]
    pub grant: u64,
}

#[derive(Clone)]
struct AppState {
    handle: Handle,
    genesis: Arc<String>,
    grant: u64,
    limits: Arc<Mutex<HashMap<IpAddr, Vec<Instant>>>>,
}

async fn bootstrap_json(State(s): State<AppState>) -> Json<serde_json::Value> {
    let Some(st) = s.handle.net_status().await else { return Json(serde_json::json!({})) };
    let addrs: Vec<String> = st
        .listen
        .iter()
        .filter_map(|a| a.parse::<Multiaddr>().ok())
        .map(|a| a.with(Protocol::P2p(st.peer_id.parse().expect("peer id"))).to_string())
        .collect();
    Json(serde_json::json!({ "peer_id": st.peer_id, "addrs": addrs }))
}

async fn faucet_handler(
    State(s): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let Some(to) = body["node_id"].as_str().and_then(|h| NodeId::from_hex(h).ok()) else {
        return Json(serde_json::json!({ "ok": false, "error": "node_id must be 64 hex characters" }));
    };
    {
        // At most 20 grants per minute per IP (browsers on one machine share an IP).
        let mut l = s.limits.lock().expect("limits");
        let v = l.entry(addr.ip()).or_default();
        v.retain(|t| t.elapsed().as_secs() < 60);
        if v.len() >= 20 {
            return Json(serde_json::json!({ "ok": false, "error": "rate limited" }));
        }
        v.push(Instant::now());
    }
    s.handle.command(Command::Grant { to, amount: s.grant });
    Json(serde_json::json!({ "ok": true, "amount": s.grant }))
}

pub async fn run(a: BootstrapArgs) -> anyhow::Result<()> {
    let g = load_genesis(&a.genesis)?;
    let key = keys::load_or_create_secret(&a.data_dir.join("node.key"))?;
    let faucet = keys::load_or_create_secret(&a.faucet_key)?;
    anyhow::ensure!(faucet.node_id() == g.faucet, "faucet key does not match genesis.faucet");
    let mut cfg = NodeConfig::new(key.clone());
    cfg.faucet = Some(faucet);
    cfg.send_heartbeats = false;
    cfg.follow_all = true;
    let Started { runtime, handle } =
        node_cmd::start(&g, cfg, Role::Bootstrap, a.listen.clone(), Vec::new(), &a.data_dir).await?;
    let state = AppState {
        handle: handle.clone(),
        genesis: Arc::new(g.to_json_pretty()),
        grant: a.grant,
        limits: Arc::new(Mutex::new(HashMap::new())),
    };
    let mut app = Router::new()
        .route("/bootstrap.json", get(bootstrap_json))
        .route("/genesis.json", get(|State(s): State<AppState>| async move { (*s.genesis).clone() }))
        .route("/faucet", post(faucet_handler))
        .with_state(state)
        .merge(node_cmd::status_router(handle.clone()));
    if let Some(dir) = &a.serve_web {
        app = app.fallback_service(ServeDir::new(dir));
    }
    let app = app.layer(CorsLayer::permissive());
    let listener = tokio::net::TcpListener::bind(a.http).await?;
    tracing::info!(peer = %keys::peer_id_of(&key), "bootstrap for genesis {} on http://{}", g.genesis8(), a.http);
    std::fs::create_dir_all(&a.data_dir)?;
    std::fs::write(a.data_dir.join("http.addr"), format!("http://{}", a.http))?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await {
            tracing::error!("http: {e}");
        }
    });
    let h = handle.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        h.stop();
    });
    runtime.run().await;
    Ok(())
}
