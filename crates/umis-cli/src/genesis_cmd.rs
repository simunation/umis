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
    /// Zone task: `water` (3D shallow water, 2×2 genesis zones), `life` (Game of Life,
    /// `--region` genesis zones) or `stencil` (the generic local-rule task, `--rule`).
    #[arg(long, default_value = "water")]
    pub task: String,
    /// stencil: the local rule, `life`, `heat` or `lbm` (lattice Boltzmann fluid).
    #[arg(long, default_value = "lbm")]
    pub rule: String,
    /// Cells per zone axis.
    #[arg(long, default_value_t = 16)]
    pub n_loc: u32,
    /// life: generations per zone slot (= halo width).
    #[arg(long, default_value_t = 1)]
    pub gens: u32,
    /// life: genesis zones per axis, `rx,ry`.
    #[arg(long, value_delimiter = ',', default_value = "2,2")]
    pub region: Vec<i32>,
    /// life: soup density of the genesis zones, percent.
    #[arg(long, default_value_t = 30)]
    pub density: u32,
    /// life: STARK layout, vertical strips and board rows per trace row (default: 8 and 3
    /// from 256 cells per axis, the fastest measured there; 4 and 2 below).
    #[arg(long)]
    pub strips: Option<u32>,
    #[arg(long)]
    pub rows_per_trace_row: Option<u32>,
    /// life: genesis boards, `soup` or `showcase` (gliders through zone corners, a gun,
    /// spaceships leaving the world, methuselahs on junctions).
    #[arg(long, default_value = "soup")]
    pub pattern: String,
    /// Zone slot lengths in ticks of the genesis zones (water: 4 values;
    /// life: cycled over the zones).
    #[arg(long, value_delimiter = ',', default_value = "4,8,4,16")]
    pub slot_ticks: Vec<u64>,
    /// Proof backend: plonky3 (STARK-proven zone slots, L = 1) or mock
    /// (signed statements, faults caught by re-execution, L = 0).
    #[arg(long, default_value = "plonky3")]
    pub proof_backend: String,
    /// STARK configuration for plonky3: `koalabear-keccak` (life2d with Keccak Merkle trees:
    /// faster proving and ~2x faster verification; water3d keeps Poseidon2), `koalabear`
    /// (Poseidon2 Merkle trees everywhere), or their `-test` variants (insecure, fast).
    #[arg(long, default_value = "koalabear-keccak")]
    pub stark_config: String,
    /// `L_agg`: beacon slots after a commitment is proven during which aggregators may replace its
    /// leaf proofs by an aggregate (stencil with `--stark-config koalabear-agg[-test]`; 0: off, the
    /// committer aggregates inline). Run full nodes with `umis-cli node --aggregator`.
    #[arg(long, default_value_t = 0)]
    pub agg_lag: u64,
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
    let time = a.time.unwrap_or_else(|| now_ms() + a.start_in * 1000);
    let mut params = umis_core::genesis::ChainParams::default();
    if a.proof_backend == "plonky3" {
        params.proof_lag = 1;
    }
    params.agg_lag = a.agg_lag;
    let mut g = match a.task.as_str() {
        "water" => {
            anyhow::ensure!(a.slot_ticks.len() == 4, "--slot-ticks needs 4 values");
            let mut o = DevnetOptions::new(seed, time, faucet.node_id());
            o.validators = validators;
            o.n_loc = a.n_loc;
            o.slot_ticks = a.slot_ticks.clone();
            o.params = params;
            devnet_genesis(o)
        }
        "life" => {
            anyhow::ensure!(a.region.len() == 2, "--region needs rx,ry");
            anyhow::ensure!(!a.slot_ticks.is_empty(), "--slot-ticks needs a value");
            let mut o = task_life2d::devnet::LifeOptions::new(seed, time, faucet.node_id());
            o.validators = validators;
            o.n = a.n_loc;
            o.gens = a.gens;
            o.region = [a.region[0], a.region[1]];
            o.density_pct = a.density;
            let (strips, rpr) = if a.n_loc >= 256 { (8, 3) } else { (4, 2) };
            o.strips = a.strips.unwrap_or(strips);
            o.rows_per_trace_row = a.rows_per_trace_row.unwrap_or(rpr);
            o.pattern = match a.pattern.as_str() {
                "soup" => 0,
                "showcase" => 1,
                other => anyhow::bail!("--pattern soup|showcase, not {other}"),
            };
            o.slot_ticks = a.slot_ticks.clone();
            o.params = params;
            task_life2d::devnet::devnet_genesis(o)
        }
        "stencil" => {
            anyhow::ensure!(a.region.len() == 2, "--region needs rx,ry");
            anyhow::ensure!(!a.slot_ticks.is_empty(), "--slot-ticks needs a value");
            let rule = task_stencil::devnet::default_rule(&a.rule)
                .ok_or_else(|| anyhow::anyhow!("--rule life|heat|lbm, not {}", a.rule))?;
            let mut o = task_stencil::devnet::StencilOptions::new(seed, time, faucet.node_id(), rule);
            o.validators = validators;
            o.n = a.n_loc;
            o.gens = a.gens;
            o.region = [a.region[0], a.region[1]];
            o.rows_per_trace_row = a.rows_per_trace_row.unwrap_or(2.max(a.gens));
            o.pattern = match a.pattern.as_str() {
                "soup" | "random" => 0,
                "showcase" => 1,
                other => anyhow::bail!("--pattern random|showcase, not {other}"),
            };
            o.slot_ticks = a.slot_ticks.clone();
            o.params = params;
            task_stencil::devnet::devnet_genesis(o)
        }
        other => anyhow::bail!("unknown --task {other} (water, life or stencil)"),
    };
    g.proof_backend = a.proof_backend.clone();
    if a.proof_backend == "plonky3" {
        g.stark_config_id = Some(a.stark_config.clone());
        if !umis_core::genesis::SECURE_STARK_CONFIGS.contains(&a.stark_config.as_str()) {
            eprintln!(
                "WARNING: {} is an insecure STARK configuration (test only); genesis marked insecure_proofs",
                a.stark_config
            );
            g.insecure_proofs = true;
        }
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
    /// stencil: write a picture of the world (`frame-NNNN.ppm`) per beacon slot into this
    /// directory.
    #[arg(long)]
    pub frames: Option<PathBuf>,
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
        if g.layers.first().map(String::as_str) == Some(task_stencil::LAYER) {
            let mut zones = Vec::new();
            let mut rule = None;
            let mut n = 0;
            for z in s.zones.values() {
                let gz = g.zones.iter().find(|gz| gz.id == z.id).expect("genesis zone");
                let p: task_stencil::StencilParams = serde_json::from_value(gz.params.clone())?;
                let st = task_stencil::decode_state(&p, &z.state).context("stencil state")?;
                line += &format!(" z{} gen={:4}", z.id, st.generation);
                n = p.n();
                rule = Some(p.rule);
                zones.push((gz.point, st.grid));
            }
            if let Some(rule) = rule {
                let w = task_stencil::render::world(&zones, n, &umis_stencil_boundary(&rule));
                line += &format!(" | {}", task_stencil::render::summary(&rule, &w));
                if let Some(dir) = &a.frames {
                    std::fs::create_dir_all(dir)?;
                    let scale = (512 / w.w.max(1)).max(1);
                    let (iw, ih, px) = task_stencil::render::rgb(&rule, &w, scale);
                    task_stencil::render::write_ppm(&dir.join(format!("frame-{e:04}.ppm")), iw, ih, &px)?;
                }
            }
            println!("{line}");
            continue;
        }
        if g.layers.first().map(String::as_str) == Some(task_life2d::LAYER) {
            for z in s.zones.values() {
                let r = z.inst.render(&z.state)?;
                line += &format!(" z{} gen={:4} pop={:5}", z.id, r.meta["gen"], r.meta["population"]);
            }
            println!("{line}");
            continue;
        }
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

fn umis_stencil_boundary(rule: &task_stencil::RuleId) -> Vec<u32> {
    use umis_stencil::Rule;
    rule.rule().boundary()
}
