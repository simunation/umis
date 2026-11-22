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
//!   checks the proof against `public_values(E, next.grid)` (`d_prev`, `d_halo`, `d_next`), so
//!   the proof binds `next.grid = E.evolve(rule, k)`.
//!
//! Proof bytes: `0` (native) or `1 ‖ postcard(BatchProof)`.

use crate::{Ctx, Decision, Stencil2d, StencilParams, apply, decide, decode_state};
use p3_batch_stark::BatchProof;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use umis_core::records::TransitionStatement;
use umis_core::{Hash, NodeId, SecretKey, SegRecord, bmt};
use umis_proof::{ProofBackend, ProofError, PublicData, Witness};
use umis_stark::config::{FriPreset, koala_bear};
use umis_stencil::air::public_values;
use umis_stencil::batch::StencilBatch;
use umis_task::{Bytes, PortId, ZoneTask};

pub const BACKEND_ID: &str = "plonky3";
const NATIVE: u8 = 0;
const STARK: u8 = 1;

struct Setup {
    ctx: Ctx,
    params_hash: Hash,
    batch: StencilBatch<crate::AnyRule>,
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
        let batch = StencilBatch::new(ctx.rule, p.n(), p.k(), p.rows_per_trace_row as usize, self.preset)?;
        let params_hash = umis_task::params_hash(crate::TASK_ID, crate::LAYER, params);
        let s = Arc::new(Setup { ctx, params_hash, batch });
        self.setups.lock().expect("setups").insert(key, s.clone());
        Ok(s)
    }

    /// Every native check; `Some(E)` for an advancing slot (the board claim the STARK proves).
    fn native(
        &self,
        stmt: &TransitionStatement,
        pd: &PublicData<'_>,
        proof: &[u8],
    ) -> Result<(Arc<Setup>, Option<(umis_stencil::Grid, umis_stencil::Grid)>), ProofError> {
        let bad = |m: &str| Err(ProofError::Invalid(m.into()));
        let (Some(params), Some(seed), Some(prev), Some(next), Some(inputs), Some(refs)) =
            (pd.params, pd.genesis_seed, pd.prev_state, pd.next_state, pd.inputs, pd.input_refs)
        else {
            return bad("stencil verification needs the states, inputs and InputRefs");
        };
        let z = self.setup(params, &seed).map_err(ProofError::Invalid)?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return bad("statement does not match the zone");
        }
        if bmt::file_address(prev) != stmt.prev_state_root || bmt::file_address(next) != stmt.state_root {
            return bad("states do not match the statement roots");
        }
        let (Some(s_in), Some(s_out)) = (decode_state(&z.ctx.p, prev), decode_state(&z.ctx.p, next)) else {
            return bad("non-canonical state");
        };
        if outputs_root(&<Stencil2d as ZoneTask>::outputs(&z.ctx, &s_out)) != stmt.outputs_root {
            return bad("outputs_root");
        }
        if umis_core::records::list_root_of(refs) != stmt.inputs_commitment
            || inputs.len() != refs.len()
            || refs.iter().any(|r| inputs.get(&r.link_id).map(|b| bmt::file_address(b)) != Some(r.payload_address))
        {
            return bad("inputs do not match inputs_commitment");
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
        let header = |s: &crate::State| (s.born, s.generation, s.birth, s.next_slot, s.start_slot, s.prev.clone());
        if header(&s_out) != header(&expect) {
            return bad("advanced state header or previous board");
        }
        Ok((z, Some((ext, s_out.grid))))
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

    /// For the verifier of another backend: every native check of this slot (INV-STENCIL-TASK-1);
    /// `Some((key, E, next))` for an advance, whose leaf statement is
    /// `public_values(E, next)`. A hold or birth slot must carry the native proof `[0]`.
    pub fn native_claim(
        &self,
        stmt: &TransitionStatement,
        pd: &PublicData<'_>,
        proof: &[u8],
    ) -> Result<Option<(LeafKey, umis_stencil::Grid, umis_stencil::Grid)>, ProofError> {
        let (z, claim) = self.native(stmt, pd, proof)?;
        Ok(claim.map(|(ext, next)| (LeafKey::of(&z.ctx.p), ext, next)))
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

    fn verify(&self, stmt: &TransitionStatement, pd: &PublicData<'_>, proof: &[u8]) -> Result<NodeId, ProofError> {
        let (z, claim) = self.native(stmt, pd, proof)?;
        let Some((ext, next)) = claim else { return Ok(NodeId::default()) };
        let Some((&tag, body)) = proof.split_first() else { return Err(ProofError::Invalid("empty proof".into())) };
        if tag != STARK {
            return Err(ProofError::Invalid("an advancing slot needs a STARK".into()));
        }
        let proof: BatchProof<koala_bear::Config> =
            postcard::from_bytes(body).map_err(|e| ProofError::Malformed(e.to_string()))?;
        let public = public_values(&z.batch.shape, &ext, &next);
        z.batch.verify(&proof, &public).map_err(|e| ProofError::Invalid(format!("stencil STARK: {e}")))?;
        let _ = stmt.address();
        Ok(NodeId::default())
    }
}
