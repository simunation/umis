//! Deterministic multi-node harness for the sans-IO protocol: a simulated
//! clock and network, scenarios (churn, partitions, adversaries, faults,
//! zone creation), and an oracle checking the plan's invariants.

pub mod net;
pub mod oracle;
pub mod probe;
pub mod scenarios;

use net::{NetModel, SimNet};
use std::sync::Arc;
use task_water3d::devnet::{DevnetOptions, devnet_genesis};
use umis_core::genesis::{Genesis, GenesisValidator, ValidatorKind};
use umis_core::{Hash, NodeId, SecretKey};
use umis_protocol::ctx::CachingVerifier;
use umis_protocol::{Ctx, Node, NodeConfig};

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub seed: u64,
    /// Genesis members per genesis zone.
    pub members_per_zone: usize,
    /// Genesis native full nodes.
    pub full_nodes: usize,
    pub n_loc: u32,
    pub slot_ticks: Vec<u64>,
    pub proof_lag: u64,
    pub model: NetModel,
    /// Add the test-only `probe` layer coupled to water's top face.
    pub probe_layer: bool,
    /// `mock` or `plonky3` (with the fast, insecure test FRI preset).
    pub proof_backend: String,
}

impl SimConfig {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            members_per_zone: 8,
            full_nodes: 2,
            n_loc: 4,
            slot_ticks: vec![4, 8, 4, 16],
            proof_lag: 0,
            model: NetModel::default(),
            probe_layer: false,
            proof_backend: "mock".into(),
        }
    }
}

pub const GENESIS_MS: u64 = 10_000;

pub struct Sim {
    pub cfg: SimConfig,
    pub net: SimNet,
    pub ctx: Arc<Ctx>,
    pub genesis: Genesis,
    pub keys: Vec<SecretKey>,
    pub faucet: SecretKey,
    /// Nodes expected to behave honestly.
    pub honest: Vec<bool>,
    /// Fault keys `(minter, zone, slot)` the scenario injected.
    pub injected: Vec<(NodeId, u32, u64)>,
    /// An honest full node keeping all zone history (for replay checks).
    pub archive: usize,
}

pub fn registry() -> umis_task::TaskRegistry {
    let mut r = umis_task::TaskRegistry::new();
    task_water3d::register(&mut r);
    r.register::<probe::Probe>();
    r
}

impl Sim {
    pub fn new(cfg: SimConfig) -> Self {
        let faucet = SecretKey::from_label(&format!("faucet-{}", cfg.seed));
        let mut keys = Vec::new();
        let mut validators = Vec::new();
        for i in 0..cfg.full_nodes {
            let k = SecretKey::from_label(&format!("full-{}-{i}", cfg.seed));
            validators.push(GenesisValidator {
                node: k.node_id(),
                stake: 1000,
                kind: ValidatorKind::FullNode,
                zone: None,
            });
            keys.push(k);
        }
        for z in 0..cfg.slot_ticks.len() as u32 {
            for m in 0..cfg.members_per_zone {
                let k = SecretKey::from_label(&format!("member-{}-{z}-{m}", cfg.seed));
                validators.push(GenesisValidator {
                    node: k.node_id(),
                    stake: 500,
                    kind: ValidatorKind::Member,
                    zone: Some(z),
                });
                keys.push(k);
            }
        }
        let seed = umis_core::keccak256(&cfg.seed.to_be_bytes());
        let mut o = DevnetOptions::new(seed, GENESIS_MS, faucet.node_id());
        o.validators = validators;
        o.n_loc = cfg.n_loc;
        o.slot_ticks = cfg.slot_ticks.clone();
        o.params.proof_lag = cfg.proof_lag;
        let mut genesis = devnet_genesis(o);
        if cfg.probe_layer {
            genesis.layers.push(probe::PROBE_LAYER.into());
            genesis.couplings.push(umis_core::genesis::CouplingSpec {
                layer_a: "water".into(),
                port_a: probe::WATER_TOP,
                layer_b: probe::PROBE_LAYER.into(),
                port_b: probe::PROBE_PORT,
            });
        }
        genesis.proof_backend = cfg.proof_backend.clone();
        if cfg.proof_backend == task_water3d::air::BACKEND_ID {
            genesis.stark_config_id = Some("koalabear-test".into());
        }
        genesis.validate().expect("valid genesis");
        let backend = task_water3d::air::backend_for(&genesis).expect("backend");
        let ctx = Arc::new(
            Ctx::new(genesis.clone(), registry(), backend).with_verifier(Arc::new(CachingVerifier::default())),
        );
        let mut net = SimNet::new(cfg.seed, cfg.model.clone());
        for (i, k) in keys.iter().enumerate() {
            let mut nc = NodeConfig::new(k.clone());
            if i == 0 {
                nc.zone_keep_slots = u64::MAX / 4;
                nc.faucet = Some(faucet.clone());
            }
            let label = if i < cfg.full_nodes {
                format!("full{i}")
            } else {
                let m = i - cfg.full_nodes;
                format!("z{}m{}", m / cfg.members_per_zone.max(1), m % cfg.members_per_zone.max(1))
            };
            net.add(Node::new(ctx.clone(), nc).expect("node"), label);
        }
        let n = keys.len();
        let mut sim =
            Sim { cfg, net, ctx, genesis, keys, faucet, honest: vec![true; n], injected: Vec::new(), archive: 0 };
        sim.net.connect_all();
        sim
    }

    /// Adds a node that is not in genesis (it must get a grant and join).
    pub fn add_node(&mut self, label: &str) -> usize {
        let k = SecretKey::from_label(&format!("late-{}-{label}", self.cfg.seed));
        let node = Node::new(self.ctx.clone(), NodeConfig::new(k.clone())).expect("node");
        let i = self.net.add(node, label.to_string());
        self.keys.push(k);
        self.honest.push(true);
        // connect it
        for j in 0..self.net.nodes.len() {
            if j != i {
                let pj = self.net.nodes[j].node.me;
                let pi = self.net.nodes[i].node.me;
                self.net.deliver(i, umis_protocol::Event::PeerConnected(pj));
                self.net.deliver(j, umis_protocol::Event::PeerConnected(pi));
            }
        }
        i
    }

    pub fn slot_start(&self, e: u64) -> u64 {
        self.ctx.grid.beacon_slot_start_ms(e)
    }

    /// Runs until the start of beacon slot `e`.
    pub fn run_to_slot(&mut self, e: u64) {
        if std::env::var("SIMNET_PROGRESS").is_err() {
            let t = self.slot_start(e);
            self.net.run_until(t);
            return;
        }
        let cur = self.ctx.grid.beacon_slot_at(self.net.now).unwrap_or(0);
        for s in cur + 1..=e {
            let t0 = std::time::Instant::now();
            let ev = self.net.events;
            self.net.run_until(self.slot_start(s));
            let a = self.node(self.archive);
            let st = a.chain.head_state();
            eprintln!(
                "  slot {s}: {:.0?} events {} head {:?} rollbacks {} slashes {} pools {:?}",
                t0.elapsed(),
                self.net.events - ev,
                a.chain.head().slot,
                st.rollback_log.len(),
                st.slash_log.len(),
                a.zc_pool_keys().len()
            );
        }
    }

    pub fn node(&self, i: usize) -> &Node {
        &self.net.nodes[i].node
    }

    pub fn index_of(&self, n: &NodeId) -> Option<usize> {
        self.net.index.get(n).copied()
    }

    pub fn command(&mut self, i: usize, c: umis_protocol::Command) {
        self.net.deliver(i, umis_protocol::Event::Command(c));
    }

    /// Honest online nodes.
    pub fn honest_online(&self) -> Vec<usize> {
        (0..self.net.nodes.len()).filter(|i| self.honest[*i] && self.net.nodes[*i].online).collect()
    }

    /// Hash of every honest node's canonical chain (determinism check).
    pub fn trace_hash(&self) -> Hash {
        let mut parts: Vec<u8> = Vec::new();
        for i in 0..self.net.nodes.len() {
            let n = self.node(i);
            parts.extend_from_slice(&n.chain.head().hash.0);
            parts.extend_from_slice(&n.chain.finalized().hash.0);
        }
        umis_core::keccak256(&parts)
    }
}
