//! Scenarios: each builds a simulation, stages its events, runs it, and
//! returns the oracle report plus scenario-specific checks.

use crate::net::DelayRule;
use crate::oracle::{self, Report};
use crate::probe;
use crate::{Sim, SimConfig};
use std::collections::BTreeMap;
use umis_core::records::ProposeZone;
use umis_core::{Hash, NodeId, SegRecord, ShortStr};
use umis_protocol::beacon::{CommitStatus, ProposalStatus};
use umis_protocol::types::{ProposeZoneTx, Topic};
use umis_protocol::{Command, VotePolicy, ranks};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    Basic,
    Churn,
    Partition,
    Adversary,
    Faults,
    ZoneCreate,
    ZoneReject,
    ZoneExpire,
    ZoneLayers,
    ZoneSparse,
    Stark,
    LifeBasic,
    LifeMixed,
    LifeCreate,
    LifeFaults,
    LifeChurn,
    LifePartition,
    LifeStark,
    LifeStarkKeccak,
    LifeSlow,
}

/// The Game of Life scenarios (globally exact zones, see `task_life2d`).
pub const LIFE: [&str; 9] = [
    "life",
    "life-mixed",
    "life-create",
    "life-faults",
    "life-churn",
    "life-partition",
    "life-stark",
    "life-stark-keccak",
    "life-slow",
];

pub const ALL: [&str; 11] = [
    "basic",
    "churn",
    "partition",
    "adversary",
    "faults",
    "zone-create",
    "zone-reject",
    "zone-expire",
    "zone-layers",
    "zone-sparse",
    "stark",
];

impl std::str::FromStr for Scenario {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "basic" => Scenario::Basic,
            "churn" => Scenario::Churn,
            "partition" => Scenario::Partition,
            "adversary" => Scenario::Adversary,
            "faults" => Scenario::Faults,
            "zone-create" => Scenario::ZoneCreate,
            "zone-reject" => Scenario::ZoneReject,
            "zone-expire" => Scenario::ZoneExpire,
            "zone-layers" => Scenario::ZoneLayers,
            "zone-sparse" => Scenario::ZoneSparse,
            "stark" => Scenario::Stark,
            "life" => Scenario::LifeBasic,
            "life-mixed" => Scenario::LifeMixed,
            "life-create" => Scenario::LifeCreate,
            "life-faults" => Scenario::LifeFaults,
            "life-churn" => Scenario::LifeChurn,
            "life-partition" => Scenario::LifePartition,
            "life-stark" => Scenario::LifeStark,
            "life-stark-keccak" => Scenario::LifeStarkKeccak,
            "life-slow" => Scenario::LifeSlow,
            _ => return Err(format!("unknown scenario {s}")),
        })
    }
}

pub struct Outcome {
    pub report: Report,
    pub trace: Hash,
    pub sim: Sim,
}

pub fn run(s: Scenario, seed: u64, slots: u64) -> Outcome {
    let cfg = SimConfig::new(seed);
    match s {
        Scenario::Basic => basic(cfg, slots),
        Scenario::Churn => churn(cfg, slots.max(60)),
        Scenario::Partition => partition(cfg, slots.max(60)),
        Scenario::Adversary => adversary(cfg, slots.max(40)),
        Scenario::Faults => faults(cfg, slots.max(60)),
        Scenario::ZoneCreate => zone_create(cfg, slots.max(50)),
        Scenario::ZoneReject => zone_reject(cfg, slots.max(40)),
        Scenario::ZoneExpire => zone_expire(cfg, slots.max(40)),
        Scenario::ZoneLayers => zone_layers(cfg, slots.max(50)),
        Scenario::ZoneSparse => zone_sparse(cfg, slots.max(50)),
        Scenario::Stark => stark(cfg, slots.clamp(20, 40)),
        Scenario::LifeBasic => life_checked(basic(SimConfig::life(seed), slots), true),
        Scenario::LifeMixed => {
            let cfg = SimConfig { slot_ticks: vec![4, 8, 4, 16], ..SimConfig::life(seed) };
            life_checked(basic(cfg, slots), false)
        }
        Scenario::LifeCreate => life_checked(life_create(SimConfig::life(seed), slots.max(50)), false),
        Scenario::LifeFaults => life_checked(faults(SimConfig::life(seed), slots.max(60)), false),
        Scenario::LifeChurn => life_checked(churn(SimConfig::life(seed), slots.max(60)), false),
        Scenario::LifePartition => life_checked(partition(SimConfig::life(seed), slots.max(60)), false),
        Scenario::LifeStark => life_checked(stark(SimConfig::life(seed), slots.clamp(20, 40)), false),
        Scenario::LifeStarkKeccak => {
            let mut cfg = SimConfig::life(seed);
            cfg.stark_config = Some("koalabear-keccak-test".into());
            life_checked(stark(cfg, slots.clamp(20, 40)), false)
        }
        Scenario::LifeSlow => life_checked(life_slow(SimConfig::life(seed), slots.max(40)), false),
    }
}

/// Generation of every life zone's best-known committed state.
pub fn life_generations(sim: &Sim) -> BTreeMap<u32, (u64, u64)> {
    let a = sim.node(sim.archive);
    let head = a.chain.head_state();
    let mut out = BTreeMap::new();
    for (z, zone) in head.zones.iter().filter(|(_, z)| z.task_id == task_life2d::TASK_ID) {
        let Ok(p) = borsh::from_slice::<task_life2d::Life2dParams>(&zone.params) else { continue };
        let Some(c) = head.commits.get(z).and_then(|v| v.iter().rev().find(|c| c.status != CommitStatus::Invalid))
        else {
            continue;
        };
        let Some(bs) = c.blocks.last() else { continue };
        let Some(st) = a.engines.get(z).and_then(|e| e.get(&bs.hash)).and_then(|e| e.state.clone()) else { continue };
        if let Some(s) = task_life2d::decode_state(&p, &st) {
            out.insert(*z, (s.generation, c.to_slot));
        }
    }
    out
}

/// Adds the life checks: the oracle already compares settled states with the
/// global Game of Life; `lockstep` also requires every zone to have advanced
/// on every slot after the first (an all-honest network loses no generation).
fn life_checked(mut o: Outcome, lockstep: bool) -> Outcome {
    let gens = life_generations(&o.sim);
    if gens.is_empty() {
        o.report.violations.push("life: no committed life state".into());
    }
    let head = o.sim.node(o.sim.archive).chain.head_state().clone();
    for (z, (g, to_slot)) in &gens {
        let zone = &head.zones[z];
        let p: task_life2d::Life2dParams = borsh::from_slice(&zone.params).expect("params");
        let ideal = to_slot.saturating_sub(zone.j_start) * p.gens as u64;
        if lockstep && *g != ideal {
            o.report.violations.push(format!("life zone {z}: generation {g} after slot {to_slot}, expected {ideal}"));
        }
        if *g == 0 && zone.j_start == 0 {
            o.report.violations.push(format!("life zone {z} never advanced"));
        }
    }
    o
}

/// Zone 1's first outputs reach everyone a slot late: its neighbours cannot
/// mint slot 1 (a strict task waits for the input) and their next block spans
/// slots 1..j. Such a multi-slot block holds the inputs it had, but must still
/// take a link's first input, or the zones would wait forever.
pub fn life_slow(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    let z1 = sim.node(0).chain.head_state().zones[&1].clone();
    let slot_ms = z1.slot_ticks * sim.ctx.p.tick_ms;
    let until_ms = sim.ctx.zone_slot_start(z1.slot_ticks, 1);
    for i in 0..sim.net.nodes.len() {
        let from = sim.node(i).me;
        sim.net.delays.push(DelayRule { from, topic: Topic::ZoneOut(1), extra_ms: 2 * slot_ms, until_ms });
    }
    sim.run_to_slot(slots);
    let report = oracle::check(&sim, 3, slots.saturating_sub(2), 3);
    finish(sim, report)
}

/// A zone created next to the genesis square (linked to zone 1 by an edge and
/// to zone 3 by a corner) is born dead at its neighbours' generation and joins
/// the exact world.
pub fn life_create(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(8);
    let z1 = members_of(&sim, 1);
    let id = propose(&mut sim, z1[0], &[z1[1]], [2, 0, 0], task_life2d::LAYER, 4);
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 3, slots.saturating_sub(2), 6);
    match proposal_status(&sim, &id) {
        Some(ProposalStatus::Accepted { zone, .. }) => {
            check_created(&sim, &mut report, zone, 1);
            let head = sim.node(sim.archive).chain.head_state().clone();
            if head.zones[&zone].inputs.len() != 2 {
                report.violations.push(format!(
                    "created life zone has {} input links, expected 2 (edge + corner)",
                    head.zones[&zone].inputs.len()
                ));
            }
            match life_generations(&sim).get(&zone) {
                Some((g, _)) if *g > 0 => {}
                g => report.violations.push(format!("created life zone not born and advancing: {g:?}")),
            }
        }
        s => report.violations.push(format!("proposal not accepted: {s:?}")),
    }
    finish(sim, report)
}

fn finish(sim: Sim, report: Report) -> Outcome {
    let trace = sim.trace_hash();
    Outcome { report, trace, sim }
}

/// All honest: 4 zones × members, full nodes, `slots` beacon slots.
pub fn basic(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(slots);
    let report = oracle::check(&sim, 3, slots.saturating_sub(2), 3);
    finish(sim, report)
}

fn members_of(sim: &Sim, z: u32) -> Vec<usize> {
    let st = sim.node(sim.archive).chain.head_state();
    st.members_of(z).iter().filter_map(|n| sim.index_of(n)).collect()
}

/// Members leave and return, a full node drops out, new nodes join late.
pub fn churn(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(12);
    let z0 = members_of(&sim, 0);
    let (a, b) = (z0[0], z0[1]);
    sim.net.set_online(a, false);
    sim.net.set_online(b, false);
    sim.run_to_slot(15);
    let late: Vec<usize> = (0..3).map(|k| sim.add_node(&format!("late{k}"))).collect();
    for &l in &late {
        let to = sim.node(l).me;
        sim.command(0, Command::Grant { to, amount: 500 });
    }
    sim.run_to_slot(18);
    for &l in &late {
        sim.command(l, Command::Join { stake: 200, full_node: false });
    }
    sim.run_to_slot(21);
    sim.net.set_online(a, true);
    sim.net.set_online(b, true);
    sim.run_to_slot(25);
    sim.net.set_online(1, false);
    sim.run_to_slot(35);
    sim.net.set_online(1, true);
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 3, slots.saturating_sub(2), 4);
    let head = sim.node(sim.archive).chain.head_state().clone();
    for &l in &late {
        let me = sim.node(l).me;
        match head.nodes.get(&me).and_then(|n| n.zone) {
            Some(z) => {
                let n = sim.node(l);
                if !n.engines.get(&z).is_some_and(|e| e.has_root()) {
                    report.violations.push(format!("late joiner {l} has no state for zone {z}"));
                } else if n.stats.zone_blocks_accepted == 0 {
                    report.violations.push(format!("late joiner {l} never validated a block of zone {z}"));
                }
            }
            None => report.violations.push(format!("late joiner {l} is not a member")),
        }
    }
    finish(sim, report)
}

/// A third of the network is cut off for 10 beacon slots, then heals.
pub fn partition(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(15);
    let mut groups = BTreeMap::new();
    groups.insert(1usize, 1u32);
    for i in members_of(&sim, 3) {
        groups.insert(i, 1);
    }
    sim.net.partition(&groups);
    sim.run_to_slot(25);
    let heal: BTreeMap<usize, u32> = groups.keys().map(|i| (*i, 0)).collect();
    sim.net.partition(&heal);
    sim.run_to_slot(slots);
    let report = oracle::check(&sim, 40, slots.saturating_sub(2), 4);
    finish(sim, report)
}

/// Members ranked first in `zone` slot `j` under the archive's anchor.
fn minter_at(sim: &Sim, z: u32, j: u64, rank: usize) -> Option<usize> {
    let n = sim.node(sim.archive);
    let zone = n.chain.head_state().zones.get(&z)?.clone();
    let (_, a) = n.chain.anchor(sim.ctx.timing(zone.slot_ticks).beacon_slot_of(j))?;
    let list = ranks::zone_ranks(&a, z, j);
    let who = *list.get(rank)?;
    let i = sim.index_of(&who)?;
    (i >= sim.cfg.full_nodes).then_some(i)
}

/// A slot of zone `z` in the next beacon slot whose anchor is already known
/// (`head + D`) with a rank-0 member satisfying `ok`; runs the simulation one
/// slot at a time until one is found. `last_of_commit` picks only
/// commit-period ends.
fn pick_slot(sim: &mut Sim, z: u32, last_of_commit: bool, ok: impl Fn(&Sim, usize) -> bool) -> Option<(u64, usize)> {
    for _ in 0..12 {
        let head = sim.node(sim.archive).chain.head().slot.unwrap_or(0);
        let e = head + sim.ctx.p.anchor_depth;
        let zone = sim.node(sim.archive).chain.head_state().zones.get(&z)?.clone();
        let t = sim.ctx.timing(zone.slot_ticks);
        let (j0, j1) = (t.first_slot_at_or_after(e), t.first_slot_at_or_after(e + 1));
        for j in j0..j1.max(j0 + 1) {
            if last_of_commit && t.j_star(t.commit_slot_of(j)) != j + 1 {
                continue;
            }
            if let Some(i) = minter_at(sim, z, j, 0)
                && ok(sim, i)
            {
                return Some((j, i));
            }
        }
        sim.run_to_slot(head + 2);
    }
    None
}

/// An equivocating minter and a minter recording a forged InputRef.
pub fn adversary(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(12);
    if let Some((j, x)) = pick_slot(&mut sim, 2, false, |_, _| true) {
        sim.net.nodes[x].node.cfg.adversary.equivocate_at = Some((2, j));
        sim.honest[x] = false;
        sim.injected.push((sim.node(x).me, 2, j));
    }
    if let Some((j, y)) = pick_slot(&mut sim, 0, false, |s, i| s.honest[i]) {
        sim.net.nodes[y].node.cfg.adversary.forge_input_at = Some((0, j));
        sim.honest[y] = false;
        sim.injected.push((sim.node(y).me, 0, j));
    }
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 3, slots.saturating_sub(2), 4);
    if sim.injected.len() < 2 {
        report.violations.push("could not stage both adversaries".into());
    }
    finish(sim, report)
}

/// Invalid transition (by a delegate), an orphaned input from a fallback race,
/// a missed proof deadline, and two overlapping faults.
pub fn faults(mut cfg: SimConfig, slots: u64) -> Outcome {
    cfg.proof_lag = 1;
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(10);
    // a) invalid transition at the end of a zone-1 commit period
    if let Some((j, m)) = pick_slot(&mut sim, 1, true, |_, _| true) {
        sim.net.nodes[m].node.cfg.adversary.corrupt_at = Some((1, j));
        sim.honest[m] = false;
        sim.injected.push((sim.node(m).me, 1, j));
    }
    sim.run_to_slot(18);
    // b) fallback race orphaning an input: zone 1's rank-0 block reaches its
    //    members after the rank-1 window, and its outputs reach consumers late.
    let zone1 = sim.node(0).chain.head_state().zones[&1].clone();
    let t1 = sim.ctx.timing(zone1.slot_ticks);
    let mut staged_orphan = false;
    for j in t1.first_slot_at_or_after(19)..t1.first_slot_at_or_after(19) + 8 {
        if let (Some(m0), Some(_m1)) = (minter_at(&sim, 1, j, 0), minter_at(&sim, 1, j, 1))
            && sim.honest[m0]
        {
            let me = sim.node(m0).me;
            let start = sim.ctx.zone_slot_start(zone1.slot_ticks, j);
            let w = sim.ctx.zone_window_ms(zone1.slot_ticks);
            let slot_ms = zone1.slot_ticks * sim.ctx.p.tick_ms;
            sim.net.delays.push(DelayRule {
                from: me,
                topic: Topic::ZoneBlock(1),
                extra_ms: w + 200,
                until_ms: start + slot_ms,
            });
            sim.net.delays.push(DelayRule {
                from: me,
                topic: Topic::ZoneOut(1),
                extra_ms: slot_ms + 1200,
                until_ms: start + slot_ms,
            });
            staged_orphan = true;
            break;
        }
    }
    sim.run_to_slot(26);
    // c) missing proof: zone 2 withholds every proof of commit slot 28
    let z2: Vec<usize> = members_of(&sim, 2).into_iter().chain(0..sim.cfg.full_nodes).collect();
    for i in z2 {
        sim.net.nodes[i].node.cfg.adversary.withhold_proofs = Some((2, 28));
    }
    sim.run_to_slot(34);
    // d) two overlapping faults
    if let Some((j, m)) = pick_slot(&mut sim, 0, false, |s, i| s.honest[i]) {
        sim.net.nodes[m].node.cfg.adversary.corrupt_at = Some((0, j));
        sim.honest[m] = false;
        sim.injected.push((sim.node(m).me, 0, j));
    }
    if let Some((j, m)) = pick_slot(&mut sim, 3, false, |s, i| s.honest[i]) {
        sim.net.nodes[m].node.cfg.adversary.equivocate_at = Some((3, j));
        sim.honest[m] = false;
        sim.injected.push((sim.node(m).me, 3, j));
    }
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 3, slots.saturating_sub(2), 6);
    let head = sim.node(sim.archive).chain.head_state().clone();
    let reasons: Vec<_> = head.rollback_log.iter().map(|r| format!("{:?}", r.reason)).collect();
    if !reasons.iter().any(|r| r == "MissingProof") {
        report.violations.push(format!("no missing-proof rollback (rollbacks: {reasons:?})"));
    }
    if staged_orphan && !reasons.iter().any(|r| r == "CrossLink") {
        report.violations.push(format!("no cross-link rollback (rollbacks: {reasons:?})"));
    }
    if sim.injected.len() < 3 {
        report.violations.push(format!("only {} faults staged", sim.injected.len()));
    }
    finish(sim, report)
}

/// Builds and submits a `ProposeZone` from `proposer` with `founders`.
pub fn propose(
    sim: &mut Sim,
    proposer: usize,
    founders: &[usize],
    point: [i32; 3],
    layer: &str,
    slot_ticks: u64,
) -> Hash {
    let (task_id, params) = if layer == task_life2d::LAYER {
        let p = task_life2d::devnet::zone_params_like(&sim.genesis, point, slot_ticks).expect("a life genesis");
        (task_life2d::TASK_ID, borsh::to_vec(&p).expect("borsh"))
    } else if layer == probe::PROBE_LAYER {
        let p = probe::ProbeParams { point, n_loc: sim.cfg.n_loc, slot_ticks: slot_ticks as u32 };
        (probe::PROBE_TASK, borsh::to_vec(&p).expect("borsh"))
    } else {
        let p = task_water3d::devnet::zone_params(point, slot_ticks, sim.cfg.n_loc);
        (task_water3d::TASK_ID, borsh::to_vec(&p).expect("borsh"))
    };
    let ids: Vec<NodeId> = founders.iter().map(|f| sim.node(*f).me).collect();
    let record = ProposeZone {
        px: point[0],
        py: point[1],
        pz: point[2],
        slot_ticks: slot_ticks as u32,
        founder_count: ids.len() as u16,
        layer_id: ShortStr::new(layer).expect("layer"),
        params_hash: umis_task::params_hash(task_id, layer, &params),
        founders_root: umis_core::bmt::list_root(&ids.iter().map(|n| n.as_hash()).collect::<Vec<_>>()),
        proposer: sim.node(proposer).me,
    };
    let id = record.address();
    let founder_sigs = founders.iter().map(|f| sim.keys[*f].sign(&id)).collect();
    let tx = ProposeZoneTx { record, task_id: task_id.into(), params, founders: ids, founder_sigs };
    sim.command(proposer, Command::Propose(Box::new(tx)));
    id
}

fn proposal_status(sim: &Sim, id: &Hash) -> Option<ProposalStatus> {
    sim.node(sim.archive).chain.head_state().proposals.get(id).map(|p| p.status)
}

/// Checks a created zone runs: it committed, its neighbour consumed it, and
/// every node computed the same start root.
fn check_created(sim: &Sim, report: &mut Report, zid: u32, neighbour: u32) {
    let head = sim.node(sim.archive).chain.head_state().clone();
    let Some(zone) = head.zones.get(&zid) else {
        report.violations.push(format!("zone {zid} missing"));
        return;
    };
    let valid = head.commits[&zid].iter().filter(|c| c.status != CommitStatus::Invalid).count();
    if valid == 0 {
        report.violations.push(format!("created zone {zid} never committed"));
    }
    let roots: std::collections::BTreeSet<Hash> = sim
        .honest_online()
        .iter()
        .filter_map(|i| sim.node(*i).chain.head_state().zones.get(&zid).map(|z| z.start_root))
        .collect();
    if roots.len() != 1 {
        report.violations.push(format!("zone {zid}: start roots differ across nodes"));
    }
    let eng = &sim.node(sim.archive).engines[&neighbour];
    let mut consumed = false;
    for e in eng.entries.values() {
        if let Some(b) = &e.block {
            for r in &b.inputs {
                if r.src_zone == zid {
                    consumed = true;
                    if r.src_slot < zone.j_start {
                        report.violations.push(format!("zone {neighbour} consumed zone {zid} before its start"));
                    }
                }
            }
        }
    }
    if !consumed {
        report.violations.push(format!("zone {neighbour} never consumed outputs of zone {zid}"));
    }
}

/// Accepted proposal + refused duplicate and linked proposals + a founder
/// whose tab closes before activation.
pub fn zone_create(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(8);
    let z1 = members_of(&sim, 1);
    let id = propose(&mut sim, z1[0], &[z1[1]], [2, 0, 0], "water", 4);
    sim.run_to_slot(9);
    // while it is pending: the same location, and a location linked to it, are refused
    let z2 = members_of(&sim, 2);
    let dup = propose(&mut sim, z2[0], &[z2[1]], [2, 0, 0], "water", 4);
    let linked = propose(&mut sim, z2[2], &[z2[3]], [2, 1, 0], "water", 4);
    sim.run_to_slot(12);
    sim.net.set_online(z1[1], false); // the founder's tab closes
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 3, slots.saturating_sub(2), 6);
    match proposal_status(&sim, &id) {
        Some(ProposalStatus::Accepted { zone, e_act, .. }) => {
            check_created(&sim, &mut report, zone, 1);
            let head = sim.node(sim.archive).chain.head_state();
            if head.zones[&zone].active_from != e_act {
                report.violations.push("zone activated at the wrong slot".into());
            }
        }
        s => report.violations.push(format!("proposal not accepted: {s:?}")),
    }
    for (name, p) in [("duplicate", dup), ("linked", linked)] {
        if proposal_status(&sim, &p).is_some() {
            report.violations.push(format!("{name} proposal was registered"));
        }
    }
    finish(sim, report)
}

/// The adjacent zone rejects.
pub fn zone_reject(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    for i in members_of(&sim, 1) {
        sim.net.nodes[i].node.cfg.vote_policy = VotePolicy::RejectAll;
    }
    sim.run_to_slot(8);
    let z1 = members_of(&sim, 1);
    let id = propose(&mut sim, z1[0], &[z1[1]], [2, 0, 0], "water", 4);
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 3, slots.saturating_sub(2), 4);
    if !matches!(proposal_status(&sim, &id), Some(ProposalStatus::Rejected { .. })) {
        report.violations.push(format!("expected rejection, got {:?}", proposal_status(&sim, &id)));
    }
    finish(sim, report)
}

/// The adjacent zone is offline: the proposal expires at its deadline.
pub fn zone_expire(cfg: SimConfig, slots: u64) -> Outcome {
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(8);
    let z1 = members_of(&sim, 1);
    for i in &z1 {
        sim.net.nodes[*i].node.cfg.vote_policy = VotePolicy::Manual;
    }
    let id = propose(&mut sim, z1[0], &[z1[1]], [2, 0, 0], "water", 4);
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 3, slots.saturating_sub(2), 4);
    match proposal_status(&sim, &id) {
        Some(ProposalStatus::Expired { e }) => {
            let p = &sim.node(sim.archive).chain.head_state().proposals[&id];
            if e != p.deadline + 1 {
                report.violations.push(format!("expired at {e}, deadline {}", p.deadline));
            }
        }
        s => report.violations.push(format!("expected expiry, got {s:?}")),
    }
    finish(sim, report)
}

/// A `probe` zone on the occupied water point (1,1,0): the co-located water
/// zone votes, the two exchange data, and a second water zone there is refused.
pub fn zone_layers(mut cfg: SimConfig, slots: u64) -> Outcome {
    cfg.probe_layer = true;
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(8);
    let z3 = members_of(&sim, 3);
    let id = propose(&mut sim, z3[0], &[z3[1]], [1, 1, 0], probe::PROBE_LAYER, 4);
    let occupied = propose(&mut sim, z3[2], &[z3[3]], [1, 1, 0], "water", 4);
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 3, slots.saturating_sub(2), 6);
    match proposal_status(&sim, &id) {
        Some(ProposalStatus::Accepted { zone, .. }) => {
            let p = &sim.node(sim.archive).chain.head_state().proposals[&id];
            if p.adjacent != vec![3] {
                report.violations.push(format!("probe zone adjacency {:?}, expected [3]", p.adjacent));
            }
            check_created(&sim, &mut report, zone, 3);
        }
        s => report.violations.push(format!("probe proposal not accepted: {s:?}")),
    }
    if proposal_status(&sim, &occupied).is_some() {
        report.violations.push("second water zone at an occupied point was registered".into());
    }
    finish(sim, report)
}

/// A sparse adjacent zone (S = 32, commits every 2 beacon slots) decides in time.
pub fn zone_sparse(mut cfg: SimConfig, slots: u64) -> Outcome {
    cfg.slot_ticks = vec![4, 8, 4, 32];
    let mut sim = Sim::new(cfg);
    sim.run_to_slot(8);
    let z3 = members_of(&sim, 3);
    let id = propose(&mut sim, z3[0], &[z3[1]], [2, 1, 0], "water", 8);
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 4, slots.saturating_sub(2), 5);
    match proposal_status(&sim, &id) {
        Some(ProposalStatus::Accepted { zone, .. }) => check_created(&sim, &mut report, zone, 3),
        s => report.violations.push(format!("proposal next to a sparse zone not accepted: {s:?}")),
    }
    finish(sim, report)
}

/// Real Plonky3 proofs (test FRI preset; water3d or life2d AIR by zone task) with proof lag L = 1: helpers prove
/// off the event loop, proof commitments follow their zone commitments one
/// beacon slot later, and beacon validators verify every STARK.
pub fn stark(mut cfg: SimConfig, slots: u64) -> Outcome {
    cfg.proof_backend = task_water3d::air::BACKEND_ID.into();
    cfg.proof_lag = 1;
    cfg.members_per_zone = 4;
    let mut sim = Sim::new(cfg);
    sim.net.prove_delay_ms = 300;
    sim.run_to_slot(slots);
    let mut report = oracle::check(&sim, 4, slots.saturating_sub(2), 4);
    let head = sim.node(sim.archive).chain.head_state().clone();
    let proven = head
        .commits
        .values()
        .flatten()
        .filter(|c| matches!(c.status, CommitStatus::Proven | CommitStatus::Settled))
        .count();
    if proven == 0 {
        report.violations.push("no commitment was proven with STARK proofs".into());
    }
    if !head.rollback_log.is_empty() {
        report.violations.push(format!("{} rollbacks with honest STARK provers", head.rollback_log.len()));
    }
    finish(sim, report)
}
