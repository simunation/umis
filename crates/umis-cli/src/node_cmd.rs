//! `umis-cli node` (native full node or member) and the pieces it shares
//! with `bootstrap`: protocol context, runtime start-up, HTTP status.

use crate::genesis_cmd::load_genesis;
use crate::keys;
use anyhow::Context;
use axum::extract::{Path as AxPath, State};
use axum::routing::get;
use axum::{Json, Router};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use umis_core::genesis::Genesis;
use umis_net::libp2p::Multiaddr;
use umis_net::{Net, NetConfig, Role};
use umis_node::{Handle, Runtime};
use umis_protocol::{Command, Ctx, Node, NodeConfig};

pub fn proof_backend(g: &Genesis) -> anyhow::Result<Arc<dyn umis_proof::ProofBackend>> {
    task_water3d::air::backend_for(g).map_err(|e| anyhow::anyhow!(e))
}

pub fn build_ctx(g: &Genesis) -> anyhow::Result<Arc<Ctx>> {
    Ok(Arc::new(Ctx::new(g.clone(), crate::registry(), proof_backend(g)?)))
}

pub fn webrtc_pem(dir: &Path) -> anyhow::Result<String> {
    let p = dir.join("webrtc.pem");
    if let Ok(s) = std::fs::read_to_string(&p) {
        return Ok(s);
    }
    std::fs::create_dir_all(dir)?;
    let s = umis_net::swarm::generate_webrtc_cert_pem()?;
    std::fs::write(&p, &s)?;
    Ok(s)
}

/// Bootstrap multiaddrs: given directly, or fetched from `<url>/bootstrap.json`.
pub async fn resolve_bootstrap(addrs: &[Multiaddr], http: Option<&str>) -> anyhow::Result<Vec<Multiaddr>> {
    let mut out = addrs.to_vec();
    if let Some(url) = http {
        let info: serde_json::Value =
            reqwest::get(format!("{url}/bootstrap.json")).await?.json().await.context("bootstrap.json")?;
        for a in info["addrs"].as_array().into_iter().flatten() {
            if let Some(s) = a.as_str() {
                out.push(s.parse()?);
            }
        }
    }
    Ok(out)
}

pub struct Started {
    pub runtime: Runtime,
    pub handle: Handle,
}

pub async fn start(
    g: &Genesis,
    mut cfg: NodeConfig,
    role: Role,
    listen: Vec<Multiaddr>,
    bootstrap: Vec<Multiaddr>,
    data_dir: &Path,
) -> anyhow::Result<Started> {
    let ctx = build_ctx(g)?;
    // One job at a time gets the whole prover pool (lowest latency per proof).
    cfg.max_jobs = 1;
    let node = Node::new(ctx, cfg.clone()).map_err(|e| anyhow::anyhow!(e))?;
    let mut nc = NetConfig::new(role, keys::keypair_of(&cfg.key));
    nc.listen = listen;
    nc.bootstrap = bootstrap;
    nc.webrtc_cert_pem = Some(webrtc_pem(data_dir)?);
    let topics = umis_net::Topics::new(g.genesis8());
    nc.register = vec![topics.ns_all()];
    nc.want = vec![umis_net::Want { namespace: topics.ns_all(), target: 8 }];
    let net = Net::new(nc).await?;
    let backend = node.ctx.backend.clone();
    let (mut runtime, handle) = Runtime::new(node, net);
    // Proving jobs (STARK backend) run on worker threads.
    runtime.set_executor(Box::new(umis_node::ThreadExecutor::new(crate::registry(), backend, cfg.key)));
    Ok(Started { runtime, handle })
}

pub fn status_router(handle: Handle) -> Router {
    Router::new()
        .route(
            "/status",
            get(|State(h): State<Handle>| async move {
                Json(serde_json::to_value(h.snapshot().await).unwrap_or_default())
            }),
        )
        .route(
            "/net",
            get(|State(h): State<Handle>| async move { Json(serde_json::to_value(h.net_status().await).unwrap_or_default()) }),
        )
        .route(
            "/tx",
            axum::routing::post(|State(h): State<Handle>, body: String| async move {
                let tx = hex::decode(body.trim())
                    .ok()
                    .and_then(|b| borsh::from_slice::<umis_protocol::types::SignedTx>(&b).ok());
                match tx {
                    Some(t) => {
                        h.command(Command::SubmitTx(Box::new(t)));
                        Json(serde_json::json!({ "ok": true }))
                    }
                    None => Json(serde_json::json!({ "ok": false, "error": "body must be a hex borsh SignedTx" })),
                }
            }),
        )
        .route(
            "/vote",
            axum::routing::post(|State(h): State<Handle>, Json(v): Json<serde_json::Value>| async move {
                match v["proposal_id"].as_str().and_then(|s| umis_core::Hash::from_hex(s).ok()) {
                    Some(proposal_id) => {
                        h.command(Command::Vote { proposal_id, accept: v["accept"].as_bool().unwrap_or(true) });
                        Json(serde_json::json!({ "ok": true }))
                    }
                    None => Json(serde_json::json!({ "ok": false, "error": "proposal_id" })),
                }
            }),
        )
        .route(
            "/metrics",
            get(|State(h): State<Handle>| async move {
                let Some(s) = h.snapshot().await else { return String::new() };
                let rows = [
                    ("head_slot", s.head_slot.unwrap_or(0)),
                    ("finalized_slot", s.finalized_slot.unwrap_or(0)),
                    ("peers", s.peers as u64),
                    ("minted_total", s.minted),
                    ("proposed_total", s.proposed),
                    ("faults_reported_total", s.faults_reported),
                    ("zone_blocks_accepted_total", s.zone_blocks_accepted),
                    ("invalid_blocks_total", s.invalid_blocks),
                    ("zone_forks_total", s.forks),
                    ("proofs_made_total", s.proved),
                    ("proof_jobs_dropped_total", s.jobs_dropped),
                    ("slashes", s.slashes as u64),
                    ("rollbacks", s.rollbacks as u64),
                    ("tx_pool", s.tx_pool as u64),
                    ("stake", s.stake),
                ];
                rows.iter().map(|(k, v)| format!("simu_{k} {v}\n")).collect::<String>()
            }),
        )
        .route(
            "/lattice",
            get(|State(h): State<Handle>, axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, u64>>| async move {
                let to = q.get("to").copied().unwrap_or(u64::MAX);
                let from = q.get("from").copied().unwrap_or(0);
                Json(serde_json::to_value(h.lattice(from, to).await).unwrap_or_default())
            }),
        )
        .route(
            "/zone/{z}",
            get(|State(h): State<Handle>, AxPath(z): AxPath<u32>| async move {
                Json(serde_json::to_value(h.render(z).await).unwrap_or_default())
            }),
        )
        .with_state(handle)
}

pub async fn serve(addr: SocketAddr, router: Router) -> anyhow::Result<SocketAddr> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!("http: {e}");
        }
    });
    Ok(bound)
}

#[derive(clap::Args, Debug)]
pub struct NodeArgs {
    #[arg(long, default_value = "genesis/devnet.json")]
    pub genesis: PathBuf,
    /// Node key (hex secret); created if missing.
    #[arg(long)]
    pub key: PathBuf,
    #[arg(long, default_value = ".devnet/node")]
    pub data_dir: PathBuf,
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "/ip4/127.0.0.1/udp/0/quic-v1,/ip4/127.0.0.1/udp/0/webrtc-direct,/ip4/127.0.0.1/tcp/0/ws"
    )]
    pub listen: Vec<Multiaddr>,
    /// Bootstrap multiaddrs (with /p2p/<id>).
    #[arg(long, value_delimiter = ',')]
    pub bootstrap: Vec<Multiaddr>,
    /// Or: the bootstrap's HTTP base URL (fetches bootstrap.json).
    #[arg(long)]
    pub bootstrap_http: Option<String>,
    /// HTTP status endpoint.
    #[arg(long, default_value = "127.0.0.1:0")]
    pub http: SocketAddr,
    /// Not a genesis validator: get faucet stake and join (`member` or `full`).
    #[arg(long)]
    pub join: Option<String>,
}

pub async fn node(a: NodeArgs) -> anyhow::Result<()> {
    let g = load_genesis(&a.genesis)?;
    let key = keys::load_or_create_secret(&a.key)?;
    std::fs::create_dir_all(&a.data_dir)?;
    let bootstrap = resolve_bootstrap(&a.bootstrap, a.bootstrap_http.as_deref()).await?;
    let cfg = NodeConfig::new(key.clone());
    let Started { runtime, handle } = start(&g, cfg, Role::Node, a.listen.clone(), bootstrap, &a.data_dir).await?;
    let bound = serve(a.http, status_router(handle.clone())).await?;
    std::fs::write(a.data_dir.join("http.addr"), format!("http://{bound}"))?;
    tracing::info!(node = %key.node_id().short(), peer = %keys::peer_id_of(&key), "node up, status on http://{bound}/status");
    if let Some(kind) = a.join.clone() {
        let full = kind == "full";
        let url = a.bootstrap_http.clone().context("--join needs --bootstrap-http (faucet)")?;
        let h = handle.clone();
        let me = key.node_id();
        tokio::spawn(async move {
            join_flow(h, url, me, full).await;
        });
    }
    let h = handle.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        h.stop();
    });
    runtime.run().await;
    Ok(())
}

/// Asks the faucet for stake, waits until it is on chain, then joins.
pub async fn join_flow(h: Handle, faucet: String, me: umis_core::NodeId, full_node: bool) {
    let client = reqwest::Client::new();
    loop {
        let Some(s) = h.snapshot().await else { return };
        if s.kind.is_some() {
            tracing::info!("joined as {:?} in zone {:?}", s.kind, s.zone);
            return;
        }
        if s.balance >= 200 {
            h.command(Command::Join { stake: 200, full_node });
        } else if s.head_slot.is_some() {
            let _ = client
                .post(format!("{faucet}/faucet"))
                .json(&serde_json::json!({ "node_id": me.to_hex() }))
                .send()
                .await;
        }
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
    }
}

#[derive(clap::Args, Debug)]
pub struct StatusArgs {
    /// Status URLs; default: every `.devnet/*/http.addr`.
    #[arg(long, value_delimiter = ',')]
    pub nodes: Vec<String>,
    #[arg(long, default_value = ".devnet")]
    pub dir: PathBuf,
    /// Print every node and check agreement.
    #[arg(long)]
    pub all: bool,
}

pub async fn status(a: StatusArgs) -> anyhow::Result<()> {
    let mut urls = a.nodes.clone();
    if urls.is_empty() {
        for e in std::fs::read_dir(&a.dir).with_context(|| format!("reading {}", a.dir.display()))? {
            let p = e?.path().join("http.addr");
            if let Ok(s) = std::fs::read_to_string(&p) {
                urls.push(s.trim().to_string());
            }
        }
        urls.sort();
    }
    let client = reqwest::Client::new();
    let mut fins = std::collections::BTreeMap::new();
    let mut ok = true;
    for u in &urls {
        let s: serde_json::Value = match client.get(format!("{u}/status")).send().await {
            Ok(r) => r.json().await.unwrap_or_default(),
            Err(e) => {
                println!("{u}: unreachable ({e})");
                ok = false;
                continue;
            }
        };
        let fin = (s["finalized_slot"].as_u64().unwrap_or(0), s["finalized_hash"].as_str().unwrap_or("").to_string());
        println!(
            "{u}: node {} {} zone {} head {} fin {} {} peers {} minted {} proposed {}",
            &s["node_id"].as_str().unwrap_or("?")[..8.min(s["node_id"].as_str().unwrap_or("").len())],
            s["kind"].as_str().unwrap_or("-"),
            s["zone"],
            s["head_slot"],
            fin.0,
            &fin.1[..8.min(fin.1.len())],
            s["peers"],
            s["minted"],
            s["proposed"],
        );
        if a.all {
            for z in s["zones"].as_array().into_iter().flatten() {
                println!("    zone {} tip {} commit {}", z["zone"], z["tip"], z["last_commit"]);
            }
        }
        fins.entry(fin.0).or_insert_with(std::collections::BTreeSet::new).insert(fin.1);
    }
    for (slot, hashes) in &fins {
        if hashes.len() > 1 {
            println!("DISAGREEMENT at finalized slot {slot}: {hashes:?}");
            ok = false;
        }
    }
    println!("{}", if ok { "finalized hashes agree" } else { "status check failed" });
    if !ok {
        anyhow::bail!("status check failed");
    }
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct ProposeArgs {
    /// `x,y,z` lattice point.
    #[arg(long, value_delimiter = ',')]
    pub point: Vec<i32>,
    #[arg(long, default_value = "water")]
    pub layer: String,
    #[arg(long, default_value_t = 16)]
    pub slot_ticks: u64,
    /// Proposer key (must hold min_stake).
    #[arg(long)]
    pub key: PathBuf,
    /// Founder keys (each co-signs; founders must be zone members).
    #[arg(long = "founder-key", required = true)]
    pub founder_keys: Vec<PathBuf>,
    #[arg(long, default_value = "genesis/devnet.json")]
    pub genesis: PathBuf,
    /// Node HTTP endpoint that submits the tx.
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
}

/// `umis-cli propose-zone --point 2,1,0 --layer water --slot-ticks 4 --key k --founder-key f…`
pub async fn propose_zone(a: ProposeArgs) -> anyhow::Result<()> {
    anyhow::ensure!(a.point.len() == 3, "--point needs x,y,z");
    anyhow::ensure!(a.layer == task_water3d::LAYER, "this build only creates water zones");
    let g = load_genesis(&a.genesis)?;
    let key = keys::load_or_create_secret(&a.key)?;
    let founders: Vec<umis_core::SecretKey> =
        a.founder_keys.iter().map(|p| keys::load_or_create_secret(p)).collect::<Result<_, _>>()?;
    let n_loc = g.zones.first().and_then(|z| z.params["n_loc"].as_u64()).unwrap_or(16) as u32;
    let point = [a.point[0], a.point[1], a.point[2]];
    let params = borsh::to_vec(&task_water3d::devnet::zone_params(point, a.slot_ticks, n_loc))?;
    let refs: Vec<&umis_core::SecretKey> = founders.iter().collect();
    let p = umis_protocol::types::build_propose_tx(
        key.node_id(),
        &refs,
        point,
        &a.layer,
        task_water3d::TASK_ID,
        params,
        a.slot_ticks,
    );
    let id = p.proposal_id();
    let tx = umis_protocol::types::Tx::ProposeZone(p);
    let signed = umis_protocol::types::SignedTx { sig: key.sign(&tx.address()), tx };
    let r: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/tx", a.node))
        .body(hex::encode(borsh::to_vec(&signed)?))
        .send()
        .await?
        .json()
        .await?;
    println!("proposal {id}: {r}");
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct VoteArgs {
    pub proposal_id: String,
    /// `accept` or `reject`
    pub decision: String,
    /// HTTP endpoint of the voting node (a member of an adjacent zone).
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
}

pub async fn vote(a: VoteArgs) -> anyhow::Result<()> {
    let accept = match a.decision.as_str() {
        "accept" => true,
        "reject" => false,
        _ => anyhow::bail!("decision must be accept or reject"),
    };
    let r: serde_json::Value = reqwest::Client::new()
        .post(format!("{}/vote", a.node))
        .json(&serde_json::json!({ "proposal_id": a.proposal_id, "accept": accept }))
        .send()
        .await?
        .json()
        .await?;
    println!("{r}");
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct ProposalsArgs {
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,
}

pub async fn proposals(a: ProposalsArgs) -> anyhow::Result<()> {
    let s: serde_json::Value = reqwest::get(format!("{}/status", a.node)).await?.json().await?;
    for p in s["proposals"].as_array().into_iter().flatten() {
        println!(
            "{} at {} {}: {} (deadline {}) tally {}",
            p["id"], p["point"], p["layer"], p["status"], p["deadline"], p["tally"]
        );
    }
    Ok(())
}
