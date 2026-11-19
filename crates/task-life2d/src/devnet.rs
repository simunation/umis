//! A Game of Life devnet genesis: `rx × ry` genesis zones (seeded soups) on an
//! 8×8×1 lattice whose `life` layer links in-plane diagonals.

use crate::Life2dParams;
use umis_core::genesis::{ChainParams, Genesis, GenesisValidator, GenesisZone, TopologySpec};
use umis_core::{Hash, NodeId};

pub const LATTICE_BOUNDS: [i32; 3] = [8, 8, 1];

#[derive(Debug, Clone)]
pub struct LifeOptions {
    pub seed: Hash,
    pub genesis_time_ms: u64,
    pub validators: Vec<GenesisValidator>,
    pub faucet: NodeId,
    pub params: ChainParams,
    /// Cells per zone side.
    pub n: u32,
    /// Generations per zone slot.
    pub gens: u32,
    /// Genesis zones per axis.
    pub region: [i32; 2],
    /// `S_z` of the genesis zones in id order (ticks), cycled.
    pub slot_ticks: Vec<u64>,
    pub density_pct: u32,
    pub strips: u32,
    pub rows_per_trace_row: u32,
    /// Genesis boards (`Life2dParams::pattern`).
    pub pattern: u32,
}

impl LifeOptions {
    pub fn new(seed: Hash, genesis_time_ms: u64, faucet: NodeId) -> Self {
        let d = Life2dParams::default_for([0, 0, 0], 4);
        Self {
            seed,
            genesis_time_ms,
            validators: Vec::new(),
            faucet,
            params: ChainParams::default(),
            n: d.n,
            gens: d.gens,
            region: d.genesis_region,
            slot_ticks: vec![4],
            density_pct: d.density_pct,
            strips: d.strips,
            rows_per_trace_row: d.rows_per_trace_row,
            pattern: 0,
        }
    }

    /// Params of the zone at `point` (genesis or created).
    pub fn zone_params(&self, point: [i32; 3], slot_ticks: u64) -> Life2dParams {
        Life2dParams {
            point,
            n: self.n,
            gens: self.gens,
            slot_ticks: slot_ticks as u32,
            genesis_region: self.region,
            density_pct: self.density_pct,
            strips: self.strips,
            rows_per_trace_row: self.rows_per_trace_row,
            pattern: self.pattern,
        }
    }

    /// Genesis zone points in id order (row-major).
    pub fn genesis_points(&self) -> Vec<[i32; 3]> {
        let mut v = Vec::new();
        for y in 0..self.region[1] {
            for x in 0..self.region[0] {
                v.push([x, y, 0]);
            }
        }
        v
    }
}

/// Params for a zone created at `point`, matching a genesis's life zones.
pub fn zone_params_like(g: &Genesis, point: [i32; 3], slot_ticks: u64) -> Option<Life2dParams> {
    let z = g.zones.iter().find(|z| z.task_id == crate::TASK_ID)?;
    let mut p: Life2dParams = serde_json::from_value(z.params.clone()).ok()?;
    p.point = point;
    p.slot_ticks = slot_ticks as u32;
    Some(p)
}

pub fn devnet_genesis(o: LifeOptions) -> Genesis {
    let zones = o
        .genesis_points()
        .into_iter()
        .enumerate()
        .map(|(id, point)| {
            let s = o.slot_ticks[id % o.slot_ticks.len()];
            GenesisZone {
                id: id as u32,
                point,
                layer: crate::LAYER.into(),
                task_id: crate::TASK_ID.into(),
                params: serde_json::to_value(o.zone_params(point, s)).expect("params json"),
                slot_ticks: s,
            }
        })
        .collect();
    Genesis {
        chain_id: "simunation-life".into(),
        genesis_time_ms: o.genesis_time_ms,
        seed: o.seed,
        params: o.params,
        topology: TopologySpec::Lattice3d { bounds: LATTICE_BOUNDS, diagonal_layers: vec![crate::LAYER.into()] },
        layers: vec![crate::LAYER.into()],
        couplings: vec![],
        proof_backend: "mock".into(),
        stark_config_id: None,
        insecure_proofs: false,
        faucet: o.faucet,
        validators: o.validators,
        zones,
    }
}
