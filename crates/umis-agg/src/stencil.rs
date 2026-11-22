//! The stencil proof backend with recursive aggregation (S4; any `umis-stencil` rule; native
//! only).
//!
//! A proof commitment's advancing slots (two or more, one relation) are proved as EF5 batch
//! leaves and folded into one recursive aggregate through [`NativeLayer1`] and the 2-to-1
//! tree: the first member's proof is `AggHead ‖ m ‖ member statement addresses ‖ postcard
//! aggregate`, every other member's `AggRef ‖ keccak(head) ‖ index` (INV-STENCIL-AGG-1, the
//! life2d group format). A single advancing slot and hold or birth slots keep the plain
//! [`StencilStark`] proofs.
//!
//! The verifier runs every native check of every member ([`StencilStark::native_claim`]),
//! derives each member's statement `public_values(E, next) = [d_prev, d_halo, d_next]` itself,
//! and verifies the aggregate against their concatenation with its own verifier for the leaf
//! relation and member count (INV-STENCIL-AGG-2). Owner trees are built once per (relation,
//! member count) from a boundary-filled leaf; proofs made with another node's tree verify
//! (INV-STENCIL-AGG-3).

use crate::backend::{AggTree, MAX_AGG, group_members, group_proofs, verify_group_with};
use crate::native::{NativeLayer1, NativeRelation};
use crate::{AggParams, Recursive};
use p3_circuit_prover::BatchStarkProof;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use task_stencil::air::{LeafKey, StencilStark};
use task_stencil::{AnyRule, TASK_ID};
use umis_core::records::TransitionStatement;
use umis_core::{Hash, NodeId, SecretKey, SegRecord};
use umis_proof::{ProofBackend, ProofError, PublicData, Witness};
use umis_stark::config::FriPreset;
use umis_stencil::batch::{BatchAir, StencilBatch, lookup_budget_for};
use umis_stencil::{Grid, Rule};

/// The owners of one leaf relation.
struct Tree {
    p: AggParams,
    batch: StencilBatch<AnyRule>,
    rel: &'static NativeRelation<BatchAir<AnyRule>>,
    l1: NativeLayer1<'static, BatchAir<AnyRule>>,
    agg: AggTree,
}

impl Tree {
    fn new(p: AggParams, key: LeafKey) -> Result<Self, String> {
        // The preset only picks the batch's own (unused here) P100/test setup.
        let batch = key.batch(FriPreset::Test)?;
        let rel = NativeRelation::new(
            &p,
            batch.airs().to_vec(),
            batch.degrees().to_vec(),
            lookup_budget_for(p.fri.log_blowup),
        )?;
        // One leaked relation per leaf relation: the layer-1 owner borrows it.
        let rel: &'static _ = Box::leak(Box::new(rel));
        let w = batch.shape.we;
        let rep = Self::leaf(&p, &batch, rel, &Grid::filled(w, w, &batch.rule.boundary()))?;
        let l1 = NativeLayer1::new(&p, rel, &rep)?;
        let first = l1.prove(&rep)?;
        Ok(Self { p, batch, rel, l1, agg: AggTree::new(p, first) })
    }

    fn leaf(
        p: &AggParams,
        batch: &StencilBatch<AnyRule>,
        rel: &NativeRelation<BatchAir<AnyRule>>,
        ext: &Grid,
    ) -> Result<crate::native::NativeLeaf, String> {
        let (traces, public, _) = batch.generate(ext);
        rel.prove(p, &traces, batch.publics(&public))
    }

    fn layer1(&self, ext: &Grid) -> Result<Recursive, String> {
        self.l1.prove(&Self::leaf(&self.p, &self.batch, self.rel, ext)?)
    }
}

/// The stencil backend that aggregates a commitment's advancing slots.
pub struct StencilAggBackend {
    inner: StencilStark,
    p: AggParams,
    trees: Mutex<HashMap<LeafKey, Arc<Tree>>>,
}

impl StencilAggBackend {
    /// `inner` proves and checks everything that is not aggregated; `p` are the aggregation
    /// FRI parameters.
    pub fn new(inner: StencilStark, p: AggParams) -> Self {
        Self { inner, p, trees: Mutex::new(HashMap::new()) }
    }

    /// From a genesis `stark_config_id`: `koalabear-agg` (P100 leaves, J101 aggregates) and
    /// `koalabear-agg-test` (test parameters for both).
    pub fn from_config_id(id: &str) -> Result<Self, String> {
        match id {
            "koalabear-agg" => Ok(Self::new(StencilStark::new(FriPreset::P100), AggParams::J101)),
            "koalabear-agg-test" => Ok(Self::new(StencilStark::new(FriPreset::Test), AggParams::TEST)),
            other => Err(format!("unknown aggregation config {other}")),
        }
    }

    fn tree(&self, key: LeafKey) -> Result<Arc<Tree>, String> {
        let mut t = self.trees.lock().expect("trees");
        if let Some(tree) = t.get(&key) {
            return Ok(tree.clone());
        }
        let tree = Arc::new(Tree::new(self.p, key)?);
        t.insert(key, tree.clone());
        Ok(tree)
    }

    /// The aggregate over the advancing extended boards of one relation.
    fn aggregate(&self, key: LeafKey, exts: &[Grid]) -> Result<Vec<u8>, String> {
        let tree = self.tree(key)?;
        let parts: Vec<Recursive> = exts.iter().map(|e| tree.layer1(e)).collect::<Result<_, _>>()?;
        let top = tree.agg.fold(&parts)?;
        postcard::to_allocvec(&top.out.0).map_err(|e| e.to_string())
    }

    /// Verifies an aggregate head and its references (INV-STENCIL-AGG-1, -2); returns the item
    /// indices it covers.
    fn verify_agg_group(
        &self,
        items: &[(&TransitionStatement, PublicData<'_>, &[u8])],
        head: usize,
    ) -> Result<Vec<usize>, ProofError> {
        let bad = |m: &str| Err(ProofError::Invalid(m.into()));
        let (idx, body) = group_members(items, head)?;
        let mut statement = Vec::new();
        let mut key = None;
        for &i in &idx {
            let (s, pd, p) = items[i];
            let Some((k, ext, next)) = self.inner.native_claim(s, &pd, p)? else {
                return bad("aggregate member is not an advancing slot");
            };
            if *key.get_or_insert(k) != k {
                return bad("aggregate members must share one leaf relation");
            }
            statement.push((ext, next));
        }
        let key = key.expect("≥ 2 members");
        let tree = self.tree(key).map_err(ProofError::Invalid)?;
        let statement: Vec<_> = statement
            .iter()
            .flat_map(|(ext, next)| umis_stencil::air::public_values(&tree.batch.shape, ext, next))
            .collect();
        let proof: BatchStarkProof<crate::AggConfig> =
            postcard::from_bytes(body).map_err(|e| ProofError::Malformed(e.to_string()))?;
        let node = tree.agg.node(idx.len()).map_err(ProofError::Invalid)?;
        node.rep.verifier.verify(&proof, &statement).map_err(|e| ProofError::Invalid(format!("aggregate: {e:?}")))?;
        Ok(idx)
    }
}

impl ProofBackend for StencilAggBackend {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn is_async(&self) -> bool {
        true
    }

    fn prove(&self, stmt: &TransitionStatement, w: &Witness<'_>, key: &SecretKey) -> Result<Vec<u8>, ProofError> {
        self.inner.prove(stmt, w, key)
    }

    fn verify(&self, stmt: &TransitionStatement, pd: &PublicData<'_>, proof: &[u8]) -> Result<NodeId, ProofError> {
        self.inner.verify(stmt, pd, proof)
    }

    fn batches(&self, task_id: &str) -> bool {
        task_id == TASK_ID
    }

    fn prove_batch(
        &self,
        items: &[(&TransitionStatement, Witness<'_>)],
        key: &SecretKey,
    ) -> Result<Vec<Vec<u8>>, ProofError> {
        let mut out: Vec<Option<Vec<u8>>> = vec![None; items.len()];
        let mut adv: Vec<(usize, LeafKey, Grid)> = Vec::new();
        for (i, (stmt, w)) in items.iter().enumerate() {
            match self.inner.advance_claim(stmt, w)? {
                Some((k, ext)) => adv.push((i, k, ext)),
                None => out[i] = Some(self.inner.prove(stmt, w, key)?),
            }
        }
        let aggregable = adv.len() >= 2 && adv.iter().all(|a| a.1 == adv[0].1);
        for part in adv.chunks(if aggregable { MAX_AGG } else { 1 }) {
            if part.len() == 1 {
                let (s, w) = &items[part[0].0];
                out[part[0].0] = Some(self.inner.prove(s, w, key)?);
                continue;
            }
            let exts: Vec<Grid> = part.iter().map(|a| a.2.clone()).collect();
            let body = self.aggregate(part[0].1, &exts).map_err(ProofError::Prover)?;
            let members: Vec<Hash> = part.iter().map(|a| items[a.0].0.address()).collect();
            for (a, proof) in part.iter().zip(group_proofs(&members, &body)) {
                out[a.0] = Some(proof);
            }
        }
        Ok(out.into_iter().map(|p| p.expect("every item")).collect())
    }

    fn verify_group(&self, items: &[(&TransitionStatement, PublicData<'_>, &[u8])]) -> Vec<Result<NodeId, ProofError>> {
        verify_group_with(items, |i| self.verify_agg_group(items, i), &self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use task_stencil::RuleId;

    fn key() -> LeafKey {
        LeafKey { rule: RuleId::Heat { a: 3, s: 4, wall: 1000 }, n: 8, k: 1, rpr: 2 }
    }

    fn ext(salt: u32, w: usize) -> Grid {
        let mut g = Grid::filled(w, w, &[1000]);
        for (i, c) in g.data.iter_mut().enumerate() {
            *c = 1000 + ((i as u32 ^ salt).wrapping_mul(2654435761) >> 12) % 40_000;
        }
        g
    }

    /// INV-STENCIL-AGG-3: an aggregate made with one node's owner tree verifies with another
    /// node's independently built tree, for 2 and 3 members; a wrong statement or another
    /// member count's verifier does not.
    #[test]
    fn aggregates_verify_across_independently_built_trees() {
        let (a, b) = (
            StencilAggBackend::from_config_id("koalabear-agg-test").unwrap(),
            StencilAggBackend::from_config_id("koalabear-agg-test").unwrap(),
        );
        let k = key();
        let batch = k.batch(FriPreset::Test).unwrap();
        for m in [2u32, 3] {
            let exts: Vec<Grid> = (0..m).map(|i| ext(i + 7 * m, batch.shape.we)).collect();
            let body = a.aggregate(k, &exts).unwrap();
            let proof: BatchStarkProof<crate::AggConfig> = postcard::from_bytes(&body).unwrap();
            let statement: Vec<_> = exts
                .iter()
                .flat_map(|e| umis_stencil::air::public_values(&batch.shape, e, &e.evolve(&batch.rule, 1)))
                .collect();
            let vb = b.tree(k).unwrap().agg.node(m as usize).unwrap();
            vb.rep.verifier.verify(&proof, &statement).unwrap_or_else(|e| panic!("m = {m}: {e:?}"));
            let mut wrong = statement.clone();
            wrong.swap(0, 24);
            assert!(vb.rep.verifier.verify(&proof, &wrong).is_err(), "m = {m}");
            let other = b.tree(k).unwrap().agg.node(m as usize + 1).unwrap();
            assert!(other.rep.verifier.verify(&proof, &statement).is_err(), "m = {m}");
        }
    }
}
