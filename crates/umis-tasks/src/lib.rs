//! The zone tasks this build ships and the proof backend a genesis selects.
//!
//! - `mock`: signature-only proofs (any task);
//! - `plonky3`: one STARK prover per task ([`umis_proof::MultiBackend`]),
//!   water3d and life2d, both with the genesis `stark_config_id`
//!   (`koalabear-keccak*`: life2d with Keccak Merkle trees, water3d unchanged;
//!   `koalabear-agg*`: life2d commitments aggregated recursively, native only, water3d on
//!   `koalabear*`).

use std::sync::Arc;
use umis_core::genesis::Genesis;
use umis_proof::{MultiBackend, ProofBackend};

pub use task_life2d;
pub use task_water3d;

/// Registers every task of this build.
pub fn register_all(r: &mut umis_task::TaskRegistry) {
    task_water3d::register(r);
    task_life2d::register(r);
}

/// A registry with every task of this build.
pub fn registry() -> umis_task::TaskRegistry {
    let mut r = umis_task::TaskRegistry::new();
    register_all(&mut r);
    r
}

/// The proof backend a genesis selects.
pub fn backend_for(g: &Genesis) -> Result<Arc<dyn ProofBackend>, String> {
    match g.proof_backend.as_str() {
        umis_proof::MOCK_BACKEND => Ok(Arc::new(umis_proof::MockBackend)),
        task_water3d::air::BACKEND_ID => {
            let id = g.stark_config_id.as_deref();
            // `koalabear-keccak[-test]` switches life2d to Keccak Merkle trees; water3d
            // keeps its Poseidon2 config with the same FRI preset.
            let water_id = id.map(|i| {
                if i.starts_with("koalabear-keccak") || i.starts_with("koalabear-agg") {
                    i.replacen("-keccak", "", 1).replacen("-agg", "", 1)
                } else {
                    i.to_string()
                }
            });
            let water = task_water3d::air::Water3dStark::from_config_id(water_id.as_deref())?;
            let life: Arc<dyn ProofBackend> = match id {
                Some(agg) if agg.starts_with("koalabear-agg") => life_agg(agg)?,
                _ => Arc::new(task_life2d::air::Life2dStark::from_config_id(id)?),
            };
            Ok(Arc::new(
                MultiBackend::new(task_water3d::air::BACKEND_ID)
                    .with(task_water3d::TASK_ID, Arc::new(water))
                    .with(task_life2d::TASK_ID, life),
            ))
        }
        other => Err(format!("unknown proof backend {other}")),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn life_agg(id: &str) -> Result<Arc<dyn ProofBackend>, String> {
    Ok(Arc::new(umis_agg::backend::LifeAggBackend::from_config_id(id)?))
}

#[cfg(target_arch = "wasm32")]
fn life_agg(id: &str) -> Result<Arc<dyn ProofBackend>, String> {
    Err(format!("stark_config_id {id}: recursive aggregation is native only"))
}

/// Task id and borsh params of a new zone proposed on `layer` at `point`,
/// shaped like the genesis zones of that layer.
pub fn zone_proposal(
    g: &Genesis,
    layer: &str,
    point: [i32; 3],
    slot_ticks: u64,
) -> Result<(&'static str, Vec<u8>), String> {
    let enc = |r: std::io::Result<Vec<u8>>| r.map_err(|e| e.to_string());
    match layer {
        l if l == task_water3d::LAYER => {
            let n_loc = g
                .zones
                .iter()
                .find(|z| z.task_id == task_water3d::TASK_ID)
                .and_then(|z| z.params["n_loc"].as_u64())
                .unwrap_or(16) as u32;
            let p = task_water3d::devnet::zone_params(point, slot_ticks, n_loc);
            Ok((task_water3d::TASK_ID, enc(borsh::to_vec(&p))?))
        }
        l if l == task_life2d::LAYER => {
            let p = task_life2d::devnet::zone_params_like(g, point, slot_ticks)
                .ok_or("the genesis has no life zone to copy params from")?;
            Ok((task_life2d::TASK_ID, enc(borsh::to_vec(&p))?))
        }
        other => Err(format!("this build cannot create zones on layer {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plonky3_covers_every_registered_task() {
        let mut g = task_life2d::devnet::devnet_genesis(task_life2d::devnet::LifeOptions::new(
            umis_core::Hash::ZERO,
            0,
            umis_core::NodeId::default(),
        ));
        assert_eq!(backend_for(&g).unwrap().id(), umis_proof::MOCK_BACKEND);
        g.proof_backend = "plonky3".into();
        g.stark_config_id = Some("koalabear-test".into());
        g.insecure_proofs = true;
        assert!(backend_for(&g).unwrap().is_async());
        for id in ["koalabear", "koalabear-keccak", "koalabear-keccak-test"] {
            g.stark_config_id = Some(id.into());
            assert!(backend_for(&g).is_ok(), "{id}");
        }
        g.stark_config_id = Some("nope".into());
        assert!(backend_for(&g).is_err());
        g.proof_backend = "nope".into();
        assert!(backend_for(&g).is_err());
    }
}
