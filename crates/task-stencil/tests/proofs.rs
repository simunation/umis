//! The stencil proof backend on real zone slots with neighbours: honest proofs verify for every
//! rule; a false next board (all roots recomputed, so every native check passes) is rejected by
//! the **release-mode verifier**; hold slots are native only (INV-STENCIL-TASK-1/2). The same
//! from state witnesses only (the stateless beacon, INV-STENCIL-TASK-4).

use std::collections::BTreeMap;
use std::sync::Arc;
use task_stencil::air::StencilStark;
use task_stencil::devnet::{StencilOptions, default_rule};
use task_stencil::{RuleId, Stencil2d, decode_state, encode_state, summary};
use umis_core::records::{InputRef, TransitionStatement};
use umis_core::{Hash, NodeId, SecretKey, bmt};
use umis_proof::{ProofBackend, PublicData, Witness};
use umis_task::topology::opposite_face;
use umis_task::{Bytes, DynZoneTask, PortId, TaskAdapter, ZoneInstance, ZoneTask};

struct Slot {
    params: Vec<u8>,
    seed: Hash,
    inst: Arc<dyn ZoneInstance>,
    prev: Bytes,
    next: Bytes,
    inputs: BTreeMap<PortId, Bytes>,
    refs: Vec<InputRef>,
    stmt: TransitionStatement,
}

/// Runs a 2×2 world for `slots` slots and returns zone (0, 0)'s last slot.
fn slot_of(rule: RuleId, slots: u64, starve: bool) -> Slot {
    let seed = umis_core::keccak256(b"stencil-proofs");
    let mut o = StencilOptions::new(seed, 0, NodeId::default(), rule);
    (o.n, o.gens, o.region, o.rows_per_trace_row) = (8, 1, [2, 2], 2);
    let mut zones: BTreeMap<[i32; 3], (Vec<u8>, Arc<dyn ZoneInstance>, Bytes)> = BTreeMap::new();
    for pt in o.genesis_points() {
        let params = borsh::to_vec(&o.zone_params(pt, 4)).unwrap();
        let inst = TaskAdapter::<Stencil2d>::new().instantiate(&params, &seed).unwrap();
        let s0 = inst.initial_state(0);
        zones.insert(pt, (params, inst, s0));
    }
    let mut last = None;
    for slot in 0..slots {
        let outs: BTreeMap<_, _> = zones.iter().map(|(pt, (_, i, s))| (*pt, i.outputs(s).unwrap())).collect();
        let mut next_states = BTreeMap::new();
        for (pt, (params, inst, s)) in &zones {
            let p = o.zone_params(*pt, 4);
            let mut inputs = BTreeMap::new();
            if slot > 0 && !(starve && *pt == [0, 0, 0] && slot + 1 == slots) {
                for q in task_stencil::PORTS {
                    if let Some(out) = outs.get(&p.neighbour_point(q)) {
                        inputs.insert(q, out[&opposite_face(q)].clone());
                    }
                }
            }
            let next = inst.step(s, &inputs, slot).unwrap();
            if *pt == [0, 0, 0] && slot + 1 == slots {
                last = Some((params.clone(), inst.clone(), s.clone(), next.clone(), inputs, slot));
            }
            next_states.insert(*pt, next);
        }
        for (pt, s) in next_states {
            zones.get_mut(&pt).unwrap().2 = s;
        }
    }
    let (params, inst, prev, next, inputs, slot) = last.unwrap();
    let refs: Vec<InputRef> = inputs
        .iter()
        .map(|(q, b)| InputRef {
            link_id: *q,
            src_zone: 1,
            src_slot: slot.saturating_sub(1),
            src_block_hash: Hash::ZERO,
            src_outputs_root: Hash::ZERO,
            payload_address: bmt::file_address(b),
        })
        .collect();
    let mut s = Slot {
        params: params.clone(),
        seed,
        inst,
        prev,
        next: Vec::new(),
        inputs,
        refs,
        stmt: TransitionStatement {
            zone: 0,
            from_slot: slot,
            to_slot: slot + 1,
            params_hash: umis_task::params_hash(task_stencil::TASK_ID, task_stencil::LAYER, &params),
            prev_state_root: Hash::ZERO,
            state_root: Hash::ZERO,
            inputs_commitment: Hash::ZERO,
            outputs_root: Hash::ZERO,
        },
    };
    s.set_next(next);
    s
}

impl Slot {
    /// Sets the claimed next state and re-derives every statement root, so only the proof can
    /// tell an honest claim from a false one.
    fn set_next(&mut self, next: Bytes) {
        self.stmt.prev_state_root = bmt::file_address(&self.prev);
        self.stmt.state_root = bmt::file_address(&next);
        self.stmt.inputs_commitment = umis_core::records::list_root_of(&self.refs);
        let outs = self.inst.outputs(&next).unwrap();
        let leaves: Vec<Hash> = outs.values().map(|b| bmt::file_address(b)).collect();
        self.stmt.outputs_root = bmt::list_root(&leaves);
        self.next = next;
    }
    fn prove(&self, b: &StencilStark) -> Bytes {
        let w = Witness {
            instance: self.inst.as_ref(),
            params: &self.params,
            genesis_seed: self.seed,
            prev_state: &self.prev,
            inputs: &self.inputs,
        };
        b.prove(&self.stmt, &w, &SecretKey::from_label("p")).unwrap()
    }
    fn verify(&self, b: &StencilStark, proof: &[u8]) -> bool {
        let pd = PublicData {
            task_id: Some(task_stencil::TASK_ID),
            params: Some(&self.params),
            genesis_seed: Some(self.seed),
            prev_state: Some(&self.prev),
            next_state: Some(&self.next),
            inputs: Some(&self.inputs),
            input_refs: Some(&self.refs),
            prev_witness: None,
            next_witness: None,
        };
        b.verify(&self.stmt, &pd, proof).is_ok()
    }
    fn witnesses(&self) -> (Bytes, Bytes) {
        let p: task_stencil::StencilParams = borsh::from_slice(&self.params).unwrap();
        (summary::witness(&p, &self.prev).unwrap(), summary::witness(&p, &self.next).unwrap())
    }
    /// Verification without the states: only the witnesses `wp`, `wn`.
    fn verify_stateless(&self, b: &StencilStark, proof: &[u8], wp: &[u8], wn: &[u8]) -> bool {
        let pd = PublicData {
            task_id: Some(task_stencil::TASK_ID),
            params: Some(&self.params),
            genesis_seed: Some(self.seed),
            prev_state: None,
            next_state: None,
            inputs: Some(&self.inputs),
            input_refs: Some(&self.refs),
            prev_witness: Some(wp),
            next_witness: Some(wn),
        };
        b.verify(&self.stmt, &pd, proof).is_ok()
    }
}

#[test]
fn honest_slots_verify_and_false_boards_do_not() {
    let backend = StencilStark::from_config_id(Some("koalabear-test"));
    for name in ["life", "heat", "lbm", "grayscott"] {
        let rule = default_rule(name).unwrap();
        let mut s = slot_of(rule, 3, false);
        let proof = s.prove(&backend);
        assert_eq!(proof[0], 1, "{name}: an advancing slot carries a STARK");
        assert!(s.verify(&backend, &proof), "{name}: honest slot");
        // Another verifier instance (fresh setup) accepts it too.
        assert!(s.verify(&StencilStark::from_config_id(Some("koalabear-test")), &proof), "{name}");
        // A false board with every root re-derived: only the STARK stands.
        let p: task_stencil::StencilParams = borsh::from_slice(&s.params).unwrap();
        let mut st = decode_state(&p, &s.next).unwrap();
        let c = st.grid.cell_mut(3, 4);
        c[0] = if c[0] == 0 { 1 } else { c[0] - 1 };
        if name == "lbm" {
            c[1] += 1; // keep the mass of an LBM cell
        }
        s.set_next(encode_state(&p, &st));
        assert!(!s.verify(&backend, &proof), "{name}: false board");
    }
}

#[test]
fn hold_slots_are_native_only() {
    // Zone (0, 0) gets no inputs at slot 2 while its genesis neighbours have advanced: it holds.
    let backend = StencilStark::from_config_id(Some("koalabear-test"));
    let s = slot_of(default_rule("lbm").unwrap(), 3, true);
    let p: task_stencil::StencilParams = borsh::from_slice(&s.params).unwrap();
    let (a, b) = (decode_state(&p, &s.prev).unwrap(), decode_state(&p, &s.next).unwrap());
    assert_eq!(a.generation, b.generation, "held");
    let proof = s.prove(&backend);
    assert_eq!(proof, vec![0]);
    assert!(s.verify(&backend, &proof));
    assert!(!s.verify(&backend, &[1, 2, 3]), "a hold slot's proof must be [0]");
    let _ = <Stencil2d as ZoneTask>::TASK_ID;
}

#[test]
fn slots_verify_from_witnesses_and_false_claims_do_not() {
    let backend = StencilStark::from_config_id(Some("koalabear-test"));
    for name in ["life", "heat", "lbm", "grayscott"] {
        let mut s = slot_of(default_rule(name).unwrap(), 3, false);
        let proof = s.prove(&backend);
        let (wp, wn) = s.witnesses();
        assert!(wp.len() < 1024 && wn.len() < 1024, "{name}: witnesses are small ({} B)", wn.len());
        assert!(s.verify_stateless(&backend, &proof, &wp, &wn), "{name}: honest slot");
        // Swapped or repeated witnesses do not open the statement's roots.
        assert!(!s.verify_stateless(&backend, &proof, &wn, &wp), "{name}: swapped");
        assert!(!s.verify_stateless(&backend, &proof, &wp, &wp), "{name}: prev twice");
        // A false board with every root and summary re-derived: only the STARK stands.
        let p: task_stencil::StencilParams = borsh::from_slice(&s.params).unwrap();
        let mut st = decode_state(&p, &s.next).unwrap();
        let c = st.grid.cell_mut(3, 4);
        c[0] = if c[0] == 0 { 1 } else { c[0] - 1 };
        if name == "lbm" {
            c[1] += 1;
        }
        s.set_next(encode_state(&p, &st));
        let (wp, wn) = s.witnesses();
        assert!(!s.verify_stateless(&backend, &proof, &wp, &wn), "{name}: false board");
    }
}

/// A witness whose summary was edited (another generation; the inclusion proof unchanged) does
/// not open the state root (mutation-checked: without the root check it verifies).
#[test]
fn edited_summaries_are_rejected() {
    use umis_core::bmt::ChunkInclusionProof;
    let backend = StencilStark::from_config_id(Some("koalabear-test"));
    let s = slot_of(default_rule("heat").unwrap(), 3, true); // a hold slot: header-only checks
    let p: task_stencil::StencilParams = borsh::from_slice(&s.params).unwrap();
    let proof = s.prove(&backend);
    assert_eq!(proof, vec![0]);
    let (wp, wn) = s.witnesses();
    assert!(s.verify_stateless(&backend, &proof, &wp, &wn), "honest hold");
    // Edit the next summary's next_slot: a header the hold rule would also reject, so edit
    // both summaries' generation the same way instead (hold keeps it equal).
    let edit = |w: &Bytes| -> Bytes {
        let (mut sum, pr): (Vec<u8>, Vec<ChunkInclusionProof>) = borsh::from_slice(w).unwrap();
        let g = u64::from_le_bytes(sum[8..16].try_into().unwrap()) + p.gens as u64;
        sum[8..16].copy_from_slice(&g.to_le_bytes());
        borsh::to_vec(&(sum, pr)).unwrap()
    };
    assert!(!s.verify_stateless(&backend, &proof, &edit(&wp), &edit(&wn)));
}
