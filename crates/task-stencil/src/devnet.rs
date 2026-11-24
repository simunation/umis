//! A stencil devnet genesis: `rx × ry` genesis zones of one rule on an 8×8×1 lattice whose
//! `stencil` layer links in-plane diagonals.

use crate::{RuleId, StencilParams};
use umis_core::genesis::{ChainParams, Genesis, GenesisValidator, GenesisZone, TopologySpec};
use umis_core::{Hash, NodeId};

pub const LATTICE_BOUNDS: [i32; 3] = [8, 8, 1];

#[derive(Debug, Clone)]
pub struct StencilOptions {
    pub seed: Hash,
    pub genesis_time_ms: u64,
    pub validators: Vec<GenesisValidator>,
    pub faucet: NodeId,
    pub params: ChainParams,
    pub rule: RuleId,
    pub n: u32,
    pub gens: u32,
    pub region: [i32; 2],
    /// `S_z` of the genesis zones in id order (ticks), cycled.
    pub slot_ticks: Vec<u64>,
    pub rows_per_trace_row: u32,
    pub pattern: u32,
}

/// The default parameters of each rule: fast enough to prove every slot on a CPU.
pub fn default_rule(name: &str) -> Option<RuleId> {
    Some(match name {
        "life" => RuleId::Life,
        "heat" => RuleId::Heat { a: 3, s: 4, wall: 1000 },
        // ω = 1.875: viscosity (1/ω − 1/2)/3 ≈ 0.011, so shear layers roll up into vortices
        "lbm" => RuleId::Lbm { a: 30, t: 4 },
        // Du 0.16, Dv 0.08, F 0.037, k 0.06: catalyst spots that grow and split
        "grayscott" | "gs" => RuleId::GrayScott { du: 655, dv: 328, f: 152, k: 246, s: 12 },
        _ => return None,
    })
}

impl StencilOptions {
    pub fn new(seed: Hash, genesis_time_ms: u64, faucet: NodeId, rule: RuleId) -> Self {
        Self {
            seed,
            genesis_time_ms,
            validators: Vec::new(),
            faucet,
            params: ChainParams::default(),
            rule,
            n: if matches!(rule, RuleId::Lbm { .. }) { 32 } else { 64 },
            gens: 1,
            region: [2, 2],
            slot_ticks: vec![4],
            rows_per_trace_row: 2,
            pattern: 0,
        }
    }

    pub fn zone_params(&self, point: [i32; 3], slot_ticks: u64) -> StencilParams {
        StencilParams {
            point,
            rule: self.rule,
            n: self.n,
            gens: self.gens,
            slot_ticks: slot_ticks as u32,
            genesis_region: self.region,
            rows_per_trace_row: self.rows_per_trace_row,
            pattern: self.pattern,
        }
    }

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

/// Params for a zone created at `point`, matching a genesis's stencil zones.
pub fn zone_params_like(g: &Genesis, point: [i32; 3], slot_ticks: u64) -> Option<StencilParams> {
    let z = g.zones.iter().find(|z| z.task_id == crate::TASK_ID)?;
    let mut p: StencilParams = serde_json::from_value(z.params.clone()).ok()?;
    p.point = point;
    p.slot_ticks = slot_ticks as u32;
    Some(p)
}

pub fn devnet_genesis(o: StencilOptions) -> Genesis {
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
        chain_id: format!("simunation-stencil-{}", o.rule.name()),
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
