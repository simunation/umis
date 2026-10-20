//! The POC devnet genesis: 2×2×1 water zones with `S_z = 4, 8, 4, 16` ticks
//! on an 8×8×2 lattice, the `water` layer only, mock proofs.

use crate::Water3dParams;
use umis_core::genesis::{ChainParams, Genesis, GenesisValidator, GenesisZone, TopologySpec};
use umis_core::{Hash, NodeId};

pub const GENESIS_POINTS: [[i32; 3]; 4] = [[0, 0, 0], [1, 0, 0], [0, 1, 0], [1, 1, 0]];
pub const GENESIS_SLOT_TICKS: [u64; 4] = [4, 8, 4, 16];
pub const LATTICE_BOUNDS: [i32; 3] = [8, 8, 2];

pub struct DevnetOptions {
    pub seed: Hash,
    pub genesis_time_ms: u64,
    pub validators: Vec<GenesisValidator>,
    pub faucet: NodeId,
    pub params: ChainParams,
    /// Cells per zone axis (16 in the POC; smaller for fast tests).
    pub n_loc: u32,
    pub slot_ticks: Vec<u64>,
}

impl DevnetOptions {
    pub fn new(seed: Hash, genesis_time_ms: u64, faucet: NodeId) -> Self {
        Self {
            seed,
            genesis_time_ms,
            validators: Vec::new(),
            faucet,
            params: ChainParams::default(),
            n_loc: 16,
            slot_ticks: GENESIS_SLOT_TICKS.to_vec(),
        }
    }
}

/// Water3d params for a zone at `point` in the devnet lattice.
pub fn zone_params(point: [i32; 3], slot_ticks: u64, n_loc: u32) -> Water3dParams {
    let mut p = Water3dParams::default_for(point, slot_ticks as u32);
    p.domain = LATTICE_BOUNDS;
    p.n_loc = n_loc;
    p
}

pub fn devnet_genesis(o: DevnetOptions) -> Genesis {
    let zones = GENESIS_POINTS
        .iter()
        .zip(o.slot_ticks.iter())
        .enumerate()
        .map(|(id, (point, s))| GenesisZone {
            id: id as u32,
            point: *point,
            layer: crate::LAYER.into(),
            task_id: crate::TASK_ID.into(),
            params: serde_json::to_value(zone_params(*point, *s, o.n_loc)).expect("params json"),
            slot_ticks: *s,
        })
        .collect();
    Genesis {
        chain_id: "simunation-devnet".into(),
        genesis_time_ms: o.genesis_time_ms,
        seed: o.seed,
        params: o.params,
        topology: TopologySpec::Lattice3d { bounds: LATTICE_BOUNDS },
        layers: vec![crate::LAYER.into()],
        couplings: vec![],
        proof_backend: "mock".into(),
        stark_config_id: None,
        faucet: o.faucet,
        validators: o.validators,
        zones,
    }
}
