//! Invariant checks over a finished (or paused) simulation.

use crate::Sim;
use std::collections::{BTreeMap, BTreeSet};
use umis_core::bmt;
use umis_protocol::beacon::{CommitStatus, ProposalStatus};
use umis_protocol::exec;

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub violations: Vec<String>,
    pub head_slot: u64,
    pub finalized_slot: u64,
    /// zone → valid commitments included on the canonical chain.
    pub commits: BTreeMap<u32, usize>,
    pub invalidated: usize,
    pub rollbacks: usize,
    pub slashes: usize,
    pub zones: usize,
    pub max_commit_gap: BTreeMap<u32, u64>,
    pub avg_staleness_ms: f64,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Runs every invariant. `quiet_from`: beacon slots from which liveness is
/// required (after warm-up and scheduled disruptions).
pub fn check(sim: &Sim, live_from: u64, live_until: u64, max_gap: u64) -> Report {
    let mut r = Report::default();
    let honest = sim.honest_online();
    let a = sim.node(sim.archive);
    let head = a.chain.head_state().clone();
    r.head_slot = a.chain.head().slot.unwrap_or(0);
    r.finalized_slot = a.chain.finalized().slot.unwrap_or(0);
    r.zones = head.zones.len();

    // 1. Finalized beacon chains are prefixes of one another.
    let min_fin = honest.iter().map(|i| sim.node(*i).chain.finalized().height).min().unwrap_or(0);
    for h in 0..=min_fin {
        let hashes: BTreeSet<_> =
            honest.iter().filter_map(|i| sim.node(*i).chain.canonical_at_height(h).map(|e| e.hash)).collect();
        if hashes.len() > 1 {
            r.violations.push(format!("finalized chains diverge at height {h}"));
            break;
        }
    }
    // Convergence: heads within one block.
    let heads: Vec<u64> = honest.iter().map(|i| sim.node(*i).chain.head().height).collect();
    if let (Some(lo), Some(hi)) = (heads.iter().min(), heads.iter().max())
        && hi - lo > 2
    {
        r.violations.push(format!("beacon heads spread over {lo}..{hi}"));
    }

    // 2. Identical anchors, members and ranks per (zone, slot).
    let fin_slot = r.finalized_slot;
    for e in fin_slot.saturating_sub(6)..=fin_slot {
        let views: BTreeSet<_> = honest
            .iter()
            .filter_map(|i| {
                let (h, st) = sim.node(*i).chain.anchor(e)?;
                let ranks: Vec<_> = st.zones.keys().map(|z| umis_protocol::ranks::zone_ranks(&st, *z, e * 4)).collect();
                Some((h, ranks))
            })
            .collect();
        if views.len() > 1 {
            r.violations.push(format!("anchors/ranks differ for beacon slot {e}"));
        }
    }

    // 3. Registry agreement, unique locations.
    let regs: BTreeSet<Vec<(u32, [i32; 3], String, usize)>> = honest
        .iter()
        .map(|i| {
            let st = sim.node(*i).chain.finalized().state.clone().expect("finalized state");
            st.zones.values().map(|z| (z.id, z.point, z.layer.clone(), z.inputs.len())).collect()
        })
        .collect();
    if regs.len() > 1 {
        r.violations.push("zone registries differ at the finalized block".into());
    }
    let mut locs = BTreeSet::new();
    for z in head.zones.values() {
        if !locs.insert((z.point, z.layer.clone())) {
            r.violations.push(format!("two zones at {:?}/{}", z.point, z.layer));
        }
    }

    // 4. Commitments: contiguous, aligned to physical time, leaves verified.
    let mut inclusions: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    for e in a.chain.canonical() {
        let Some(b) = &e.block else { continue };
        for zc in &b.body.zone_commitments {
            inclusions.entry(zc.commitment.zone).or_default().push(b.slot());
        }
    }
    for (z, list) in &head.commits {
        let zone = &head.zones[z];
        let t = sim.ctx.timing(zone.slot_ticks);
        let valid: Vec<_> = list.iter().filter(|c| c.status != CommitStatus::Invalid).collect();
        r.invalidated += list.iter().filter(|c| c.status == CommitStatus::Invalid).count();
        for w in valid.windows(2) {
            if w[0].to_slot != w[1].from_slot || w[0].state_root != w[1].prev_state_root {
                r.violations.push(format!("zone {z}: commitments {} and {} do not link", w[0].e, w[1].e));
            }
        }
        for c in &valid {
            if c.to_slot * zone.slot_ticks != (c.e + 1) * sim.ctx.b() || t.j_star(c.e) != c.to_slot {
                r.violations.push(format!("zone {z}: commitment {} not at a physical-time boundary", c.e));
            }
        }
        r.commits.insert(*z, inclusions.get(z).map(|v| v.len()).unwrap_or(0));
        // liveness: gaps between inclusions inside the live window
        let mut incl: Vec<u64> = inclusions.get(z).cloned().unwrap_or_default();
        incl.retain(|s| *s >= live_from.max(zone.active_from) && *s <= live_until);
        let start = live_from.max(zone.active_from + 2);
        if live_until > start + max_gap {
            let mut prev = start;
            let mut gap = 0;
            for s in incl.iter().chain(std::iter::once(&live_until)) {
                gap = gap.max(s.saturating_sub(prev));
                prev = *s;
            }
            r.max_commit_gap.insert(*z, gap);
            if gap > max_gap {
                r.violations.push(format!("zone {z}: no commitment included for {gap} beacon slots"));
            }
        }
    }

    // 5. Replay the archive node's committed zone chains.
    for (z, eng) in &a.engines {
        let zone = &head.zones[z];
        let Ok(inst) = sim.ctx.instance(&zone.task_id, &zone.params, &zone.params_hash) else { continue };
        for c in head.commits[z].iter().filter(|c| c.status != CommitStatus::Invalid) {
            for bs in &c.blocks {
                let Some(entry) = eng.get(&bs.hash) else { continue };
                let Some(block) = &entry.block else { continue };
                let Some(parent) = eng.get(&entry.parent) else { continue };
                let Some(pstate) = &parent.state else { continue };
                match exec::reexecute_block(inst.as_ref(), pstate, block) {
                    Ok(ex) if exec::matches_header(&ex, block.header()) => {}
                    _ => r.violations.push(format!("zone {z}: committed block at slot {} does not replay", bs.slot)),
                }
                if let Some(st) = &entry.state
                    && bmt::file_address(st) != bs.state_root
                {
                    r.violations.push(format!("zone {z}: stored state of slot {} has a wrong root", bs.slot));
                }
            }
        }
    }

    // 6. Slashing: honest nodes never; injected faults exactly once.
    r.slashes = head.slash_log.len();
    r.rollbacks = head.rollback_log.len();
    let mut seen: BTreeMap<(umis_core::NodeId, u32, u64), usize> = BTreeMap::new();
    for s in &head.slash_log {
        *seen.entry((s.minter, s.zone, s.zone_slot)).or_default() += 1;
        if let Some(i) = sim.index_of(&s.minter)
            && sim.honest[i]
        {
            r.violations.push(format!("honest node {} slashed", sim.net.nodes[i].label));
        }
    }
    for (k, n) in &seen {
        if *n > 1 {
            r.violations.push(format!("fault {:?} slashed {n} times", k.2));
        }
    }
    for k in &sim.injected {
        if !seen.contains_key(k) {
            r.violations.push(format!("injected fault at zone {} slot {} not slashed", k.1, k.2));
        }
    }

    // 7. Rollback reach.
    let bound = sim.ctx.p.proof_lag + sim.ctx.p.anchor_depth + 1 + 1;
    for rb in &head.rollback_log {
        let from = rb.checkpoint_e.map(|e| e + 1).unwrap_or(head.zones[&rb.zone].active_from);
        if rb.e_record.saturating_sub(from) > bound + 2 {
            r.violations.push(format!(
                "zone {}: {:?} rollback at {} reaches back to {} (bound {})",
                rb.zone, rb.reason, rb.e_record, from, bound
            ));
        }
    }

    // 8. Zones active only after every adjacent zone accepted with ≥ q votes.
    for p in head.proposals.values() {
        if let ProposalStatus::Accepted { zone, .. } = p.status {
            for adj in &p.adjacent {
                let t = p.tally.get(adj);
                let n = p.voters[adj].len() as u64;
                let need = (n * sim.ctx.p.vote_num).div_ceil(sim.ctx.p.vote_den);
                if t.is_none_or(|t| (t.yes.len() as u64) < need) {
                    r.violations.push(format!("zone {zone} activated without q votes of zone {adj}"));
                }
            }
        }
    }

    // staleness
    let (sum, n): (u64, u64) = honest
        .iter()
        .map(|i| (sim.node(*i).stats.input_staleness_sum, sim.node(*i).stats.input_staleness_n))
        .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    r.avg_staleness_ms = if n > 0 { sum as f64 / n as f64 } else { 0.0 };
    r
}
