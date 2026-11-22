//! The life2d proof backend with recursive aggregation (ROADMAP A4, step 1; native only).
//!
//! A proof commitment's advancing slots (two or more) are proved as digest leaves
//! ([`LifeDigestAir`](task_life2d::air::digest::LifeDigestAir)) and folded into one recursive
//! aggregate. The first member's proof is `ProofTag::AggHead ‖ m ‖ member statement addresses ‖
//! postcard aggregate`, every other member's `ProofTag::AggRef ‖ keccak(head) ‖ index`
//! (INV-LIFE-AGG-1). Single advancing slots, hold and birth slots, and shapes the digest AIR
//! does not support keep the plain [`Life2dStark`] proofs.
//!
//! The verifier runs every native check of every member ([`Life2dStark::native_claim`]),
//! derives each member's statement `[d_prev, d_halo, d_next, absent]` from the public data,
//! and verifies the aggregate against their concatenation with its own verifier for the
//! zone shape and member count (INV-LIFE-AGG-2). That verifier comes from an owner tree the
//! node builds once per (shape, member count) from dummy leaves; owners are reusable, so
//! proofs made with another node's tree verify (INV-LIFE-AGG-3).

use crate::{AggOwner, AggParams, Layer1Owner, Recursive, config, digest_leaf_of};
use p3_circuit_prover::BatchStarkProof;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use task_life2d::air::digest::{DigestShape, public_values};
use task_life2d::air::{Life2dStark, ProofTag, Shape, group_head, group_ref, parse_group_head};
use task_life2d::board::Board;
use umis_core::records::TransitionStatement;
use umis_core::{Hash, NodeId, SecretKey, SegRecord};
use umis_proof::{ProofBackend, ProofError, PublicData, Witness};

/// Most members one aggregate may have (its tree has depth ≤ 5).
pub const MAX_AGG: usize = 32;

/// One node of an owner tree: a representative recursive proof over `size` leaves and, for
/// `size ≥ 2`, the 2-to-1 owner that combines a `⌈size/2⌉` and a `⌊size/2⌋` subtree.
pub(crate) struct Node {
    pub(crate) rep: Recursive,
    owner: Option<AggOwner>,
}

/// The 2-to-1 owners above one layer-1 owner, every subtree size built on first use. Any leaf
/// kind: only the representative layer-1 proof matters.
pub(crate) struct AggTree {
    p: AggParams,
    nodes: Mutex<HashMap<usize, Arc<Node>>>,
}

impl AggTree {
    pub(crate) fn new(p: AggParams, rep1: Recursive) -> Self {
        let nodes = HashMap::from([(1, Arc::new(Node { rep: rep1, owner: None }))]);
        Self { p, nodes: Mutex::new(nodes) }
    }

    /// The node of `size` leaves, built (with its subtrees) on first use.
    pub(crate) fn node(&self, size: usize) -> Result<Arc<Node>, String> {
        if let Some(n) = self.nodes.lock().expect("nodes").get(&size) {
            return Ok(n.clone());
        }
        let (l, r) = (self.node(size.div_ceil(2))?, self.node(size / 2)?);
        let owner = AggOwner::new(&self.p, &l.rep, &r.rep)?;
        let rep = owner.prove(&l.rep, &r.rep)?;
        let n = Arc::new(Node { rep, owner: Some(owner) });
        self.nodes.lock().expect("nodes").insert(size, n.clone());
        Ok(n)
    }

    /// Folds layer-1 proofs into one aggregate with the same tree shape as [`AggTree::node`].
    pub(crate) fn fold(&self, parts: &[Recursive]) -> Result<Recursive, String> {
        match parts.len() {
            0 => Err("nothing to aggregate".into()),
            1 => Err("a single leaf is not aggregated".into()),
            n => {
                let mid = n.div_ceil(2);
                let one = |xs: &[Recursive]| if xs.len() == 1 { Ok(None) } else { self.fold(xs).map(Some) };
                let (l, r) = (one(&parts[..mid])?, one(&parts[mid..])?);
                let node = self.node(n)?;
                let owner = node.owner.as_ref().expect("size ≥ 2");
                owner.prove(l.as_ref().unwrap_or(&parts[0]), r.as_ref().unwrap_or(&parts[mid]))
            }
        }
    }
}

/// The owners of one zone shape: layer 1 and the 2-to-1 tree above it.
struct Tree {
    l1: Layer1Owner<'static>,
    agg: AggTree,
}

impl Tree {
    fn new(p: AggParams, shape: Shape) -> Result<Self, String> {
        let cfg = config(&p);
        let e = shape.n + 2 * shape.k;
        // A dead extended board fixes the allocation shape; one leaked leaf per zone shape.
        let rep: &'static _ = Box::leak(Box::new(digest_leaf_of(&cfg, shape, &Board::dead(e, e), &[false; 8])));
        let l1 = Layer1Owner::new(&p, rep)?;
        let leaf = l1.prove(rep)?;
        Ok(Self { l1, agg: AggTree::new(p, leaf) })
    }
}

/// Group proof format shared by the aggregating backends (life2d's tags; they do not collide
/// with any task's plain proof tags).
pub(crate) fn group_proofs(members: &[Hash], body: &[u8]) -> Vec<Vec<u8>> {
    let head = group_head(ProofTag::AggHead, members, body);
    let h = umis_core::keccak256(&head);
    (0..members.len()).map(|k| if k == 0 { head.clone() } else { group_ref(ProofTag::AggRef, &h, k) }).collect()
}

/// The member indices in `items` of the aggregate whose head is `items[head]`
/// (INV-LIFE-AGG-1), and its proof body.
pub(crate) fn group_members<'a>(
    items: &[(&TransitionStatement, PublicData<'_>, &'a [u8])],
    head: usize,
) -> Result<(Vec<usize>, &'a [u8]), ProofError> {
    let bad = |m: &str| Err(ProofError::Invalid(m.into()));
    let (members, body) = parse_group_head(ProofTag::AggHead, MAX_AGG, items[head].2)?;
    if items[head].0.address() != members[0] {
        return bad("aggregate head is not its own first member");
    }
    let head_hash = umis_core::keccak256(items[head].2);
    let mut idx = vec![head];
    for (k, m) in members.iter().enumerate().skip(1) {
        let want = group_ref(ProofTag::AggRef, &head_hash, k);
        match items.iter().position(|(s, _, p)| s.address() == *m && *p == want.as_slice()) {
            Some(i) => idx.push(i),
            None => return bad("aggregate member missing from the group"),
        }
    }
    Ok((idx, body))
}

/// [`ProofBackend::verify_group`] of an aggregating backend: aggregate heads through `agg`
/// (which returns the member indices it verified), everything else through `inner`.
pub(crate) fn verify_group_with(
    items: &[(&TransitionStatement, PublicData<'_>, &[u8])],
    agg: impl Fn(usize) -> Result<Vec<usize>, ProofError>,
    inner: &dyn ProofBackend,
) -> Vec<Result<NodeId, ProofError>> {
    let tag = |p: &[u8]| p.first().copied();
    let mut out: Vec<Option<Result<NodeId, ProofError>>> = (0..items.len()).map(|_| None).collect();
    for (i, (_, _, p)) in items.iter().enumerate() {
        if tag(p) == Some(ProofTag::AggHead as u8) {
            match agg(i) {
                Ok(idx) => idx.into_iter().for_each(|j| out[j] = Some(Ok(NodeId::default()))),
                Err(e) => out[i] = Some(Err(e)),
            }
        }
    }
    // Everything else (plain proofs, batch groups, native slots) goes to the inner backend.
    let rest: Vec<usize> =
        (0..items.len()).filter(|&i| out[i].is_none() && tag(items[i].2) != Some(ProofTag::AggRef as u8)).collect();
    let sub: Vec<_> = rest.iter().map(|&i| (items[i].0, items[i].1, items[i].2)).collect();
    for (&i, r) in rest.iter().zip(inner.verify_group(&sub)) {
        out[i] = Some(r);
    }
    out.into_iter()
        .map(|r| r.unwrap_or_else(|| Err(ProofError::Invalid("aggregate reference without a verified head".into()))))
        .collect()
}

/// The life2d backend that aggregates a commitment's advancing slots.
pub struct LifeAggBackend {
    inner: Life2dStark,
    p: AggParams,
    trees: Mutex<HashMap<Shape, Arc<Tree>>>,
}

impl LifeAggBackend {
    /// `inner` proves and checks everything that is not aggregated; `p` are the aggregation
    /// FRI parameters.
    pub fn new(inner: Life2dStark, p: AggParams) -> Self {
        Self { inner, p, trees: Mutex::new(HashMap::new()) }
    }

    /// From a genesis `stark_config_id`: `koalabear-agg` (P100 leaves, J101 aggregates) and
    /// `koalabear-agg-test` (test parameters for both).
    pub fn from_config_id(id: &str) -> Result<Self, String> {
        match id {
            "koalabear-agg" => Ok(Self::new(Life2dStark::from_config_id(Some("koalabear"))?, AggParams::J101)),
            "koalabear-agg-test" => {
                Ok(Self::new(Life2dStark::from_config_id(Some("koalabear-test"))?, AggParams::TEST))
            }
            other => Err(format!("unknown aggregation config {other}")),
        }
    }

    fn tree(&self, shape: Shape) -> Result<Arc<Tree>, String> {
        // Built outside the map lock is not worth it: a tree is built once per shape.
        let mut t = self.trees.lock().expect("trees");
        if let Some(tree) = t.get(&shape) {
            return Ok(tree.clone());
        }
        let tree = Arc::new(Tree::new(self.p, shape)?);
        t.insert(shape, tree.clone());
        Ok(tree)
    }

    /// The aggregate over the advancing claims `(E, absent)` of one shape.
    fn aggregate(&self, shape: Shape, claims: &[(Board, [bool; 8])]) -> Result<Vec<u8>, String> {
        let tree = self.tree(shape)?;
        let cfg = config(&self.p);
        let parts: Vec<Recursive> = claims
            .iter()
            .map(|(ext, absent)| tree.l1.prove(&digest_leaf_of(&cfg, shape, ext, absent)))
            .collect::<Result<_, _>>()?;
        let top = tree.agg.fold(&parts)?;
        postcard::to_allocvec(&top.out.0).map_err(|e| e.to_string())
    }

    /// Verifies an aggregate head and its references in `items` (INV-LIFE-AGG-1, -2); returns
    /// the item indices it covers.
    fn verify_agg_group(
        &self,
        items: &[(&TransitionStatement, PublicData<'_>, &[u8])],
        head: usize,
    ) -> Result<Vec<usize>, ProofError> {
        let bad = |m: &str| Err(ProofError::Invalid(m.into()));
        let (idx, body) = group_members(items, head)?;
        let mut statement = Vec::new();
        let mut shape = None;
        for &i in &idx {
            let (s, pd, p) = items[i];
            let Some(c) = self.inner.native_claim(s, &pd, p)? else {
                return bad("aggregate member is not an advancing slot");
            };
            if *shape.get_or_insert(c.shape) != c.shape || !DigestShape::supports(&c.shape) {
                return bad("aggregate members must share one supported shape");
            }
            statement.extend(public_values(&DigestShape::new(c.shape), &c.ext, &c.absent, &c.x_out));
        }
        let shape = shape.expect("≥ 2 members");
        let proof: BatchStarkProof<crate::AggConfig> =
            postcard::from_bytes(body).map_err(|e| ProofError::Malformed(e.to_string()))?;
        let tree = self.tree(shape).map_err(ProofError::Invalid)?;
        let node = tree.agg.node(idx.len()).map_err(ProofError::Invalid)?;
        node.rep.verifier.verify(&proof, &statement).map_err(|e| ProofError::Invalid(format!("aggregate: {e:?}")))?;
        Ok(idx)
    }
}

impl ProofBackend for LifeAggBackend {
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
        task_id == task_life2d::TASK_ID
    }

    fn prove_batch(
        &self,
        items: &[(&TransitionStatement, Witness<'_>)],
        key: &SecretKey,
    ) -> Result<Vec<Vec<u8>>, ProofError> {
        let mut out: Vec<Option<Vec<u8>>> = vec![None; items.len()];
        let mut adv: Vec<(usize, Shape, Board, [bool; 8])> = Vec::new();
        for (i, (stmt, w)) in items.iter().enumerate() {
            match self.inner.advance_claim(stmt, w)? {
                Some((shape, ext, absent)) => adv.push((i, shape, ext, absent)),
                None => out[i] = Some(vec![ProofTag::Native as u8]),
            }
        }
        let aggregable = adv.len() >= 2 && adv.iter().all(|a| a.1 == adv[0].1) && DigestShape::supports(&adv[0].1);
        if !aggregable {
            // The inner backend's batch STARK (or single proofs) for everything.
            return self.inner.prove_batch(items, key);
        } else {
            for part in adv.chunks(MAX_AGG) {
                if part.len() == 1 {
                    out[part[0].0] = Some(self.inner.prove(items[part[0].0].0, &items[part[0].0].1, key)?);
                    continue;
                }
                let claims: Vec<(Board, [bool; 8])> = part.iter().map(|a| (a.2.clone(), a.3)).collect();
                let body = self.aggregate(part[0].1, &claims).map_err(ProofError::Prover)?;
                let members: Vec<Hash> = part.iter().map(|a| items[a.0].0.address()).collect();
                for (a, proof) in part.iter().zip(group_proofs(&members, &body)) {
                    out[a.0] = Some(proof);
                }
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
    use task_life2d::Life2dParams;

    fn shape() -> Shape {
        let mut p = Life2dParams::default_for([1, 1, 0], 4);
        (p.n, p.gens, p.strips, p.rows_per_trace_row) = (16, 2, 1, 2);
        Shape::of(&p)
    }

    fn ext(salt: u64, e: usize) -> Board {
        let mut b = Board::dead(e, e);
        for (i, c) in b.cells.iter_mut().enumerate() {
            *c = ((i as u64 ^ salt).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 6) as u8;
        }
        b
    }

    /// INV-LIFE-AGG-3: an aggregate made with one node's owner tree verifies with another
    /// node's independently built tree, for 2, 3 and 5 members; a wrong statement does not.
    #[test]
    fn aggregates_verify_across_independently_built_trees() {
        let s = shape();
        let d = DigestShape::new(s);
        let e = s.n + 2 * s.k;
        let (a, b) = (
            LifeAggBackend::from_config_id("koalabear-agg-test").unwrap(),
            LifeAggBackend::from_config_id("koalabear-agg-test").unwrap(),
        );
        for m in [2usize, 3, 5] {
            let claims: Vec<(Board, [bool; 8])> =
                (0..m as u64).map(|i| (ext(i + 7 * m as u64, e), [false; 8])).collect();
            let body = a.aggregate(s, &claims).unwrap();
            let proof: BatchStarkProof<crate::AggConfig> = postcard::from_bytes(&body).unwrap();
            let statement: Vec<_> = claims
                .iter()
                .flat_map(|(x, ab)| {
                    let (_, out) = task_life2d::air::generate_trace(&s, x, ab);
                    public_values(&d, x, ab, &out)
                })
                .collect();
            let vb = b.tree(s).unwrap().agg.node(m).unwrap();
            vb.rep.verifier.verify(&proof, &statement).unwrap_or_else(|e| panic!("m = {m}: {e:?}"));
            let mut wrong = statement.clone();
            wrong.swap(0, task_life2d::air::digest::N_PUBLIC);
            assert!(vb.rep.verifier.verify(&proof, &wrong).is_err(), "m = {m}");
            // A different member count's verifier rejects it.
            let other = b.tree(s).unwrap().agg.node(m + 1).unwrap();
            assert!(other.rep.verifier.verify(&proof, &statement).is_err(), "m = {m}");
        }
    }
}
