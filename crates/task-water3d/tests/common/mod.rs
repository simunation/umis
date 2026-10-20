//! Shared fixtures: a 2×2×1 genesis domain run through the physics reference.
// Each test binary uses a different subset.
#![allow(dead_code)]

use std::sync::Arc;
use task_water3d::{Water3d, Water3dParams};
use umis_core::Hash;
use umis_task::sim::{DomainSim, SimLink, SimZone};
use umis_task::topology::{Lattice3d, LayerId, Location, links_between};
use umis_task::{DynZoneTask, TaskAdapter};

pub const SLOTS: [u64; 4] = [4, 8, 4, 16];
pub const POINTS: [[i32; 3]; 4] = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0]];

pub fn genesis_seed() -> Hash {
    umis_core::keccak256(b"simunation golden seed")
}

pub fn domain(n_loc: u32) -> DomainSim {
    let task = TaskAdapter::<Water3d>::new();
    let topo = Lattice3d { bounds: [8, 8, 2] };
    let mut zones = Vec::new();
    for (id, (point, s)) in POINTS.iter().zip(SLOTS).enumerate() {
        let mut p = Water3dParams::default_for(*point, s as u32);
        p.n_loc = n_loc;
        let params = borsh::to_vec(&p).unwrap();
        let inst: Arc<dyn umis_task::ZoneInstance> = task.instantiate(&params, &genesis_seed()).unwrap();
        zones.push(SimZone { id: id as u32, slot_ticks: s, state: inst.initial_state(0), inst, next_slot: 0 });
    }
    let mut links = Vec::new();
    let loc = |p: [i32; 3]| Location { point: p, layer: LayerId::new("water") };
    for (a, pa) in POINTS.iter().enumerate() {
        for (b, pb) in POINTS.iter().enumerate() {
            for l in links_between(&topo, &[], &loc(*pa), &loc(*pb)) {
                links.push(SimLink {
                    from_zone: a as u32,
                    from_port: l.from_port,
                    to_zone: b as u32,
                    to_port: l.to_port,
                });
            }
        }
    }
    DomainSim::new(zones, links)
}

/// Proves and verifies one slot of a 4³ zone with the test FRI preset;
/// returns the proof's address (proofs are deterministic: no hiding).
#[cfg(feature = "stark")]
pub fn stark_roundtrip() -> Hash {
    stark_roundtrip_with(4, umis_stark::config::FriPreset::Test, &|| 0.0).address
}

#[cfg(feature = "stark")]
pub struct StarkRun {
    pub address: Hash,
    pub prove_ms: f64,
    pub verify_ms: f64,
    pub bytes: usize,
}

/// One slot of an `n_loc`³ zone: setup, prove (timed with `now_ms`), verify.
#[cfg(feature = "stark")]
pub fn stark_roundtrip_with(n_loc: u32, preset: umis_stark::config::FriPreset, now_ms: &dyn Fn() -> f64) -> StarkRun {
    use std::collections::BTreeMap;
    use task_water3d::air::Water3dStark;
    use umis_core::bmt;
    use umis_core::records::{InputRef, TransitionStatement, list_root_of};
    use umis_proof::{ProofBackend, PublicData, Witness};

    let mut p = Water3dParams::default_for([0, 0, 0], 4);
    p.n_loc = n_loc;
    let params = borsh::to_vec(&p).unwrap();
    let seed = genesis_seed();
    let inst = TaskAdapter::<Water3d>::new().instantiate(&params, &seed).unwrap();
    let prev = inst.initial_state(0);
    let mut face = vec![0u8; p.face_bytes()];
    face[12..16].copy_from_slice(&700i32.to_le_bytes());
    let inputs: BTreeMap<u32, Vec<u8>> = [(1u32, face.clone())].into_iter().collect();
    let refs = vec![InputRef {
        link_id: 1,
        src_zone: 1,
        src_slot: 0,
        src_block_hash: Hash::ZERO,
        src_outputs_root: Hash::ZERO,
        payload_address: bmt::file_address(&face),
    }];
    let slot = 2;
    let next = inst.step(&prev, &inputs, slot).unwrap();
    let outs = inst.outputs(&next).unwrap();
    let out_leaves: Vec<Hash> = outs.values().map(|o| bmt::file_address(o)).collect();
    let stmt = TransitionStatement {
        zone: 0,
        from_slot: slot,
        to_slot: slot + 1,
        params_hash: umis_task::params_hash(task_water3d::TASK_ID, task_water3d::LAYER, &params),
        prev_state_root: bmt::file_address(&prev),
        state_root: bmt::file_address(&next),
        inputs_commitment: list_root_of(&refs),
        outputs_root: bmt::list_root(&out_leaves),
    };
    let b = Water3dStark::new(preset);
    let w =
        Witness { instance: inst.as_ref(), params: &params, genesis_seed: seed, prev_state: &prev, inputs: &inputs };
    let key = umis_core::SecretKey::from_label("prover");
    // The first proof also builds the zone setup (preprocessed commitment).
    b.prove(&stmt, &w, &key).unwrap();
    let t = now_ms();
    let proof = b.prove(&stmt, &w, &key).unwrap();
    let prove_ms = now_ms() - t;
    let pd = PublicData {
        params: Some(&params),
        genesis_seed: Some(seed),
        prev_state: Some(&prev),
        next_state: Some(&next),
        inputs: Some(&inputs),
        input_refs: Some(&refs),
    };
    let t = now_ms();
    b.verify(&stmt, &pd, &proof).unwrap();
    let verify_ms = now_ms() - t;
    StarkRun { address: bmt::file_address(&proof), prove_ms, verify_ms, bytes: proof.len() }
}
