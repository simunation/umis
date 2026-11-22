//! Global exactness: lockstep zones exchanging halos compute exactly the monolithic world.
//! Each slot the world (padded by `h` cells of the boundary value, the outside) evolves `k`
//! generations; every zone's board must equal its part of it.

use std::collections::BTreeMap;
use std::sync::Arc;
use task_stencil::devnet::{StencilOptions, default_rule};
use task_stencil::{RuleId, Stencil2d, decode_state};
use umis_core::{Hash, NodeId};
use umis_stencil::{Grid, Rule};
use umis_task::topology::opposite_face;
use umis_task::{Bytes, DynZoneTask, TaskAdapter, ZoneInstance};

struct World {
    o: StencilOptions,
    zones: BTreeMap<[i32; 3], (Vec<u8>, Arc<dyn ZoneInstance>, Bytes)>,
}

impl World {
    fn new(rule: RuleId, n: u32, k: u32, region: [i32; 2], pattern: u32) -> Self {
        let seed = umis_core::keccak256(b"stencil-global");
        let mut o = StencilOptions::new(seed, 0, NodeId::default(), rule);
        (o.n, o.gens, o.region, o.rows_per_trace_row, o.pattern) = (n, k, region, 2.max(k), pattern);
        let mut zones = BTreeMap::new();
        for pt in o.genesis_points() {
            let params = borsh::to_vec(&o.zone_params(pt, 4)).unwrap();
            let inst = TaskAdapter::<Stencil2d>::new().instantiate(&params, &seed).unwrap();
            let s0 = inst.initial_state(0);
            zones.insert(pt, (params, inst, s0));
        }
        World { o, zones }
    }

    fn step(&mut self, slot: u64) {
        let outs: BTreeMap<[i32; 3], _> =
            self.zones.iter().map(|(pt, (_, inst, s))| (*pt, inst.outputs(s).unwrap())).collect();
        for (pt, (_, inst, s)) in self.zones.iter_mut() {
            let p = self.o.zone_params(*pt, 4);
            let mut inputs = BTreeMap::new();
            if slot > 0 {
                for q in task_stencil::PORTS {
                    if let Some(o) = outs.get(&p.neighbour_point(q)) {
                        inputs.insert(q, o[&opposite_face(q)].clone());
                    }
                }
            }
            *s = inst.step(s, &inputs, slot).unwrap();
        }
    }

    /// The world board assembled from the zones, and their common generation.
    fn board(&self) -> (Grid, u64) {
        let (n, rx, ry) = (self.o.n as usize, self.o.region[0] as usize, self.o.region[1] as usize);
        let rule = self.o.rule.rule();
        let mut w = Grid::filled(rx * n, ry * n, &rule.boundary());
        let mut gens = Vec::new();
        for (pt, (_, _, s)) in &self.zones {
            let st = decode_state(&self.o.zone_params(*pt, 4), s).unwrap();
            gens.push(st.generation);
            for y in 0..n {
                for x in 0..n {
                    w.cell_mut(pt[0] as usize * n + x, pt[1] as usize * n + y).copy_from_slice(st.grid.cell(x, y));
                }
            }
        }
        assert!(gens.iter().all(|g| *g == gens[0]), "lockstep zones share a generation");
        (w, gens[0])
    }
}

/// One slot of the monolithic world: pad with the boundary, evolve k generations.
fn monolithic_step(w: &Grid, rule: &impl Rule, k: usize) -> Grid {
    let h = k * rule.spec().radius;
    let mut e = Grid::filled(w.w + 2 * h, w.h + 2 * h, &rule.boundary());
    for y in 0..w.h {
        for x in 0..w.w {
            e.cell_mut(x + h, y + h).copy_from_slice(w.cell(x, y));
        }
    }
    e.evolve(rule, k)
}

#[test]
fn zones_compute_the_monolithic_world() {
    for (name, k, slots) in
        [("life", 1, 6), ("life", 2, 4), ("heat", 1, 5), ("heat", 2, 3), ("lbm", 1, 5), ("lbm", 2, 3)]
    {
        let rule_id = default_rule(name).unwrap();
        let rule = rule_id.rule();
        let mut world = World::new(rule_id, 8, k, [3, 2], 0);
        let (mut mono, g0) = world.board();
        assert_eq!(g0, 0);
        for slot in 0..slots {
            world.step(slot);
            mono = monolithic_step(&mono, &rule, k as usize);
            let (w, g) = world.board();
            assert_eq!(g, (slot + 1) * k as u64, "{name} k={k}");
            assert_eq!(w, mono, "{name} k={k} slot {slot}");
        }
    }
    let _ = Hash::ZERO;
}

#[test]
fn the_lbm_demo_stays_in_the_provable_regime() {
    // The default fluid (shear layer, ω = 1.875) on a 4×2 world of 16² zones for 400
    // generations: every slot of every zone stays inside the bounds the proof checks.
    let rule_id = default_rule("lbm").unwrap();
    let RuleId::Lbm { a, t } = rule_id else { unreachable!() };
    let lbm = umis_stencil::lbm::Lbm::new(a as i64, t as usize);
    let (mut mono, _) = World::new(rule_id, 16, 1, [4, 2], 0).board();
    let rule = rule_id.rule();
    for slot in 0..400u64 {
        let h = 1;
        let mut e = Grid::filled(mono.w + 2 * h, mono.h + 2 * h, &rule.boundary());
        for y in 0..mono.h {
            for x in 0..mono.w {
                e.cell_mut(x + h, y + h).copy_from_slice(mono.cell(x, y));
            }
        }
        assert!(lbm.provable(&e), "slot {slot} leaves the provable regime");
        mono = e.evolve(&rule, 1);
    }
}
