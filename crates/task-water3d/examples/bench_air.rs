//! `cargo run --release -p task-water3d --features stark --example bench_air -- [n_loc...]`
//! Prove / verify time and proof size of one water3d slot (benchmark FRI preset).
use std::collections::BTreeMap;
use task_water3d::air::Water3dStark;
use task_water3d::{Water3d, Water3dParams};
use umis_core::records::TransitionStatement;
use umis_core::{SecretKey, bmt};
use umis_proof::{ProofBackend, PublicData, Witness};
use umis_stark::config::FriPreset;
use umis_task::{DynZoneTask, TaskAdapter};

fn main() {
    let sizes: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    for n in if sizes.is_empty() { vec![8, 16] } else { sizes } {
        let mut p = Water3dParams::default_for([0, 0, 0], 4);
        p.n_loc = n;
        let params = borsh::to_vec(&p).unwrap();
        let seed = umis_core::keccak256(b"bench");
        let inst = TaskAdapter::<Water3d>::new().instantiate(&params, &seed).unwrap();
        let prev = inst.initial_state(0);
        let inputs = BTreeMap::new();
        let next = inst.step(&prev, &inputs, 0).unwrap();
        let outs = inst.outputs(&next).unwrap();
        let leaves: Vec<_> = outs.values().map(|b| bmt::file_address(b)).collect();
        let stmt = TransitionStatement {
            zone: 0,
            from_slot: 0,
            to_slot: 1,
            params_hash: umis_task::params_hash(task_water3d::TASK_ID, task_water3d::LAYER, &params),
            prev_state_root: bmt::file_address(&prev),
            state_root: bmt::file_address(&next),
            inputs_commitment: umis_core::records::list_root_of::<umis_core::records::InputRef>(&[]),
            outputs_root: bmt::list_root(&leaves),
        };
        let b = Water3dStark::new(FriPreset::Benchmark);
        let w = Witness {
            instance: inst.as_ref(),
            params: &params,
            genesis_seed: seed,
            prev_state: &prev,
            inputs: &inputs,
        };
        let t = std::time::Instant::now();
        let _ = b.prove(&stmt, &w, &SecretKey::from_label("x")).unwrap(); // includes setup
        let setup_and_prove = t.elapsed();
        let t = std::time::Instant::now();
        let proof = b.prove(&stmt, &w, &SecretKey::from_label("x")).unwrap();
        let prove = t.elapsed();
        let pd = PublicData {
            params: Some(&params),
            genesis_seed: Some(seed),
            prev_state: Some(&prev),
            next_state: Some(&next),
            inputs: Some(&inputs),
            input_refs: Some(&[]),
        };
        let t = std::time::Instant::now();
        b.verify(&stmt, &pd, &proof).unwrap();
        let verify = t.elapsed();
        println!(
            "n_loc {n:2}: rows {:6}  first (setup+prove) {setup_and_prove:>9.1?}  prove {prove:>9.1?}  verify {verify:>8.1?}  proof {} KB",
            (p.k_sub as usize * (n as usize).pow(3)).max(4096),
            proof.len() / 1024
        );
    }
}
