//! Genesis: chain parameters, topology, genesis zones and validators.
//!
//! The genesis file is JSON. Its hash is the bmt file address of its canonical
//! JSON encoding (struct field order, sorted map keys); `genesis8` is the first
//! 8 hex characters of that hash and prefixes topics and namespaces.

use crate::crypto::NodeId;
use crate::hash::{Hash, keccak256_concat};
use crate::time::{TimeGrid, ZoneTiming};
use serde::{Deserialize, Serialize};

/// Protocol parameters (plan, "Consensus parameters" and "Faults, rollback and stake").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainParams {
    /// `tick_ms`
    pub tick_ms: u64,
    /// `B`: beacon slot length in ticks.
    pub beacon_slot_ticks: u64,
    /// `D`: anchor depth in beacon slots.
    pub anchor_depth: u64,
    /// `K_f`: finality depth.
    pub finality_depth: u64,
    /// `R`: highest fallback minter rank.
    pub max_zone_rank: u32,
    /// `R_b`: highest fallback beacon-proposer rank.
    pub max_beacon_rank: u32,
    /// `L`: proof lag in beacon slots.
    pub proof_lag: u64,
    /// `δ`: clock tolerance in ms.
    pub clock_tolerance_ms: u64,
    /// `E_hb`: heartbeat timeout in beacon slots.
    pub heartbeat_timeout: u64,
    pub min_stake: u64,
    /// `slash_frac` in percent.
    pub slash_pct: u64,
    /// `reward_frac` in percent of the slashed amount.
    pub reward_pct: u64,
    /// `U`: unbonding delay in beacon slots.
    pub unbonding: u64,
    /// `V`: voting window in beacon slots.
    pub voting_window: u64,
    /// `q_vote` as a fraction.
    pub vote_num: u64,
    pub vote_den: u64,
    /// Spam limit on pending zone proposals.
    pub max_pending_proposals: u32,
    /// Largest zone state blob.
    pub max_state_bytes: u32,
}

impl Default for ChainParams {
    fn default() -> Self {
        Self {
            tick_ms: 250,
            beacon_slot_ticks: 16,
            anchor_depth: 2,
            finality_depth: 2,
            max_zone_rank: 2,
            max_beacon_rank: 2,
            proof_lag: 0,
            clock_tolerance_ms: 500,
            heartbeat_timeout: 10,
            min_stake: 100,
            slash_pct: 100,
            reward_pct: 10,
            unbonding: 20,
            voting_window: 10,
            vote_num: 2,
            vote_den: 3,
            max_pending_proposals: 4,
            max_state_bytes: 64 * 1024,
        }
    }
}

/// Where zones can live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TopologySpec {
    /// Integer lattice points `0 ≤ p < bounds` per axis, face adjacency.
    Lattice3d { bounds: [i32; 3] },
}

/// Connects ports of two different layers co-located at one point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouplingSpec {
    pub layer_a: String,
    pub port_a: u32,
    pub layer_b: String,
    pub port_b: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorKind {
    /// Member of one zone.
    Member,
    /// Native full node: follows every zone, fallback minter.
    FullNode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenesisValidator {
    pub node: NodeId,
    pub stake: u64,
    pub kind: ValidatorKind,
    /// Zone for members; ignored for full nodes.
    pub zone: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenesisZone {
    pub id: u32,
    pub point: [i32; 3],
    pub layer: String,
    pub task_id: String,
    /// Task-specific parameters; the task turns them into its borsh `Params`.
    pub params: serde_json::Value,
    /// `S_z` in ticks.
    pub slot_ticks: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Genesis {
    pub chain_id: String,
    /// `G`: Unix ms at which tick 0 starts.
    pub genesis_time_ms: u64,
    /// `genesis.seed`: source of all random genesis parameters and the first
    /// beacon seed.
    pub seed: Hash,
    pub params: ChainParams,
    pub topology: TopologySpec,
    pub layers: Vec<String>,
    pub couplings: Vec<CouplingSpec>,
    /// Proof backend id (`mock` or `plonky3`).
    pub proof_backend: String,
    /// Pinned STARK configuration when `proof_backend = plonky3`.
    pub stark_config_id: Option<String>,
    /// Devnet faucet key that may sign `Grant` txs.
    pub faucet: NodeId,
    pub validators: Vec<GenesisValidator>,
    pub zones: Vec<GenesisZone>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenesisError {
    #[error("invalid json: {0}")]
    Json(String),
    #[error("{0}")]
    Invalid(String),
}

impl Genesis {
    pub fn from_json(s: &str) -> Result<Self, GenesisError> {
        let g: Genesis = serde_json::from_str(s).map_err(|e| GenesisError::Json(e.to_string()))?;
        g.validate()?;
        Ok(g)
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("genesis serializes")
    }

    /// Canonical encoding: compact JSON in struct field order with sorted map keys.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("genesis serializes")
    }

    pub fn hash(&self) -> Hash {
        crate::bmt::file_address(&self.canonical_bytes())
    }

    /// First 8 hex characters of the genesis hash.
    pub fn genesis8(&self) -> String {
        self.hash().to_hex()[..8].to_string()
    }

    pub fn time_grid(&self) -> TimeGrid {
        TimeGrid {
            genesis_ms: self.genesis_time_ms,
            tick_ms: self.params.tick_ms,
            beacon_slot_ticks: self.params.beacon_slot_ticks,
        }
    }

    /// The first beacon seed: `H(genesis.seed ‖ "beacon")`.
    pub fn initial_beacon_seed(&self) -> Hash {
        keccak256_concat(&[&self.seed.0, b"beacon"])
    }

    /// Per-purpose seed derived from `genesis.seed`, e.g. `H(seed ‖ "water3d")`.
    pub fn derived_seed(&self, purpose: &str) -> Hash {
        keccak256_concat(&[&self.seed.0, purpose.as_bytes()])
    }

    pub fn validate(&self) -> Result<(), GenesisError> {
        let p = &self.params;
        let bad = |s: String| Err(GenesisError::Invalid(s));
        if p.tick_ms == 0 || p.beacon_slot_ticks == 0 {
            return bad("tick_ms and beacon_slot_ticks must be positive".into());
        }
        if p.beacon_slot_ticks % 4 != 0 {
            return bad("beacon_slot_ticks must be a multiple of 4 (quarter-slot pause)".into());
        }
        if p.anchor_depth < p.finality_depth {
            return bad("anchor depth D must be >= finality depth K_f".into());
        }
        if p.vote_den == 0 || p.vote_num > p.vote_den {
            return bad("vote threshold must be a fraction <= 1".into());
        }
        if p.unbonding <= p.proof_lag + p.anchor_depth + 1 {
            return bad("unbonding U must exceed the rollback window L + D + 1".into());
        }
        let TopologySpec::Lattice3d { bounds } = &self.topology;
        let mut seen_ids = std::collections::BTreeSet::new();
        let mut seen_loc = std::collections::BTreeSet::new();
        for z in &self.zones {
            if !seen_ids.insert(z.id) {
                return bad(format!("duplicate zone id {}", z.id));
            }
            if !seen_loc.insert((z.point, z.layer.clone())) {
                return bad(format!("two zones at location {:?}/{}", z.point, z.layer));
            }
            if !self.layers.contains(&z.layer) {
                return bad(format!("zone {} uses unknown layer {}", z.id, z.layer));
            }
            if (0..3).any(|i| z.point[i] < 0 || z.point[i] >= bounds[i]) {
                return bad(format!("zone {} point {:?} outside bounds {:?}", z.id, z.point, bounds));
            }
            if let Err(e) = ZoneTiming::new(z.slot_ticks, p.beacon_slot_ticks) {
                return bad(format!("zone {}: {e}", z.id));
            }
        }
        for (i, z) in self.zones.iter().enumerate() {
            if z.id != i as u32 {
                return bad("genesis zone ids must be 0, 1, 2, ... in order".into());
            }
        }
        for v in &self.validators {
            if v.stake < p.min_stake {
                return bad(format!("validator {} stake below min_stake", v.node.short()));
            }
            if v.kind == ValidatorKind::Member && v.zone.is_none_or(|z| z as usize >= self.zones.len()) {
                return bad(format!("validator {} has no valid zone", v.node.short()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn sample() -> Genesis {
        Genesis {
            chain_id: "simunation-test".into(),
            genesis_time_ms: 1_700_000_000_000,
            seed: crate::hash::keccak256(b"seed"),
            params: ChainParams::default(),
            topology: TopologySpec::Lattice3d { bounds: [8, 8, 2] },
            layers: vec!["water".into()],
            couplings: vec![],
            proof_backend: "mock".into(),
            stark_config_id: None,
            faucet: crate::crypto::SecretKey::from_label("faucet").node_id(),
            validators: vec![],
            zones: [(0, [0, 0, 0], 4), (1, [1, 0, 0], 8), (2, [0, 1, 0], 4), (3, [1, 1, 0], 16)]
                .into_iter()
                .map(|(id, point, s)| GenesisZone {
                    id,
                    point,
                    layer: "water".into(),
                    task_id: "water3d@1".into(),
                    params: serde_json::json!({"n": 16}),
                    slot_ticks: s,
                })
                .collect(),
        }
    }

    #[test]
    fn json_round_trip_and_hash() {
        let g = sample();
        let back = Genesis::from_json(&g.to_json_pretty()).unwrap();
        assert_eq!(back, g);
        assert_eq!(back.hash(), g.hash());
        assert_eq!(g.genesis8().len(), 8);
        let mut g2 = g.clone();
        g2.seed = crate::hash::keccak256(b"other");
        assert_ne!(g2.hash(), g.hash());
    }

    #[test]
    fn validation() {
        let mut g = sample();
        g.zones[1].slot_ticks = 6;
        assert!(g.validate().is_err());
        let mut g = sample();
        g.zones[1].point = [0, 0, 0];
        assert!(g.validate().is_err());
        let mut g = sample();
        g.params.anchor_depth = 1;
        assert!(g.validate().is_err());
        assert!(sample().validate().is_ok());
    }
}
