//! Runs a stencil world with real proofs: `rx × ry` zones in lockstep exchange their halos;
//! every slot, every zone's transition is proved by its STARK (the protocol backend,
//! `StencilStark`) and verified from the public data alone, and the world is drawn.
//!
//! ```text
//! cargo run --release -p task-stencil --example world -- --rule lbm --n 32 --region 4,2 --slots 40 --frames out/
//! ```
//! Options: `--rule life|heat|lbm`, `--n` cells per zone side, `--k` generations per slot,
//! `--region rx,ry`, `--slots`, `--preset p100|test`, `--frames DIR` (PPM per slot),
//! `--pattern 0|1`, `--prove-every N` (prove every N-th slot; 1 = all).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;
use task_stencil::air::StencilStark;
use task_stencil::devnet::{StencilOptions, default_rule};
use task_stencil::{Stencil2d, decode_state, render};
use umis_core::records::{InputRef, TransitionStatement};
use umis_core::{Hash, NodeId, SecretKey, bmt};
use umis_proof::{ProofBackend, PublicData, Witness};
use umis_stencil::Rule;
use umis_task::topology::opposite_face;
use umis_task::{Bytes, DynZoneTask, PortId, TaskAdapter, ZoneInstance};

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

fn main() {
    let rule_name = arg("--rule").unwrap_or_else(|| "lbm".into());
    let rule_id = default_rule(&rule_name).expect("--rule life|heat|lbm");
    let n: u32 = arg("--n").map_or(32, |v| v.parse().expect("--n"));
    let k: u32 = arg("--k").map_or(1, |v| v.parse().expect("--k"));
    let region: Vec<i32> =
        arg("--region").map_or(vec![2, 2], |v| v.split(',').map(|x| x.parse().expect("--region")).collect());
    let slots: u64 = arg("--slots").map_or(20, |v| v.parse().expect("--slots"));
    let every: u64 = arg("--prove-every").map_or(1, |v| v.parse().expect("--prove-every"));
    let pattern: u32 = arg("--pattern").map_or(0, |v| v.parse().expect("--pattern"));
    let preset = arg("--preset").unwrap_or_else(|| "p100".into());
    let frames = arg("--frames").map(std::path::PathBuf::from);
    let backend = StencilStark::from_config_id(Some(if preset == "test" { "koalabear-test" } else { "koalabear" }));

    let seed = umis_core::keccak256(b"stencil-world");
    let mut o = StencilOptions::new(seed, 0, NodeId::default(), rule_id);
    (o.n, o.gens, o.region, o.rows_per_trace_row, o.pattern) = (n, k, [region[0], region[1]], 2.max(k), pattern);
    let mut zones: BTreeMap<[i32; 3], (Vec<u8>, Arc<dyn ZoneInstance>, Bytes)> = BTreeMap::new();
    for pt in o.genesis_points() {
        let params = borsh::to_vec(&o.zone_params(pt, 4)).expect("params");
        let inst = TaskAdapter::<Stencil2d>::new().instantiate(&params, &seed).expect("zone");
        let s0 = inst.initial_state(0);
        zones.insert(pt, (params, inst, s0));
    }
    println!("{rule_name}: {} zones of {n}² cells, k = {k}, preset {preset}, {} slots", zones.len(), slots);
    if let Some(d) = &frames {
        std::fs::create_dir_all(d).expect("frames dir");
    }
    let boundary = rule_id.rule().boundary();
    let (mut proved, mut total_prove, mut total_verify, mut bytes) = (0usize, 0f64, 0f64, 0usize);
    for slot in 0..slots {
        let outs: BTreeMap<_, _> = zones.iter().map(|(pt, (_, i, s))| (*pt, i.outputs(s).expect("outputs"))).collect();
        let mut next = BTreeMap::new();
        let (mut slot_prove, mut slot_verify) = (0f64, 0f64);
        for (pt, (params, inst, s)) in &zones {
            let p = o.zone_params(*pt, 4);
            let mut inputs: BTreeMap<PortId, Bytes> = BTreeMap::new();
            if slot > 0 {
                for q in task_stencil::PORTS {
                    if let Some(out) = outs.get(&p.neighbour_point(q)) {
                        inputs.insert(q, out[&opposite_face(q)].clone());
                    }
                }
            }
            let nx = inst.step(s, &inputs, slot).expect("step");
            if slot % every == 0 {
                let refs: Vec<InputRef> = inputs
                    .iter()
                    .map(|(q, b)| InputRef {
                        link_id: *q,
                        src_zone: 0,
                        src_slot: slot.saturating_sub(1),
                        src_block_hash: Hash::ZERO,
                        src_outputs_root: Hash::ZERO,
                        payload_address: bmt::file_address(b),
                    })
                    .collect();
                let leaves: Vec<Hash> =
                    inst.outputs(&nx).expect("outputs").values().map(|b| bmt::file_address(b)).collect();
                let stmt = TransitionStatement {
                    zone: 0,
                    from_slot: slot,
                    to_slot: slot + 1,
                    params_hash: umis_task::params_hash(task_stencil::TASK_ID, task_stencil::LAYER, params),
                    prev_state_root: bmt::file_address(s),
                    state_root: bmt::file_address(&nx),
                    inputs_commitment: umis_core::records::list_root_of(&refs),
                    outputs_root: bmt::list_root(&leaves),
                };
                let w = Witness { instance: inst.as_ref(), params, genesis_seed: seed, prev_state: s, inputs: &inputs };
                let t = Instant::now();
                let proof = backend.prove(&stmt, &w, &SecretKey::from_label("prover")).expect("prove");
                let tp = t.elapsed().as_secs_f64() * 1e3;
                let pd = PublicData {
                    task_id: Some(task_stencil::TASK_ID),
                    params: Some(params),
                    genesis_seed: Some(seed),
                    prev_state: Some(s),
                    next_state: Some(&nx),
                    inputs: Some(&inputs),
                    input_refs: Some(&refs),
                    prev_witness: None,
                    next_witness: None,
                };
                let t = Instant::now();
                backend.verify(&stmt, &pd, &proof).unwrap_or_else(|e| panic!("zone {pt:?} slot {slot}: {e}"));
                let tv = t.elapsed().as_secs_f64() * 1e3;
                if proof[0] == 1 {
                    proved += 1;
                    bytes += proof.len();
                    total_prove += tp;
                    total_verify += tv;
                }
                slot_prove = slot_prove.max(tp);
                slot_verify = slot_verify.max(tv);
            }
            next.insert(*pt, nx);
        }
        for (pt, s) in next {
            zones.get_mut(&pt).expect("zone").2 = s;
        }
        let boards: Vec<_> = zones
            .iter()
            .map(|(pt, (_, _, s))| (*pt, decode_state(&o.zone_params(*pt, 4), s).expect("state").grid))
            .collect();
        let world = render::world(&boards, n as usize, &boundary);
        println!(
            "slot {slot:4}: gen {:4} | {} | slowest zone: prove {slot_prove:6.1} ms, verify {slot_verify:6.1} ms",
            (slot + 1) * k as u64,
            render::summary(&rule_id, &world)
        );
        if let Some(d) = &frames {
            let scale = (512 / world.w.max(1)).max(1);
            let (iw, ih, px) = render::rgb(&rule_id, &world, scale);
            render::write_ppm(&d.join(format!("frame-{slot:04}.ppm")), iw, ih, &px).expect("frame");
        }
    }
    if proved > 0 {
        println!(
            "{proved} zone slots proved and verified: mean prove {:.1} ms, mean verify {:.1} ms, mean proof {} KB",
            total_prove / proved as f64,
            total_verify / proved as f64,
            bytes / proved / 1024
        );
    }
}
