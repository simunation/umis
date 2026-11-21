//! The aggregation backend on real life2d slots (INV-LIFE-AGG-1..3): a commitment's
//! advancing slots aggregate into one head proof that another node verifies with its own
//! owner tree; forged groups are rejected by `verify_group`.
use std::collections::BTreeMap;
use std::sync::Arc;
use task_life2d::air::{ProofTag, group_head, group_ref, parse_group_head};
use task_life2d::devnet::LifeOptions;
use task_life2d::{Life2d, TASK_ID};
use umis_agg::backend::{LifeAggBackend, MAX_AGG};
use umis_core::records::TransitionStatement;
use umis_core::{Hash, NodeId, SecretKey, bmt};
use umis_proof::{ProofBackend, PublicData, Witness};
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

fn run(slots: u64, salt: &[u8]) -> Run {
    let seed = umis_core::keccak256(salt);
    let mut o = LifeOptions::new(seed, 0, NodeId::default());
    (o.n, o.gens, o.region, o.density_pct, o.strips, o.rows_per_trace_row) = (16, 2, [1, 1], 35, 1, 2);
    let pt = o.genesis_points()[0];
    let p = o.zone_params(pt, 4);
    let params = borsh::to_vec(&p).unwrap();
    let inst = TaskAdapter::<Life2d>::new().instantiate(&params, &seed).unwrap();
    let ctx = Life2d::prepare(&p, &seed).unwrap();
    let inputs = BTreeMap::new();
    let mut states = vec![inst.initial_state(0)];
    let mut stmts = Vec::new();
    for slot in 0..slots {
        let next = inst.step(&states[slot as usize], &inputs, slot).unwrap();
        let s = task_life2d::decode_state(&p, &next).unwrap();
        let outs = <Life2d as ZoneTask>::outputs(&ctx, &s);
        let leaves: Vec<Hash> = outs.values().map(|b| bmt::file_address(b)).collect();
        stmts.push(TransitionStatement {
            zone: 0,
            from_slot: slot,
            to_slot: slot + 1,
            params_hash: umis_task::params_hash(TASK_ID, task_life2d::LAYER, &params),
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
    fn prove(&self, b: &LifeAggBackend, range: std::ops::Range<usize>) -> Vec<Bytes> {
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

    fn verify(&self, b: &LifeAggBackend, from: usize, proofs: &[Bytes]) -> Vec<bool> {
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
                };
                (&self.stmts[i], pd, p.as_slice())
            })
            .collect();
        b.verify_group(&items).into_iter().map(|r| r.is_ok()).collect()
    }
}

fn backend() -> LifeAggBackend {
    LifeAggBackend::from_config_id("koalabear-agg-test").unwrap()
}

#[test]
fn a_commitment_aggregates_and_verifies_on_another_node() {
    let r = run(4, b"agg-backend");
    let (prover, verifier) = (backend(), backend());
    let proofs = r.prove(&prover, 0..4);
    assert_eq!(proofs[0][0], ProofTag::AggHead as u8, "advancing slots aggregate");
    assert!(proofs[1..].iter().all(|p| p[0] == ProofTag::AggRef as u8));
    assert_eq!(r.verify(&verifier, 0, &proofs), vec![true; 4]);
    // A reference alone, or the head without its references, does not verify.
    assert_eq!(r.verify(&verifier, 1, &proofs[1..2]), vec![false]);
    assert_eq!(r.verify(&verifier, 0, &proofs[..1]), vec![false]);
}

#[test]
fn forged_aggregate_groups_are_rejected() {
    let r = run(8, b"agg-forge");
    let b = backend();
    let honest = r.prove(&b, 0..4);
    let later = r.prove(&b, 4..8);
    // The aggregate of slots 4..8 under slots 0..4's head (same member list): its proof does
    // not match the statements derived from slots 0..4.
    let (members, _) = parse_group_head(ProofTag::AggHead, MAX_AGG, &honest[0]).unwrap();
    let (_, other_body) = parse_group_head(ProofTag::AggHead, MAX_AGG, &later[0]).unwrap();
    let head = group_head(ProofTag::AggHead, &members, other_body);
    let h = umis_core::keccak256(&head);
    let forged: Vec<Bytes> =
        (0..4).map(|k| if k == 0 { head.clone() } else { group_ref(ProofTag::AggRef, &h, k) }).collect();
    assert_eq!(r.verify(&b, 0, &forged), vec![false; 4]);
    // Members listed in another order: the references no longer match.
    let mut swapped = members.clone();
    swapped.swap(1, 2);
    let (_, body) = parse_group_head(ProofTag::AggHead, MAX_AGG, &honest[0]).unwrap();
    let head = group_head(ProofTag::AggHead, &swapped, body);
    let h = umis_core::keccak256(&head);
    let reordered: Vec<Bytes> =
        (0..4).map(|k| if k == 0 { head.clone() } else { group_ref(ProofTag::AggRef, &h, k) }).collect();
    assert!(r.verify(&b, 0, &reordered).iter().all(|ok| !ok));
    // Sanity: the honest groups verify.
    assert_eq!(r.verify(&b, 0, &honest), vec![true; 4]);
    assert_eq!(r.verify(&b, 4, &later), vec![true; 4]);
}
