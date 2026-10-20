//! Physics reference: runs a whole domain in one process, without consensus,
//! using the idealized input schedule where every consumer slot `j_c` takes
//! the causally freshest producer output: the largest producer slot `j_p` with
//! `(j_p + 1) · S_p ≤ j_c · S_c` (its outputs are those of state
//! `X_p(j_p + 1)`). Before any producer slot qualifies, the link uses the
//! task's empty-neighbour boundary, as in the network protocol.

use crate::{Bytes, PortId, ZoneInstance};
use std::collections::BTreeMap;
use std::sync::Arc;
use umis_core::Hash;

pub struct SimZone {
    pub id: u32,
    /// `S_z` in ticks.
    pub slot_ticks: u64,
    pub inst: Arc<dyn ZoneInstance>,
    /// State at the start of `next_slot`.
    pub state: Bytes,
    pub next_slot: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimLink {
    pub from_zone: u32,
    pub from_port: PortId,
    pub to_zone: u32,
    pub to_port: PortId,
}

pub struct DomainSim {
    pub zones: BTreeMap<u32, SimZone>,
    pub links: Vec<SimLink>,
    /// zone → (end tick of the producing slot, outputs of the state after it),
    /// oldest first; trimmed to what consumers can still need.
    outputs: BTreeMap<u32, Vec<(u64, BTreeMap<PortId, Bytes>)>>,
    /// Chain of `(zone, slot, state_root)` hashes; equal traces mean equal runs.
    trace: Hash,
}

impl DomainSim {
    pub fn new(zones: Vec<SimZone>, links: Vec<SimLink>) -> Self {
        Self {
            outputs: zones.iter().map(|z| (z.id, Vec::new())).collect(),
            zones: zones.into_iter().map(|z| (z.id, z)).collect(),
            links,
            trace: Hash::ZERO,
        }
    }

    pub fn trace_hash(&self) -> Hash {
        self.trace
    }

    /// The zone slot that starts next in global time, if before `until_tick`.
    fn next_due(&self, until_tick: u64) -> Option<u32> {
        self.zones
            .values()
            .map(|z| (z.next_slot * z.slot_ticks, z.id))
            .filter(|(t, _)| *t < until_tick)
            .min()
            .map(|(_, id)| id)
    }

    /// Steps every zone slot that starts before `until_tick`.
    pub fn run_until(&mut self, until_tick: u64) -> crate::Result<()> {
        while let Some(zid) = self.next_due(until_tick) {
            self.step_zone(zid)?;
        }
        Ok(())
    }

    fn step_zone(&mut self, zid: u32) -> crate::Result<()> {
        let z = &self.zones[&zid];
        let start_tick = z.next_slot * z.slot_ticks;
        let mut inputs = BTreeMap::new();
        for l in self.links.iter().filter(|l| l.to_zone == zid) {
            let hist = &self.outputs[&l.from_zone];
            if let Some((_, outs)) = hist.iter().rev().find(|(end, _)| *end <= start_tick)
                && let Some(p) = outs.get(&l.from_port)
            {
                inputs.insert(l.to_port, p.clone());
            }
        }
        let z = self.zones.get_mut(&zid).expect("zone");
        let slot = z.next_slot;
        let next = z.inst.step(&z.state, &inputs, slot)?;
        let outs = z.inst.outputs(&next)?;
        let root = z.inst.state_root(&next);
        z.state = next;
        z.next_slot += 1;
        let end_tick = z.next_slot * z.slot_ticks;
        self.trace = umis_core::keccak256_concat(&[&self.trace.0, &zid.to_be_bytes(), &slot.to_be_bytes(), &root.0]);
        let hist = self.outputs.get_mut(&zid).expect("zone");
        hist.push((end_tick, outs));
        // Consumers start no earlier than the slowest zone's next start tick.
        let horizon = self.zones.values().map(|z| z.next_slot * z.slot_ticks).min().unwrap_or(0);
        let hist = self.outputs.get_mut(&zid).expect("zone");
        while hist.len() > 2 && hist[1].0 <= horizon {
            hist.remove(0);
        }
        Ok(())
    }
}

/// Builds the reference simulation for the genesis zones: instantiates every
/// zone and wires the links given by the topology.
pub fn from_genesis(g: &umis_core::genesis::Genesis, reg: &crate::TaskRegistry) -> crate::Result<DomainSim> {
    use crate::topology::{Coupling, Lattice3d, LayerId, Location, links_between};
    let umis_core::genesis::TopologySpec::Lattice3d { bounds } = &g.topology;
    let topo = Lattice3d { bounds: *bounds };
    let couplings: Vec<Coupling> = g
        .couplings
        .iter()
        .map(|c| Coupling {
            layer_a: LayerId::new(&c.layer_a),
            port_a: c.port_a,
            layer_b: LayerId::new(&c.layer_b),
            port_b: c.port_b,
        })
        .collect();
    let mut zones = Vec::new();
    for z in &g.zones {
        let task = reg.get(&z.task_id)?;
        let params = task.params_from_json(&z.params)?;
        let inst = task.instantiate(&params, &g.seed)?;
        zones.push(SimZone { id: z.id, slot_ticks: z.slot_ticks, state: inst.initial_state(0), inst, next_slot: 0 });
    }
    let loc = |z: &umis_core::genesis::GenesisZone| Location { point: z.point, layer: LayerId::new(&z.layer) };
    let mut links = Vec::new();
    for a in &g.zones {
        for b in &g.zones {
            for l in links_between(&topo, &couplings, &loc(a), &loc(b)) {
                links.push(SimLink { from_zone: a.id, from_port: l.from_port, to_zone: b.id, to_port: l.to_port });
            }
        }
    }
    Ok(DomainSim::new(zones, links))
}
