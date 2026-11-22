//! Multi-slot proofs on real zone slots (INV-STENCIL-TASK-3), for heat and the lattice
//! Boltzmann fluid: a commitment's advancing slots share one batch proof that another node
//! verifies; forged groups are rejected by the release-mode `verify_group`.
use std::collections::BTreeMap;
use std::sync::Arc;
use task_stencil::air::{MAX_MULTI, StencilStark};
use task_stencil::devnet::StencilOptions;
use task_stencil::{RuleId, Stencil2d, TASK_ID};
use umis_core::records::TransitionStatement;
use umis_core::{Hash, NodeId, SecretKey, bmt};
use umis_proof::group;
use umis_proof::{ProofBackend, PublicData, Witness};
use umis_stark::config::FriPreset;
use umis_task::{Bytes, DynZoneTask, PortId, TaskAdapter, ZoneInstance, ZoneTask};

/// One isolated zone (no neighbours: every halo region absent) run for `slots` slots.
struct Run {
    params: Vec<u8>,
    seed: Hash,
    inst: Arc<dyn ZoneInstance>,
    states: Vec<Bytes>,
    stmts: Vec<TransitionStatement>,
    inputs: BTreeMap<PortId, Bytes>,
}

fn run(rule: RuleId, slots: u64, salt: &[u8]) -> Run {
    let seed = umis_core::keccak256(salt);
    let mut o = StencilOptions::new(seed, 0, NodeId::default(), rule);
    (o.n, o.gens, o.region, o.rows_per_trace_row) = (8, 1, [1, 1], 2);
    let pt = o.genesis_points()[0];
    let p = o.zone_params(pt, 4);
    let params = borsh::to_vec(&p).unwrap();
    let inst = TaskAdapter::<Stencil2d>::new().instantiate(&params, &seed).unwrap();
    let ctx = Stencil2d::prepare(&p, &seed).unwrap();
    let inputs = BTreeMap::new();
    let mut states = vec![inst.initial_state(0)];
    let mut stmts = Vec::new();
    for slot in 0..slots {
        let next = inst.step(&states[slot as usize], &inputs, slot).unwrap();
        let s = task_stencil::decode_state(&p, &next).unwrap();
        let outs = <Stencil2d as ZoneTask>::outputs(&ctx, &s);
        let leaves: Vec<Hash> = outs.values().map(|b| bmt::file_address(b)).collect();
        stmts.push(TransitionStatement {
            zone: 0,
            from_slot: slot,
            to_slot: slot + 1,
            params_hash: umis_task::params_hash(TASK_ID, task_stencil::LAYER, &params),
            prev_state_root: bmt::file_address(&states[slot as usize]),
            state_root: bmt::file_address(&next),
            inputs_commitment: umis_core::records::list_root_of::<umis_core::records::InputRef>(&[]),
            outputs_root: bmt::list_root(&leaves),
        });
        states.push(next);
    }
    Run { params, seed, inst, states, stmts, inputs }
}

impl Run {
    fn prove(&self, b: &StencilStark, range: std::ops::Range<usize>) -> Vec<Bytes> {
        let items: Vec<_> = range
            .map(|i| {
                (
                    &self.stmts[i],
                    Witness {
                        instance: self.inst.as_ref(),
                        params: &self.params,
                        genesis_seed: self.seed,
                        prev_state: &self.states[i],
                        inputs: &self.inputs,
                    },
                )
            })
            .collect();
        b.prove_batch(&items, &SecretKey::from_label("p")).unwrap()
    }

    fn verify(&self, b: &StencilStark, from: usize, proofs: &[Bytes]) -> Vec<bool> {
        let items: Vec<_> = proofs
            .iter()
            .enumerate()
            .map(|(j, p)| {
                let i = from + j;
                let pd = PublicData {
                    task_id: Some(TASK_ID),
                    params: Some(&self.params),
                    genesis_seed: Some(self.seed),
                    prev_state: Some(&self.states[i]),
                    next_state: Some(&self.states[i + 1]),
                    inputs: Some(&self.inputs),
                    input_refs: Some(&[]),
                    prev_witness: None,
                    next_witness: None,
                };
                (&self.stmts[i], pd, p.as_slice())
            })
            .collect();
        b.verify_group(&items).into_iter().map(|r| r.is_ok()).collect()
    }
}

fn backend() -> StencilStark {
    StencilStark::new(FriPreset::Test)
}

const HEAD: u8 = 2;
const REF: u8 = 3;

const RULES: [RuleId; 2] = [RuleId::Heat { a: 3, s: 4, wall: 1000 }, RuleId::Lbm { a: 30, t: 4 }];

#[test]
fn a_commitment_shares_one_proof_and_verifies_on_another_node() {
    for rule in RULES {
        one_commitment(rule);
    }
}

fn one_commitment(rule: RuleId) {
    let r = run(rule, 4, b"agg-backend");
    let (prover, verifier) = (backend(), backend());
    let proofs = r.prove(&prover, 0..4);
    assert_eq!(proofs[0][0], HEAD, "advancing slots share one proof");
    assert!(proofs[1..].iter().all(|p| p[0] == REF));
    assert_eq!(r.verify(&verifier, 0, &proofs), vec![true; 4]);
    // A reference alone, or the head without its references, does not verify.
    assert_eq!(r.verify(&verifier, 1, &proofs[1..2]), vec![false]);
    assert_eq!(r.verify(&verifier, 0, &proofs[..1]), vec![false]);
}

#[test]
fn forged_multi_slot_groups_are_rejected() {
    let r = run(RULES[0], 8, b"multi-forge");
    let b = backend();
    let honest = r.prove(&b, 0..4);
    let later = r.prove(&b, 4..8);
    let relabel = |members: &[Hash], body: &[u8]| group::proofs(HEAD, REF, members, body);
    // The proof of slots 4..8 under slots 0..4's member list: it does not match the statements
    // derived from slots 0..4.
    let (members, body) = group::parse_head(HEAD, MAX_MULTI, &honest[0]).unwrap();
    let (_, other_body) = group::parse_head(HEAD, MAX_MULTI, &later[0]).unwrap();
    assert_eq!(r.verify(&b, 0, &relabel(&members, other_body)), vec![false; 4]);
    // Members listed in another order: slot j's statement no longer matches cells AIR j.
    let mut swapped = members.clone();
    swapped.swap(1, 2);
    assert!(r.verify(&b, 0, &relabel(&swapped, body)).iter().all(|ok| !ok));
    // Sanity: the honest groups verify.
    assert_eq!(r.verify(&b, 0, &honest), vec![true; 4]);
    assert_eq!(r.verify(&b, 4, &later), vec![true; 4]);
}

#[test]
fn a_single_advancing_slot_keeps_its_plain_proof() {
    let r = run(RULES[0], 1, b"agg-single");
    let b = backend();
    let proofs = r.prove(&b, 0..1);
    assert_eq!(proofs[0][0], 1, "plain stencil STARK");
    let _ = MAX_MULTI;
    assert_eq!(r.verify(&b, 0, &proofs), vec![true]);
}
