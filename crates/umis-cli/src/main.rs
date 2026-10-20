//! `umis-cli`: genesis, keygen, node, bootstrap and sim.

mod bootstrap;
mod genesis_cmd;
mod keys;
mod node_cmd;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "umis-cli", version, about = "simunation command line")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a bootstrap node: relay + rendezvous server and HTTP endpoints.
    Bootstrap(bootstrap::BootstrapArgs),
    /// Write a devnet genesis file (and validator/faucet keys).
    Genesis(genesis_cmd::GenesisArgs),
    /// Create a node key.
    Keygen(genesis_cmd::KeygenArgs),
    /// Physics reference: run the genesis domain without consensus.
    Sim(genesis_cmd::SimArgs),
    /// Run a native node (genesis validator, or `--join` a running devnet).
    Node(node_cmd::NodeArgs),
    /// Show and compare the status of running nodes.
    Status(node_cmd::StatusArgs),
    /// Propose a new zone at a free location (founders co-sign).
    ProposeZone(node_cmd::ProposeArgs),
    /// Vote on a proposal through a member node (manual vote policy).
    Vote(node_cmd::VoteArgs),
    /// List zone proposals and their tallies.
    Proposals(node_cmd::ProposalsArgs),
}

/// All tasks this build knows.
pub fn registry() -> umis_task::TaskRegistry {
    let mut r = umis_task::TaskRegistry::new();
    task_water3d::register(&mut r);
    r
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,libp2p_webrtc=warn,webrtc=warn".into()),
        )
        .init();
    match Cli::parse().cmd {
        Cmd::Bootstrap(a) => bootstrap::run(a).await,
        Cmd::Genesis(a) => genesis_cmd::genesis(a),
        Cmd::Keygen(a) => genesis_cmd::keygen(a),
        Cmd::Sim(a) => genesis_cmd::sim(a),
        Cmd::Node(a) => node_cmd::node(a).await,
        Cmd::Status(a) => node_cmd::status(a).await,
        Cmd::ProposeZone(a) => node_cmd::propose_zone(a).await,
        Cmd::Vote(a) => node_cmd::vote(a).await,
        Cmd::Proposals(a) => node_cmd::proposals(a).await,
    }
}
