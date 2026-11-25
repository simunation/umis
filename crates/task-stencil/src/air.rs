//! The STARK proof backend of stencil zones (INV-STENCIL-TASK-*).
//!
//! An advancing slot is proved by the `umis-stencil` batch leaf ([`StencilBatch`]: cells,
//! Poseidon2 hash table, range table). The verifier never takes a claim from the proof:
//!
//! - **native (TASK-1):** both states and the inputs match the statement's roots, states are
//!   canonical, `outputs_root` is the next state's, the decision is recomputed with [`decide`],
//!   and every header field of the next state equals [`apply`]'s; a hold or birth slot is
//!   checked entirely natively (proof `[0]`);
//! - **STARK (TASK-2):** for an advance, the verifier derives the extended board `E` itself and
//!   checks the proof against `statement(E, next.grid)` (`d_prev`, `d_halo`, `d_next`,
//!   `d_faces`), so the proof binds `next.grid = E.evolve(rule, k)` and its border digest.
//!
//! Proof bytes: `0` (native), `1 ‖ postcard(BatchProof)` (one slot), or a **multi-slot group**
//! (INV-STENCIL-TASK-3): a commitment's advancing slots of one zone in one batch proof with one
//! cells AIR per slot and shared hash and range tables (`StencilBatch::new_multi`). The first
//! member carries `2 ‖ m ‖ member addresses ‖ postcard(BatchProof)`, the others
//! `3 ‖ keccak(head) ‖ index` ([`umis_proof::group`]); the verifier derives every member's
//! statement itself and checks the proof against their concatenation.

use crate::{Ctx, Decision, Stencil2d, StencilParams, apply, decide, decode_state, summary};
use p3_batch_stark::BatchProof;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use umis_core::records::TransitionStatement;
use umis_core::{Hash, NodeId, SecretKey, SegRecord, bmt};
use umis_proof::{ProofBackend, ProofError, PublicData, Witness};
use umis_stark::config::{FriPreset, koala_bear};
use umis_stencil::batch::{StencilBatch, statement};
use umis_stencil::{Grid, Rule, Val};
use umis_task::{Bytes, PortId, ZoneTask};

pub const BACKEND_ID: &str = "plonky3";
const NATIVE: u8 = 0;
const STARK: u8 = 1;
const MULTI_HEAD: u8 = 2;
const MULTI_REF: u8 = 3;
/// Most slots in one multi-slot proof (the gain flattens beyond 8; 2026-11-22).
pub const MAX_MULTI: usize = 8;

struct Setup {
    ctx: Ctx,
    params_hash: Hash,
    batch: StencilBatch<crate::AnyRule>,
    /// Multi-slot setups by slot count.
    multi: Mutex<HashMap<usize, Arc<StencilBatch<crate::AnyRule>>>>,
}

impl Setup {
    fn multi(&self, m: usize) -> Result<Arc<StencilBatch<crate::AnyRule>>, String> {
        if let Some(b) = self.multi.lock().expect("multi").get(&m) {
            return Ok(b.clone());
        }
        let p = &self.ctx.p;
        let mut b =
            StencilBatch::new_multi(self.ctx.rule, p.n(), p.k(), p.rows_per_trace_row as usize, self.batch.preset, m)?;
        b.use_gpu_if_available();
        let b = Arc::new(b);
        self.multi.lock().expect("multi").insert(m, b.clone());
        Ok(b)
    }
}

/// The Plonky3 backend of stencil zones (per zone it caches the task context and the
/// batch setup).
pub struct StencilStark {
    preset: FriPreset,
    setups: Mutex<HashMap<(Hash, Hash), Arc<Setup>>>,
}

impl StencilStark {
    pub fn new(preset: FriPreset) -> Self {
        Self { preset, setups: Mutex::new(HashMap::new()) }
    }

    /// From a genesis `stark_config_id`: `*-test` ids use the fast insecure preset, every other
    /// id P100.
    pub fn from_config_id(id: Option<&str>) -> Self {
        Self::new(if id.is_some_and(|i| i.ends_with("-test")) { FriPreset::Test } else { FriPreset::P100 })
    }

    fn setup(&self, params: &[u8], seed: &Hash) -> Result<Arc<Setup>, String> {
        let key = (umis_core::keccak256(params), *seed);
        if let Some(s) = self.setups.lock().expect("setups").get(&key) {
            return Ok(s.clone());
        }
        let p: StencilParams = borsh::from_slice(params).map_err(|e| e.to_string())?;
        Stencil2d::validate_params(&p).map_err(|e| e.to_string())?;
        let ctx = Stencil2d::prepare(&p, seed).map_err(|e| e.to_string())?;
        let mut batch = StencilBatch::new(ctx.rule, p.n(), p.k(), p.rows_per_trace_row as usize, self.preset)?;
        // `cuda` builds with a usable GPU prove there (same proofs; verification stays stock).
        batch.use_gpu_if_available();
        let params_hash = umis_task::params_hash(crate::TASK_ID, crate::LAYER, params);
        let s = Arc::new(Setup { ctx, params_hash, batch, multi: Mutex::new(HashMap::new()) });
        self.setups.lock().expect("setups").insert(key, s.clone());
        Ok(s)
    }

    /// Every native check; for an advancing slot `Some(statement)`, the leaf's public values
    /// `[d_prev, d_halo, d_next, d_faces]` the STARK must prove. From full states (members,
    /// shares) or, without states, from state witnesses (the stateless beacon,
    /// INV-STENCIL-TASK-4, TASK-5).
    fn native(
        &self,
        stmt: &TransitionStatement,
        pd: &PublicData<'_>,
        proof: &[u8],
    ) -> Result<(Arc<Setup>, Option<Vec<Val>>), ProofError> {
        let bad = |m: &str| Err(ProofError::Invalid(m.into()));
        let (Some(params), Some(seed), Some(inputs), Some(refs)) =
            (pd.params, pd.genesis_seed, pd.inputs, pd.input_refs)
        else {
            return bad("stencil verification needs the params, inputs and InputRefs");
        };
        let z = self.setup(params, &seed).map_err(ProofError::Invalid)?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return bad("statement does not match the zone");
        }
        if umis_core::records::list_root_of(refs) != stmt.inputs_commitment
            || inputs.len() != refs.len()
            || refs.iter().any(|r| inputs.get(&r.link_id).map(|b| bmt::file_address(b)) != Some(r.payload_address))
        {
            return bad("inputs do not match inputs_commitment");
        }
        let p = &z.ctx.p;
        match (pd.prev_state, pd.next_state, pd.prev_witness, pd.next_witness) {
            (Some(prev), Some(next), ..) => {
                if bmt::file_address(prev) != stmt.prev_state_root || bmt::file_address(next) != stmt.state_root {
                    return bad("states do not match the statement roots");
                }
                let (Some(s_in), Some(s_out)) = (decode_state(p, prev), decode_state(p, next)) else {
                    return bad("non-canonical state");
                };
                if outputs_root(&<Stencil2d as ZoneTask>::outputs(&z.ctx, &s_out)) != stmt.outputs_root {
                    return bad("outputs_root");
                }
                let decision = decide(&z.ctx, &s_in, inputs, stmt.from_slot);
                let expect = apply(&z.ctx, &s_in, &decision, stmt.from_slot);
                let Decision::Advance { ext } = decision else {
                    if proof != [NATIVE] || s_out != expect {
                        return bad("hold/birth slot does not match");
                    }
                    return Ok((z, None));
                };
                // Everything but the board is native; the board is the STARK claim.
                let header =
                    |s: &crate::State| (s.born, s.generation, s.birth, s.next_slot, s.start_slot, s.prev.clone());
                if header(&s_out) != header(&expect) {
                    return bad("advanced state header or previous board");
                }
                let public = statement(&z.batch.shape, &ext, &s_out.grid);
                Ok((z, Some(public)))
            }
            (None, None, Some(wp), Some(wn)) => {
                let (Some(a), Some(b)) = (
                    summary::check_witness(p, &stmt.prev_state_root, wp),
                    summary::check_witness(p, &stmt.state_root, wn),
                ) else {
                    return bad("state witnesses do not match the statement roots");
                };
                // INV-STENCIL-TASK-5: the outputs read only the header and both borders, which
                // the witness opens against the summary's border digests.
                if outputs_root(&<Stencil2d as ZoneTask>::outputs(&z.ctx, &b.outer)) != stmt.outputs_root {
                    return bad("outputs_root");
                }
                let (a, b) = (a.summary, b.summary);
                // The decision reads only the header and the inputs (the zone part of `E` is
                // not in the halo digest), so a blank board stands in for the state's.
                let blank = Grid::filled(p.n(), p.n(), &z.ctx.rule.boundary());
                let h = &a.header;
                let s_in = crate::State {
                    born: h.born,
                    generation: h.generation,
                    birth: h.birth,
                    next_slot: h.next_slot,
                    start_slot: h.start_slot,
                    grid: blank.clone(),
                    prev: blank,
                };
                let decision = decide(&z.ctx, &s_in, inputs, stmt.from_slot);
                let expect = summary::Header::of(&apply(&z.ctx, &s_in, &decision, stmt.from_slot));
                if b.header != expect {
                    return bad("next state header");
                }
                match decision {
                    Decision::Advance { ext } => {
                        if b.prev != a.board || b.prev_faces != a.faces {
                            return bad("the previous board of an advanced state");
                        }
                        let halo = umis_stencil::air::halo_digest(&z.batch.shape, &ext);
                        Ok((z, Some([a.board, halo, b.board, b.faces].concat())))
                    }
                    Decision::Hold
                        if proof == [NATIVE]
                            && (b.board, b.prev, b.faces, b.prev_faces) == (a.board, a.prev, a.faces, a.prev_faces) =>
                    {
                        Ok((z, None))
                    }
                    Decision::Birth(_)
                        if proof == [NATIVE]
                            && b.board == summary::blank_root(p)
                            && b.faces == summary::blank_faces(p)
                            && (b.prev, b.prev_faces) == (b.board, b.faces) =>
                    {
                        Ok((z, None))
                    }
                    _ => bad("hold/birth slot does not match"),
                }
            }
            _ => bad("stencil verification needs both states or both state witnesses"),
        }
    }
}

/// The relation a zone's leaf proves: its rule and proof layout (one aggregation owner tree
/// per key).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LeafKey {
    pub rule: crate::RuleId,
    pub n: usize,
    pub k: usize,
    pub rpr: usize,
}

impl LeafKey {
    fn of(p: &StencilParams) -> Self {
        Self { rule: p.rule, n: p.n(), k: p.k(), rpr: p.rows_per_trace_row as usize }
    }

    /// The batch leaf of this key (relation and witness generation) at `preset`.
    pub fn batch(&self, preset: FriPreset) -> Result<StencilBatch<crate::AnyRule>, String> {
        StencilBatch::new(self.rule.rule(), self.n, self.k, self.rpr, preset)
    }
}

impl StencilStark {
    /// For the prover of another backend (aggregation): `Some((key, E))` if this slot advances,
    /// with `E` the extended board the leaf proves.
    pub fn advance_claim(
        &self,
        stmt: &TransitionStatement,
        w: &Witness<'_>,
    ) -> Result<Option<(LeafKey, umis_stencil::Grid)>, ProofError> {
        let z = self.setup(w.params, &w.genesis_seed).map_err(ProofError::Prover)?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return Err(ProofError::Prover("statement must cover one slot of this zone".into()));
        }
        let s_in = decode_state(&z.ctx.p, w.prev_state).ok_or_else(|| ProofError::Prover("prev state".into()))?;
        Ok(match decide(&z.ctx, &s_in, w.inputs, stmt.from_slot) {
            Decision::Advance { ext } => Some((LeafKey::of(&z.ctx.p), ext)),
            _ => None,
        })
    }

    /// Verifies a multi-slot head and its references (INV-STENCIL-TASK-3); returns the item
    /// indices it covers.
    fn verify_multi(
        &self,
        items: &[(&TransitionStatement, PublicData<'_>, &[u8])],
        head: usize,
    ) -> Result<Vec<usize>, ProofError> {
        let bad = |m: &str| Err(ProofError::Invalid(m.into()));
        let (idx, body) = umis_proof::group::members(items, head, MULTI_HEAD, MULTI_REF, MAX_MULTI)?;
        let mut setup: Option<Arc<Setup>> = None;
        let mut public = Vec::new();
        for &i in &idx {
            let (s, pd, p) = items[i];
            let (z, claim) = self.native(s, &pd, p)?;
            let Some(statement) = claim else { return bad("multi-slot member is not an advancing slot") };
            if setup.as_ref().is_some_and(|x| !Arc::ptr_eq(x, &z)) {
                return bad("multi-slot members must be slots of one zone");
            }
            public.extend(statement);
            setup = Some(z);
        }
        let z = setup.expect("≥ 2 members");
        let b = z.multi(idx.len()).map_err(ProofError::Invalid)?;
        let proof: BatchProof<koala_bear::Config> =
            postcard::from_bytes(body).map_err(|e| ProofError::Malformed(e.to_string()))?;
        b.verify(&proof, &public).map_err(|e| ProofError::Invalid(format!("multi-slot STARK: {e}")))?;
        Ok(idx)
    }

    /// For the verifier of another backend: every native check of this slot (INV-STENCIL-TASK-1,
    /// from states or witnesses); `Some((key, statement))` for an advance, with the leaf
    /// statement `[d_prev, d_halo, d_next, d_faces]`. A hold or birth slot must carry the native proof
    /// `[0]`.
    pub fn native_claim(
        &self,
        stmt: &TransitionStatement,
        pd: &PublicData<'_>,
        proof: &[u8],
    ) -> Result<Option<(LeafKey, Vec<Val>)>, ProofError> {
        let (z, claim) = self.native(stmt, pd, proof)?;
        Ok(claim.map(|st| (LeafKey::of(&z.ctx.p), st)))
    }
}

/// `outputs_root` over port payloads (as the protocol computes it).
fn outputs_root(outputs: &BTreeMap<PortId, Bytes>) -> Hash {
    let leaves: Vec<Hash> = outputs.values().map(|p| bmt::file_address(p)).collect();
    bmt::list_root(&leaves)
}

impl ProofBackend for StencilStark {
    fn id(&self) -> &'static str {
        BACKEND_ID
    }

    fn is_async(&self) -> bool {
        true
    }

    fn prove(&self, stmt: &TransitionStatement, w: &Witness<'_>, _prover: &SecretKey) -> Result<Bytes, ProofError> {
        let z = self.setup(w.params, &w.genesis_seed).map_err(ProofError::Prover)?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return Err(ProofError::Prover("statement must cover one slot of this zone".into()));
        }
        let s_in = decode_state(&z.ctx.p, w.prev_state).ok_or_else(|| ProofError::Prover("prev state".into()))?;
        let Decision::Advance { ext } = decide(&z.ctx, &s_in, w.inputs, stmt.from_slot) else {
            return Ok(vec![NATIVE]);
        };
        let (traces, public, _) = z.batch.generate(&ext);
        let proof = z.batch.prove(&traces, &public).map_err(ProofError::Prover)?;
        let mut out = vec![STARK];
        out.extend(postcard::to_allocvec(&proof).map_err(|e| ProofError::Prover(e.to_string()))?);
        Ok(out)
    }

    fn batches(&self, task_id: &str) -> bool {
        task_id == crate::TASK_ID
    }

    fn stateless(&self, task_id: &str) -> bool {
        task_id == crate::TASK_ID
    }

    fn prove_batch(
        &self,
        items: &[(&TransitionStatement, Witness<'_>)],
        prover: &SecretKey,
    ) -> Result<Vec<Bytes>, ProofError> {
        let mut out: Vec<Option<Bytes>> = vec![None; items.len()];
        let mut adv: Vec<(usize, umis_stencil::Grid)> = Vec::new();
        let mut zone = None;
        for (i, (stmt, w)) in items.iter().enumerate() {
            match self.advance_claim(stmt, w)? {
                Some((key, ext)) if *zone.get_or_insert((key, w.params)) == (key, w.params) => adv.push((i, ext)),
                Some(_) => return items.iter().map(|(s, w)| self.prove(s, w, prover)).collect(),
                None => out[i] = Some(vec![NATIVE]),
            }
        }
        for part in adv.chunks(MAX_MULTI) {
            if part.len() == 1 {
                let (s, w) = &items[part[0].0];
                out[part[0].0] = Some(self.prove(s, w, prover)?);
                continue;
            }
            let w = &items[part[0].0].1;
            let z = self.setup(w.params, &w.genesis_seed).map_err(ProofError::Prover)?;
            let b = z.multi(part.len()).map_err(ProofError::Prover)?;
            let exts: Vec<_> = part.iter().map(|a| a.1.clone()).collect();
            let (traces, public, _) = b.generate_multi(&exts);
            let proof = b.prove(&traces, &public).map_err(ProofError::Prover)?;
            let body = postcard::to_allocvec(&proof).map_err(|e| ProofError::Prover(e.to_string()))?;
            let members: Vec<Hash> = part.iter().map(|a| items[a.0].0.address()).collect();
            for (a, p) in part.iter().zip(umis_proof::group::proofs(MULTI_HEAD, MULTI_REF, &members, &body)) {
                out[a.0] = Some(p);
            }
        }
        Ok(out.into_iter().map(|p| p.expect("every item")).collect())
    }

    fn verify_group(&self, items: &[(&TransitionStatement, PublicData<'_>, &[u8])]) -> Vec<Result<NodeId, ProofError>> {
        let mut out: Vec<Option<Result<NodeId, ProofError>>> = (0..items.len()).map(|_| None).collect();
        for (i, (_, _, p)) in items.iter().enumerate() {
            if p.first() == Some(&MULTI_HEAD) {
                match self.verify_multi(items, i) {
                    Ok(idx) => idx.into_iter().for_each(|j| out[j] = Some(Ok(NodeId::default()))),
                    Err(e) => out[i] = Some(Err(e)),
                }
            }
        }
        for (i, (s, pd, p)) in items.iter().enumerate() {
            if out[i].is_none() {
                out[i] = Some(if p.first() == Some(&MULTI_REF) {
                    Err(ProofError::Invalid("multi-slot reference without a verified head".into()))
                } else {
                    self.verify(s, pd, p)
                });
            }
        }
        out.into_iter().map(|r| r.expect("every item")).collect()
    }

    fn verify(&self, stmt: &TransitionStatement, pd: &PublicData<'_>, proof: &[u8]) -> Result<NodeId, ProofError> {
        let (z, claim) = self.native(stmt, pd, proof)?;
        let Some(public) = claim else { return Ok(NodeId::default()) };
        let Some((&tag, body)) = proof.split_first() else { return Err(ProofError::Invalid("empty proof".into())) };
        if tag != STARK {
            return Err(ProofError::Invalid("an advancing slot needs a STARK".into()));
        }
        let proof: BatchProof<koala_bear::Config> =
            postcard::from_bytes(body).map_err(|e| ProofError::Malformed(e.to_string()))?;
        z.batch.verify(&proof, &public).map_err(|e| ProofError::Invalid(format!("stencil STARK: {e}")))?;
        let _ = stmt.address();
        Ok(NodeId::default())
    }
}
