//! `cargo run --release -p task-life2d --features stark --example bench_life_air -- [n,k,strips,rpr ...]`
//!
//! One advancing life2d slot of a centre zone (8 live neighbours): native
//! step time vs proof generation (trace + STARK, benchmark FRI preset),
//! verification time and proof size. Thread count: `RAYON_NUM_THREADS`;
//! `LIFE_HASH=keccak` uses the Keccak Merkle config, `keccak-gpu` also the GPU
//! coset LDE (`--features cuda`).
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use task_life2d::air::{Life2dStark, MerkleHash, Shape, trace_cells};
use task_life2d::devnet::LifeOptions;
use task_life2d::{Decision, Life2d, decide, decode_state};
use umis_core::records::TransitionStatement;
use umis_core::{NodeId, SecretKey, bmt};
use umis_proof::{ProofBackend, PublicData, Witness};
use umis_stark::config::FriPreset;
use umis_task::ZoneTask;
use umis_task::{DynZoneTask, TaskAdapter};

fn best<T>(reps: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut out = None;
    let mut best = Duration::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        let v = f();
        best = best.min(t.elapsed());
        out = Some(v);
    }
    (best, out.unwrap())
}

fn main() {
    if std::env::var("TRACE").is_ok() {
        // Plonky3's info spans with their busy/idle times on close.
        tracing_subscriber::fmt()
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
            .with_target(false)
            .init();
    }
    let mut layouts: Vec<[u32; 4]> = std::env::args()
        .skip(1)
        .filter_map(|a| {
            let v: Vec<u32> = a.split(',').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
        })
        .collect();
    if layouts.is_empty() {
        layouts = vec![[64, 4, 1, 2], [64, 4, 2, 2], [64, 4, 4, 2], [64, 4, 4, 4], [64, 4, 8, 4], [64, 8, 4, 2]];
    }
    let reps: usize = std::env::var("REPS").ok().and_then(|s| s.parse().ok()).unwrap_or(3);
    println!(
        "{:>4} {:>2} {:>3} {:>3} | {:>6} {:>5} {:>9} | {:>9} {:>9} {:>9} {:>8} {:>7} | {:>7}",
        "n", "k", "S", "rpr", "width", "rows", "cells", "step", "prove", "verify", "proof", "KB", "prove/step"
    );
    for [n, k, strips, rpr] in layouts {
        let seed = umis_core::keccak256(b"bench-life");
        let mut o = LifeOptions::new(seed, 0, NodeId::default());
        (o.n, o.gens, o.strips, o.rows_per_trace_row, o.region) = (n, k, strips, rpr, [3, 3]);
        let p = o.zone_params([1, 1, 0], 4);
        if Life2d::validate_params(&p).is_err() {
            println!("{n},{k},{strips},{rpr}: invalid params");
            continue;
        }
        let params = borsh::to_vec(&p).unwrap();
        let inst = TaskAdapter::<Life2d>::new().instantiate(&params, &seed).unwrap();
        let prev = inst.initial_state(0);
        let inputs = BTreeMap::new(); // slot 0: the neighbours' genesis soups
        let (t_step, next) = best(reps.max(5), || inst.step(&prev, &inputs, 0).unwrap());
        let ctx = Life2d::prepare(&p, &seed).unwrap();
        assert!(matches!(decide(&ctx, &decode_state(&p, &prev).unwrap(), &inputs, 0), Decision::Advance { .. }));
        let outs = inst.outputs(&next).unwrap();
        let leaves: Vec<_> = outs.values().map(|b| bmt::file_address(b)).collect();
        let stmt = TransitionStatement {
            zone: 0,
            from_slot: 0,
            to_slot: 1,
            params_hash: umis_task::params_hash(task_life2d::TASK_ID, task_life2d::LAYER, &params),
            prev_state_root: bmt::file_address(&prev),
            state_root: bmt::file_address(&next),
            inputs_commitment: umis_core::records::list_root_of::<umis_core::records::InputRef>(&[]),
            outputs_root: bmt::list_root(&leaves),
        };
        let lh = std::env::var("LIFE_HASH").unwrap_or_default();
        let hash = if lh.starts_with("keccak") { MerkleHash::Keccak } else { MerkleHash::Poseidon2 };
        let mut b = Life2dStark::with_hash(FriPreset::Benchmark, hash);
        if lh == "keccak-gpu" {
            assert!(b.use_gpu(true), "GPU prover unavailable (build with --features cuda)");
        }
        let w = Witness {
            instance: inst.as_ref(),
            params: &params,
            genesis_seed: seed,
            prev_state: &prev,
            inputs: &inputs,
        };
        let key = SecretKey::from_label("x");
        let _ = b.prove(&stmt, &w, &key).unwrap(); // setup
        let (t_prove, proof) = best(reps, || b.prove(&stmt, &w, &key).unwrap());
        let pd = PublicData {
            task_id: Some(task_life2d::TASK_ID),
            params: Some(&params),
            genesis_seed: Some(seed),
            prev_state: Some(&prev),
            next_state: Some(&next),
            inputs: Some(&inputs),
            input_refs: Some(&[]),
        };
        let (t_verify, r) = best(reps, || b.verify(&stmt, &pd, &proof));
        r.unwrap();
        let s = Shape::of(&p);
        println!(
            "{n:>4} {k:>2} {strips:>3} {rpr:>3} | {:>6} {:>5} {:>9} | {:>9.1?} {:>9.1?} {:>9.1?} {:>8} {:>7} | {:>7.0}x",
            s.width(),
            s.height,
            trace_cells(&p),
            t_step,
            t_prove,
            t_verify,
            proof.len(),
            proof.len() / 1024,
            t_prove.as_secs_f64() / t_step.as_secs_f64()
        );
    }
}
