//! Leader schedules, all derived from an anchor beacon state.
//!
//! - zone minters for slot `j`: zone members sorted by
//!   `H(seed ‖ "zone" ‖ z ‖ j ‖ node_id)`, then native full nodes (same key) as
//!   a fallback tail; ranks `0..=R` may mint;
//! - beacon proposers for slot `e`: all registered nodes sorted by
//!   `H(seed ‖ "beacon" ‖ e ‖ node_id)`;
//! - priority prover of slot `j`: lowest `H(seed ‖ "prove" ‖ z ‖ j ‖ node_id)`
//!   among the zone's provers, excluding the slot's minter when possible.
//!
//! Lists are memoized (per thread) by seed, slot and a fingerprint of the
//! node set; nodes and SimNet instances compute the same lists many times.

use crate::beacon::state::BeaconState;
use crate::types::ZoneId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use umis_core::{Hash, NodeId, keccak256_concat};

type Key = (Hash, u8, Option<ZoneId>, u64, [u8; 32], usize);

thread_local! {
    static CACHE: RefCell<HashMap<Key, Arc<Vec<NodeId>>>> = RefCell::new(HashMap::new());
}

fn fingerprint(nodes: &[NodeId]) -> [u8; 32] {
    let mut f = [0u8; 32];
    for n in nodes {
        for i in 0..32 {
            f[i] ^= n.0[i].rotate_left((i % 7) as u32);
        }
    }
    f
}

fn key(seed: &Hash, tag: &[u8], z: Option<ZoneId>, slot: u64, node: &NodeId) -> Hash {
    let zb = z.map(|z| z.to_be_bytes()).unwrap_or_default();
    keccak256_concat(&[&seed.0, tag, &zb, &slot.to_be_bytes(), &node.0])
}

fn sorted(seed: &Hash, tag: &[u8], z: Option<ZoneId>, slot: u64, nodes: &[NodeId]) -> Vec<NodeId> {
    let mut v: Vec<(Hash, NodeId)> = nodes.iter().map(|n| (key(seed, tag, z, slot, n), *n)).collect();
    v.sort();
    v.into_iter().map(|(_, n)| n).collect()
}

fn memo(
    tag: u8,
    a: &BeaconState,
    z: Option<ZoneId>,
    slot: u64,
    nodes: Vec<NodeId>,
    f: impl FnOnce(&[NodeId]) -> Vec<NodeId>,
) -> Arc<Vec<NodeId>> {
    let k: Key = (a.seed, tag, z, slot, fingerprint(&nodes), nodes.len());
    if let Some(v) = CACHE.with(|c| c.borrow().get(&k).cloned()) {
        return v;
    }
    let v = Arc::new(f(&nodes));
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 200_000 {
            c.clear();
        }
        c.insert(k, v.clone());
    });
    v
}

/// Minter rank list of zone `z` in slot `j` under anchor state `a`.
pub fn zone_ranks(a: &BeaconState, z: ZoneId, j: u64) -> Arc<Vec<NodeId>> {
    let members = a.members_of(z);
    let full = a.full_nodes();
    let mut all = members.clone();
    all.push(NodeId([0xff; 32])); // separator: members and full nodes are distinct sets
    all.extend(full.iter().copied());
    memo(0, a, Some(z), j, all, |_| {
        let mut list = sorted(&a.seed, b"zone", Some(z), j, &members);
        list.extend(sorted(&a.seed, b"zone", Some(z), j, &full));
        list
    })
}

pub fn rank_of(list: &[NodeId], n: &NodeId) -> Option<u32> {
    list.iter().position(|x| x == n).map(|p| p as u32)
}

/// Beacon proposer rank list for slot `e` under anchor state `a`.
pub fn beacon_ranks(a: &BeaconState, e: u64) -> Arc<Vec<NodeId>> {
    memo(1, a, None, e, a.active_nodes(), |nodes| sorted(&a.seed, b"beacon", None, e, nodes))
}

/// The priority prover of zone slot `j`.
pub fn priority_prover(a: &BeaconState, z: ZoneId, j: u64, minter: &NodeId) -> Option<NodeId> {
    let mut provers: Vec<NodeId> = a.members_of(z);
    provers.extend(a.full_nodes());
    let list = memo(2, a, Some(z), j, provers, |p| sorted(&a.seed, b"prove", Some(z), j, p));
    list.iter().find(|n| *n != minter).or(list.first()).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ctx;
    use std::sync::Arc;

    fn ctx() -> Ctx {
        let mut reg = umis_task::TaskRegistry::new();
        task_water3d::register(&mut reg);
        let mut o = task_water3d::devnet::DevnetOptions::new(
            umis_core::keccak256(b"r"),
            0,
            umis_core::SecretKey::from_label("f").node_id(),
        );
        o.n_loc = 4;
        for i in 0..6u32 {
            o.validators.push(umis_core::genesis::GenesisValidator {
                node: umis_core::SecretKey::from_label(&format!("m{i}")).node_id(),
                stake: 500,
                kind: umis_core::genesis::ValidatorKind::Member,
                zone: Some(i % 2),
            });
        }
        o.validators.push(umis_core::genesis::GenesisValidator {
            node: umis_core::SecretKey::from_label("full").node_id(),
            stake: 500,
            kind: umis_core::genesis::ValidatorKind::FullNode,
            zone: None,
        });
        Ctx::new(task_water3d::devnet::devnet_genesis(o), reg, Arc::new(umis_proof::MockBackend))
    }

    #[test]
    fn ranks_are_deterministic_and_members_first() {
        let c = ctx();
        let s = BeaconState::genesis(&c).unwrap();
        let a = zone_ranks(&s, 0, 5);
        let b = zone_ranks(&s, 0, 5);
        assert_eq!(a, b);
        assert_eq!(a.len(), 4, "3 members + 1 full node");
        assert_eq!(*a.last().unwrap(), umis_core::SecretKey::from_label("full").node_id());
        assert_ne!(zone_ranks(&s, 0, 6), a, "a new slot reshuffles (with overwhelming probability)");
        let p = beacon_ranks(&s, 3);
        assert_eq!(p.len(), 7);
        let minter = a[0];
        assert_ne!(priority_prover(&s, 0, 5, &minter), Some(minter));
    }

    #[test]
    fn genesis_state_wires_face_links() {
        let c = ctx();
        let s = BeaconState::genesis(&c).unwrap();
        // zone 0 at (0,0,0) consumes from zone 1 (+x) and zone 2 (+y)
        let froms: Vec<u32> = s.zones[&0].inputs.iter().map(|l| l.from_zone).collect();
        assert_eq!(froms, vec![1, 2]);
        assert_eq!(s.zones[&3].inputs.len(), 2);
        assert_eq!(s.members_of(1).len(), 3);
        let h = s.hash();
        assert_eq!(BeaconState::genesis(&c).unwrap().hash(), h);
    }
}
