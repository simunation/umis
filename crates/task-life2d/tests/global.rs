//! Global exactness: every state any zone ever reaches equals the monolithic
//! Game of Life of the whole world at that state's generation, whatever input
//! schedule the zones see (fresh, stale, out of order, skipped slots), with
//! mixed slot lengths and zones created at runtime.

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use std::collections::BTreeMap;
use std::sync::Arc;
use task_life2d::devnet::LifeOptions;
use task_life2d::reference::{RefZone, check_global};
use task_life2d::{Life2dParams, State, decode_state};
use umis_core::{Hash, NodeId};
use umis_task::topology::{Lattice3d, LayerId, Location, links_between};
use umis_task::{Bytes, DynZoneTask, PortId, TaskAdapter, ZoneInstance};

struct Zone {
    point: [i32; 3],
    slot_ticks: u64,
    params: Life2dParams,
    inst: Arc<dyn ZoneInstance>,
    state: Bytes,
    next_slot: u64,
    /// `(end tick of the producing slot, outputs)`, oldest first.
    outs: Vec<(u64, BTreeMap<PortId, Bytes>)>,
}

#[derive(Clone, Copy)]
enum Schedule {
    /// Every consumer takes the freshest causal output.
    Fresh,
    /// Random causal outputs (stale, non-monotonic) and random skipped slots.
    Adversarial,
}

struct World {
    seed: Hash,
    opts: LifeOptions,
    topo: Lattice3d,
    zones: Vec<Zone>,
    /// consumer → [(producer, producer port, consumer port)]
    links: BTreeMap<usize, Vec<(usize, PortId, PortId)>>,
    /// Every born state reached: (zone, state).
    seen: Vec<(usize, State)>,
    rng: ChaCha8Rng,
}

impl World {
    fn new(region: [i32; 2], n: u32, k: u32, slot_ticks: &[u64], rng_seed: u64) -> Self {
        let seed = umis_core::keccak256(&rng_seed.to_be_bytes());
        let mut opts = LifeOptions::new(seed, 0, NodeId::default());
        opts.n = n;
        opts.gens = k;
        opts.region = region;
        opts.density_pct = 35;
        opts.strips = 1;
        let mut w = World {
            seed,
            topo: Lattice3d { bounds: [8, 8, 1], diagonal_layers: vec![LayerId::new("life")] },
            zones: Vec::new(),
            links: BTreeMap::new(),
            seen: Vec::new(),
            rng: ChaCha8Rng::seed_from_u64(rng_seed),
            opts,
        };
        for (i, p) in w.opts.genesis_points().into_iter().enumerate() {
            w.add_zone(p, slot_ticks[i % slot_ticks.len()], 0);
        }
        w
    }

    fn add_zone(&mut self, point: [i32; 3], slot_ticks: u64, start_slot: u64) {
        let params = self.opts.zone_params(point, slot_ticks);
        let task = TaskAdapter::<task_life2d::Life2d>::new();
        let inst = task.instantiate(&borsh::to_vec(&params).unwrap(), &self.seed).unwrap();
        let state = inst.initial_state(start_slot);
        let id = self.zones.len();
        let loc = |p: [i32; 3]| Location { point: p, layer: LayerId::new("life") };
        for (j, other) in self.zones.iter().enumerate() {
            for l in links_between(&self.topo, &[], &loc(other.point), &loc(point)) {
                self.links.entry(id).or_default().push((j, l.from_port, l.to_port));
            }
            for l in links_between(&self.topo, &[], &loc(point), &loc(other.point)) {
                self.links.entry(j).or_default().push((id, l.from_port, l.to_port));
            }
        }
        self.zones.push(Zone { point, slot_ticks, params, inst, state, next_slot: start_slot, outs: Vec::new() });
    }

    fn now_next(&self) -> (u64, usize) {
        self.zones.iter().enumerate().map(|(i, z)| (z.next_slot * z.slot_ticks, i)).min().unwrap()
    }

    /// Runs zone slots in global time order until `until_tick`.
    fn run(&mut self, until_tick: u64, sched: Schedule) {
        loop {
            let (t, zi) = self.now_next();
            if t >= until_tick {
                break;
            }
            let skip = matches!(sched, Schedule::Adversarial) && self.rng.next_u32() % 5 == 0;
            let slot = self.zones[zi].next_slot;
            if skip {
                // The slot has no block; the next block covers it with the same inputs.
                self.zones[zi].next_slot += 1;
                continue;
            }
            let mut inputs = BTreeMap::new();
            for &(p, from_port, to_port) in self.links.get(&zi).map(|v| v.as_slice()).unwrap_or(&[]) {
                let causal: Vec<&(u64, BTreeMap<PortId, Bytes>)> =
                    self.zones[p].outs.iter().filter(|(end, _)| *end <= t).collect();
                if causal.is_empty() {
                    continue; // allowed: the producer has no causal block
                }
                let pick = match sched {
                    Schedule::Fresh => causal.len() - 1,
                    Schedule::Adversarial => {
                        if self.rng.next_u32() % 2 == 0 {
                            causal.len() - 1
                        } else {
                            (self.rng.next_u32() as usize) % causal.len()
                        }
                    }
                };
                inputs.insert(to_port, causal[pick].1[&from_port].clone());
            }
            let z = &mut self.zones[zi];
            z.state = z.inst.step(&z.state, &inputs, slot).unwrap();
            z.next_slot = slot + 1;
            let outs = z.inst.outputs(&z.state).unwrap();
            z.outs.push(((slot + 1) * z.slot_ticks, outs));
            let s = decode_state(&z.params, &z.state).unwrap();
            if s.born {
                self.seen.push((zi, s));
            }
        }
    }

    /// Births of created zones (genesis zones are born at 0).
    fn births(&self) -> Vec<Option<u64>> {
        self.zones
            .iter()
            .map(|z| {
                let s = decode_state(&z.params, &z.state).unwrap();
                s.born.then_some(s.birth)
            })
            .collect()
    }

    fn check(&self) -> (usize, u64) {
        let births = self.births();
        let zones: Vec<RefZone> =
            self.zones.iter().zip(&births).map(|(z, b)| RefZone { params: z.params.clone(), birth: *b }).collect();
        let n = check_global(&self.seed, &zones, &self.seen).unwrap_or_else(|e| panic!("{e}"));
        (n, self.seen.iter().map(|(_, s)| s.generation).max().unwrap_or(0))
    }

    fn generation(&self, zi: usize) -> u64 {
        decode_state(&self.zones[zi].params, &self.zones[zi].state).unwrap().generation
    }
}

#[test]
fn fresh_schedule_is_exact_and_lockstep() {
    let mut w = World::new([3, 3], 16, 2, &[4], 1);
    w.run(4 * 40, Schedule::Fresh);
    let (states, max_gen) = w.check();
    // Slot 0 uses the neighbours' genesis soups, so no slot is lost.
    for zi in 0..w.zones.len() {
        assert_eq!(w.generation(zi), 2 * 40, "zone {zi} kept pace");
    }
    assert!(states >= 9 * 40 && max_gen == 80);
    let pop: usize = w.zones.iter().map(|z| decode_state(&z.params, &z.state).unwrap().grid.population()).sum();
    assert!(pop > 0, "the soup is still alive");
}

#[test]
fn mixed_slot_lengths_are_exact() {
    // Slower zones gate faster ones: every zone advances at the pace of its
    // slowest neighbourhood, and stays exact.
    let mut w = World::new([2, 2], 16, 2, &[4, 8, 4, 16], 2);
    w.run(16 * 30, Schedule::Fresh);
    let (_, max_gen) = w.check();
    assert!(max_gen >= 2 * 25, "progress: max generation {max_gen}");
}

#[test]
fn adversarial_schedules_never_break_exactness() {
    for seed in 0..6 {
        let mut w = World::new([3, 2], 16, 4, &[4, 4, 8], 10 + seed);
        w.run(4 * 120, Schedule::Adversarial);
        let (states, max_gen) = w.check();
        eprintln!("adversarial seed {seed}: {states} states checked, max gen {max_gen}");
        assert!(states > 0 && max_gen >= 4 * 5, "seed {seed}: some progress (max gen {max_gen})");
        // Neighbouring zones are never more than k apart.
        for (c, ls) in &w.links {
            for (p, _, _) in ls {
                let (gc, gp) = (w.generation(*c), w.generation(*p));
                assert!(gc.abs_diff(gp) <= 4, "seed {seed}: neighbours at {gc} and {gp}");
            }
        }
    }
}

#[test]
fn created_zones_are_born_consistently() {
    for (seed, sched) in [(20, Schedule::Fresh), (21, Schedule::Adversarial), (22, Schedule::Adversarial)] {
        let mut w = World::new([2, 2], 16, 2, &[4], seed);
        w.run(4 * 10, sched);
        w.add_zone([2, 1, 0], 4, 10);
        w.run(4 * 14, sched);
        // Two adjacent zones created close together, one with a slower slot.
        w.add_zone([2, 2, 0], 8, 7);
        w.add_zone([1, 2, 0], 4, 14);
        w.run(4 * 80, sched);
        let births = w.births();
        assert!(births.iter().all(|b| b.is_some()), "seed {seed}: every zone is born: {births:?}");
        let (states, max_gen) = w.check();
        let pops: Vec<usize> =
            w.zones.iter().map(|z| decode_state(&z.params, &z.state).unwrap().grid.population()).collect();
        eprintln!("seed {seed}: births {births:?}, {states} states checked, max gen {max_gen}, populations {pops:?}");
        assert!(max_gen > births.iter().flatten().copied().max().unwrap(), "seed {seed}: progress after the births");
    }
}

#[test]
fn a_silent_neighbour_stalls_its_neighbourhood() {
    // Zone 0 never steps after slot 3: its neighbours stop within k of it,
    // and the hold spreads outward instead of letting anyone run ahead.
    let mut w = World::new([3, 1], 16, 2, &[4], 30);
    w.run(4 * 4, Schedule::Fresh);
    w.zones[0].slot_ticks = 1 << 40;
    w.run(4 * 30, Schedule::Fresh);
    w.check();
    let g: Vec<u64> = (0..3).map(|z| w.generation(z)).collect();
    assert_eq!(g[0], 8);
    assert!(g[1] <= g[0] + 2 && g[2] <= g[1] + 2, "generations {g:?}");
}

#[test]
fn showcase_world_is_exact() {
    // Gliders through zone corners, a gun, spaceships leaving the world, methuselahs on
    // junctions: the zones, each a 64x64 board with halos, equal one monolithic board.
    for (k, sched) in [(1, Schedule::Fresh), (2, Schedule::Fresh)] {
        let seed = umis_core::keccak256(&7u64.to_be_bytes());
        let mut w = World::new([4, 3], 64, k, &[4], 7);
        w.seed = seed;
        w.opts.pattern = 1;
        let points = w.opts.genesis_points();
        w.zones.clear();
        w.links.clear();
        w.seen.clear();
        w.opts.seed = seed;
        for p in points {
            w.add_zone(p, 4, 0);
        }
        w.run(4 * 60, sched);
        let (states, max_gen) = w.check();
        assert!(states >= 12 * 60 && max_gen == 60 * k as u64, "k={k}: {states} states, max gen {max_gen}");
    }
}
