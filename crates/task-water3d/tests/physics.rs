//! Physics sanity and determinism of water3d (native).
#![cfg(not(target_arch = "wasm32"))]

mod common;

use task_water3d::{CELL_BYTES, Q_BOUND};

fn values(state: &[u8]) -> impl Iterator<Item = i32> + '_ {
    state.chunks_exact(4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}

#[test]
fn golden_trace_hash() {
    let mut sim = common::domain(8);
    sim.run_until(64).unwrap();
    assert_eq!(sim.trace_hash().to_hex(), GOLDEN_TRACE_8, "native trace changed");
}

/// Shared with tests/wasm.rs: native and wasm must produce this trace.
const GOLDEN_TRACE_8: &str = include_str!("golden-trace-8.txt");

#[test]
fn bounded_and_alive_over_many_slots() {
    let mut sim = common::domain(16);
    sim.run_until(16 * 40).unwrap();
    let mut max_dye = 0;
    for z in sim.zones.values() {
        assert_eq!(z.state.len(), 16 * 16 * 16 * CELL_BYTES);
        for (i, v) in values(&z.state).enumerate() {
            assert!((v as i64).abs() < Q_BOUND, "zone {} value {v} out of bounds", z.id);
            if i % 4 == 3 {
                assert!(v >= 0, "dye must stay non-negative");
                max_dye = max_dye.max(v);
            }
        }
    }
    assert!(max_dye > 0, "dye sources must inject dye");
}

#[test]
fn run_is_deterministic() {
    let mut a = common::domain(8);
    let mut b = common::domain(8);
    a.run_until(48).unwrap();
    b.run_until(48).unwrap();
    assert_eq!(a.trace_hash(), b.trace_hash());
    for (za, zb) in a.zones.values().zip(b.zones.values()) {
        assert_eq!(za.state, zb.state);
    }
}

#[test]
fn neighbours_exchange_through_faces() {
    // Zero the field everywhere except a dye blob in zone 0; it must reach zone 1.
    use std::collections::BTreeMap;
    use task_water3d::{Water3d, Water3dParams};
    use umis_task::{DynZoneTask, TaskAdapter};
    let task = TaskAdapter::<Water3d>::new();
    let mut p = Water3dParams::default_for([0, 0, 0], 4);
    p.n_loc = 4;
    p.vortices = 0;
    p.dye_sources = 0;
    let inst0 = task.instantiate(&borsh::to_vec(&p).unwrap(), &common::genesis_seed()).unwrap();
    let mut state = inst0.initial_state(0);
    // dye at the +x face of zone 0
    for y in 0..4 {
        for z in 0..4 {
            let i = 3 + 4 * (y + 4 * z);
            state[i * CELL_BYTES + 12..i * CELL_BYTES + 16].copy_from_slice(&5000i32.to_le_bytes());
        }
    }
    let out = inst0.outputs(&state).unwrap();
    let mut p1 = p.clone();
    p1.point = [1, 0, 0];
    let inst1 = task.instantiate(&borsh::to_vec(&p1).unwrap(), &common::genesis_seed()).unwrap();
    let mut inputs = BTreeMap::new();
    inputs.insert(1u32, out[&0].clone()); // zone 1's −x halo = zone 0's +x face
    let next = inst1.step(&inst1.initial_state(0), &inputs, 0).unwrap();
    let dye_at_minus_x: i32 = (0..16)
        .map(|t| {
            let i = 4 * t; // x = 0 cells
            i32::from_le_bytes(next[i * CELL_BYTES + 12..i * CELL_BYTES + 16].try_into().unwrap())
        })
        .sum();
    assert!(dye_at_minus_x > 0, "dye diffuses across the shared face");
}
