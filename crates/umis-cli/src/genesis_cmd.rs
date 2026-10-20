//! `umis-cli genesis`, `keygen` and `sim`.

use crate::keys;
use anyhow::Context;
use std::path::PathBuf;
use task_water3d::devnet::{DevnetOptions, devnet_genesis};
use umis_core::Hash;
use umis_core::genesis::{Genesis, GenesisValidator, ValidatorKind};

#[derive(clap::Args, Debug)]
pub struct GenesisArgs {
    /// `random` or 64 hex characters.
    #[arg(long, default_value = "random")]
    pub seed: String,
    /// Genesis time as Unix ms; default: now + `--start-in`.
    #[arg(long)]
    pub time: Option<u64>,
    /// Seconds from now until tick 0, when `--time` is not given.
    #[arg(long, default_value_t = 10)]
    pub start_in: u64,
    /// Number of genesis full-node validators to create keys for.
    #[arg(long, default_value_t = 4)]
    pub validators: usize,
    /// Directory for validator and faucet keys.
    #[arg(long, default_value = ".devnet/keys")]
    pub keys_dir: PathBuf,
    #[arg(long, default_value = "genesis/devnet.json")]
    pub out: PathBuf,
    /// Cells per zone axis.
    #[arg(long, default_value_t = 16)]
    pub n_loc: u32,
    /// Zone slot lengths in ticks for the 4 genesis zones.
    #[arg(long, value_delimiter = ',', default_value = "4,8,4,16")]
    pub slot_ticks: Vec<u64>,
    /// Proof backend: plonky3 (STARK-proven zone slots, L = 1) or mock
    /// (signed statements, faults caught by re-execution, L = 0).
    #[arg(long, default_value = "plonky3")]
    pub proof_backend: String,
    /// STARK configuration for plonky3: `koalabear` or `koalabear-test` (insecure, fast).
    #[arg(long, default_value = "koalabear")]
    pub stark_config: String,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock after 1970").as_millis() as u64
}

pub fn genesis(a: GenesisArgs) -> anyhow::Result<()> {
    let seed = if a.seed == "random" {
        Hash(rand::random::<[u8; 32]>())
    } else {
        Hash::from_hex(&a.seed).context("seed must be 64 hex characters")?
    };
    let faucet_path = a.keys_dir.join("faucet.key");
    let faucet = keys::load_or_create_secret(&faucet_path)?;
    let mut validators = Vec::new();
    for i in 0..a.validators {
        let k = keys::load_or_create_secret(&a.keys_dir.join(format!("node-{i}.key")))?;
        validators.push(GenesisValidator { node: k.node_id(), stake: 1000, kind: ValidatorKind::FullNode, zone: None });
    }
    anyhow::ensure!(a.slot_ticks.len() == 4, "--slot-ticks needs 4 values");
    let mut o = DevnetOptions::new(seed, a.time.unwrap_or_else(|| now_ms() + a.start_in * 1000), faucet.node_id());
    o.validators = validators;
    o.n_loc = a.n_loc;
    o.slot_ticks = a.slot_ticks.clone();
    if a.proof_backend == "plonky3" {
        o.params.proof_lag = 1;
    }
    let mut g = devnet_genesis(o);
    g.proof_backend = a.proof_backend.clone();
    if a.proof_backend == "plonky3" {
        g.stark_config_id = Some(a.stark_config.clone());
    }
    g.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
    let reg = crate::registry();
    for z in &g.zones {
        let t = reg.get(&z.task_id)?;
        t.validate_params(&t.params_from_json(&z.params)?)?;
    }
    if let Some(dir) = a.out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&a.out, g.to_json_pretty())?;
    println!("genesis {} ({}) written to {}", g.genesis8(), g.hash(), a.out.display());
    println!("  genesis time {} ms, seed {}", g.genesis_time_ms, g.seed);
    println!("  {} validators, keys in {}", g.validators.len(), a.keys_dir.display());
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct KeygenArgs {
    /// Where to write the key (hex secret).
    #[arg(long)]
    pub out: PathBuf,
}

pub fn keygen(a: KeygenArgs) -> anyhow::Result<()> {
    anyhow::ensure!(!a.out.exists(), "{} exists", a.out.display());
    let k = keys::load_or_create_secret(&a.out)?;
    println!("node_id {}", k.node_id());
    println!("peer_id {}", keys::peer_id_of(&k));
    Ok(())
}

#[derive(clap::Args, Debug)]
pub struct SimArgs {
    #[arg(long, default_value = "genesis/devnet.json")]
    pub genesis: PathBuf,
    /// Beacon slots to simulate.
    #[arg(long, default_value_t = 20)]
    pub beacon_slots: u64,
    /// Write the final zone states (raw blobs) into this directory.
    #[arg(long)]
    pub dump: Option<PathBuf>,
}

pub fn load_genesis(path: &std::path::Path) -> anyhow::Result<Genesis> {
    let s = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Genesis::from_json(&s).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Physics reference: the whole domain in one process, no consensus.
pub fn sim(a: SimArgs) -> anyhow::Result<()> {
    let g = load_genesis(&a.genesis)?;
    let reg = crate::registry();
    let mut s = umis_task::sim::from_genesis(&g, &reg)?;
    let b = g.params.beacon_slot_ticks;
    let t0 = std::time::Instant::now();
    for e in 0..a.beacon_slots {
        s.run_until((e + 1) * b)?;
        let mut line = format!("beacon slot {e:4}:");
        for z in s.zones.values() {
            let vals: Vec<i32> =
                z.state.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().expect("4 bytes"))).collect();
            let vmax = vals.chunks_exact(4).map(|c| c[..3].iter().map(|v| v.abs()).max().unwrap_or(0)).max();
            let dye: i64 = vals.chunks_exact(4).map(|c| c[3] as i64).sum();
            line += &format!(" z{} |v|max={:5} dye={:9}", z.id, vmax.unwrap_or(0), dye);
        }
        println!("{line}");
    }
    println!("trace {} in {:.2?}", s.trace_hash(), t0.elapsed());
    for z in s.zones.values() {
        println!("zone {} slot {} state_root {}", z.id, z.next_slot, z.inst.state_root(&z.state));
    }
    if let Some(dir) = a.dump {
        std::fs::create_dir_all(&dir)?;
        for z in s.zones.values() {
            std::fs::write(dir.join(format!("zone-{}.bin", z.id)), &z.state)?;
        }
    }
    Ok(())
}
