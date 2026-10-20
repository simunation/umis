//! A test-only task on a second layer, `probe`, coupled to water's top face.
//! It counts steps, folds what it receives into a checksum, and returns a
//! water-shaped face whose dye value is its step count, so a coupled water
//! zone visibly receives its data.

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use umis_core::Hash;
use umis_task::{Bytes, PortDir, PortId, PortSpec, TaskError, ZoneTask};

pub const PROBE_TASK: &str = "probe@1";
pub const PROBE_LAYER: &str = "probe";
/// Water's top face port (+z) couples to the probe's port 0.
pub const WATER_TOP: u32 = 4;
pub const PROBE_PORT: u32 = 0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ProbeParams {
    pub point: [i32; 3],
    pub n_loc: u32,
    pub slot_ticks: u32,
}

pub struct Probe;

pub struct ProbeState {
    pub count: u64,
    pub checksum: u64,
}

fn face_bytes(n: u32) -> usize {
    (n * n) as usize * 16
}

impl ZoneTask for Probe {
    const TASK_ID: &'static str = PROBE_TASK;
    const LAYER: &'static str = PROBE_LAYER;
    type Params = ProbeParams;
    type Ctx = ProbeParams;
    type State = ProbeState;
    type Inputs = Bytes;

    fn ports(p: &ProbeParams) -> Vec<PortSpec> {
        vec![PortSpec {
            id: PROBE_PORT,
            name: "bottom".into(),
            dir: PortDir::InOut,
            schema: format!("water3d.face{}@1", p.n_loc),
            payload_bytes: face_bytes(p.n_loc) as u32,
        }]
    }

    fn validate_params(p: &ProbeParams) -> umis_task::Result<()> {
        if p.n_loc == 0 || p.slot_ticks == 0 {
            return Err(TaskError::Params("bad probe params".into()));
        }
        Ok(())
    }

    fn check_zone(p: &ProbeParams, point: [i32; 3], slot_ticks: u64) -> umis_task::Result<()> {
        if p.point != point || p.slot_ticks as u64 != slot_ticks {
            return Err(TaskError::Params("probe params do not match the proposal".into()));
        }
        Ok(())
    }

    fn prepare(p: &ProbeParams, _seed: &Hash) -> umis_task::Result<ProbeParams> {
        Ok(p.clone())
    }

    fn initial_state(_c: &ProbeParams, start_slot: u64) -> ProbeState {
        ProbeState { count: start_slot, checksum: 0 }
    }

    fn empty_neighbour_input(c: &ProbeParams, _port: PortId) -> Bytes {
        vec![0; face_bytes(c.n_loc)]
    }

    fn decode_inputs(c: &ProbeParams, ports: &BTreeMap<PortId, Bytes>) -> umis_task::Result<Bytes> {
        Ok(ports.get(&PROBE_PORT).cloned().unwrap_or_else(|| vec![0; face_bytes(c.n_loc)]))
    }

    fn step(_c: &ProbeParams, s: &ProbeState, i: &Bytes, _slot: u64) -> ProbeState {
        let sum = i.iter().fold(s.checksum, |a, b| a.wrapping_mul(31).wrapping_add(*b as u64));
        ProbeState { count: s.count + 1, checksum: sum }
    }

    fn outputs(c: &ProbeParams, s: &ProbeState) -> BTreeMap<PortId, Bytes> {
        let mut face = Vec::with_capacity(face_bytes(c.n_loc));
        for _ in 0..c.n_loc * c.n_loc {
            for v in [0i32, 0, 0, (s.count % 100) as i32 * 10] {
                face.extend_from_slice(&v.to_le_bytes());
            }
        }
        [(PROBE_PORT, face)].into_iter().collect()
    }

    fn encode_state(_c: &ProbeParams, s: &ProbeState) -> Bytes {
        let mut b = s.count.to_le_bytes().to_vec();
        b.extend_from_slice(&s.checksum.to_le_bytes());
        b
    }

    fn decode_state(_c: &ProbeParams, b: &[u8]) -> umis_task::Result<ProbeState> {
        if b.len() != 16 {
            return Err(TaskError::State("probe state is 16 bytes".into()));
        }
        Ok(ProbeState {
            count: u64::from_le_bytes(b[..8].try_into().expect("8")),
            checksum: u64::from_le_bytes(b[8..].try_into().expect("8")),
        })
    }
}
