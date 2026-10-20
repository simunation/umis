//! The water3d transition AIR (Plonky3 0.8, KoalaBear) and its proof backend.
//!
//! One proof covers one zone slot: `k_sub` substeps of the stencil over the
//! zone's `C = n³` cells.
//!
//! # Layout ("tall": one row per (substep t, cell i), row r = t·C + i)
//!
//! The height is `max(k_sub·C, 2^s)` (a power of two); rows past `k_sub·C`
//! are padding (`active = 0`), so the `2^s`-entry remainder table fits.
//!
//! - main (46): `q[4]` (cell i at substep t), `nb[6][4]` (its neighbours),
//!   `qn[4]` (cell i at t+1), `rem[4]`, `lo[4]`, `hi[4]` (8-bit limbs of
//!   `qn + 2^15`), `m_rem`, `m_byte` (table multiplicities);
//! - preprocessed (32, committed once per zone: they hold the zone's genesis
//!   coefficients): `addr = t·C + i`, `active`, `first`, `last`, `has[6]`
//!   (neighbour inside the zone), `mult` (readers of `qn`), `in_mult` (readers
//!   of the input), `wv[7]`, `wc[7]` (stencil weights), `fv[3]`, `fc`
//!   (forcing shapes), `t_rem = r mod 2^s`, `t_byte = r mod 256`;
//! - periodic (32, period C, evaluated by the verifier from the data it
//!   holds): `x_in[4]`, `x_out[4]`, `halo[6][4]` (the input face on boundary
//!   cells, 0 elsewhere);
//! - public values (17): `tide(j)` and a 256-bit keccak digest (16 × 16-bit
//!   limbs) of `params_hash ‖ slot ‖ X_in ‖ X_out ‖ halos`. Batch-STARK does
//!   not absorb periodic data into Fiat–Shamir, so the digest binds it.
//!
//! # Constraints (all row-local, degree ≤ 3)
//!
//! - stencil, per value f: `Σ_k w_k·val_k + tide·F_f = 2^s·qn_f + rem_f`
//!   (`val_0 = q_f`, `val_{1+d} = nb[d]_f`);
//! - bounds: `qn_f + 2^15 = lo_f + 256·hi_f`, with `lo, hi, rem` looked up in
//!   the tables, so `|qn| ≤ 2^15` and `rem ∈ [0, 2^s)`. With `Σ w ≤ 2^s` the
//!   integer identity holds exactly in the field (no wrap-around);
//! - halo: `active·(1 − has_d)·(nb[d]_f − halo_d_f) = 0`;
//! - IO: `first·(q_f − x_in_f) = 0`, `last·(qn_f − x_out_f) = 0`;
//! - memory bus `mem` with messages `(address, v0, v1, v2, v3)`: each row
//!   provides `(addr + C, qn)` × `mult` and (first substep) `(addr, q)` ×
//!   `in_mult`; it reads `(addr, q)` and `(addr + off_d, nb[d])` for every
//!   in-zone neighbour. Balance forces every read to equal the value written.

use crate::stencil::{Coeffs, OFFSETS, face_index, idx, numerator};
use crate::{CELL_BYTES, Cell, Ctx, FIELDS, Water3d, Water3dParams};
use alloc_shim::*;
use p3_air::{Air, BaseAir, PermutationAirBuilder, WindowAccess};
use p3_batch_stark::{BatchProof, ProverData, StarkInstance, prove_batch, verify_batch};
use p3_field::PrimeCharacteristicRing;
use p3_lookup::{Count, InteractionBuilder, LookupBus};
use p3_matrix::dense::RowMajorMatrix;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use umis_core::records::TransitionStatement;
use umis_core::{Hash, NodeId, SecretKey, bmt, keccak256_concat};
use umis_proof::{ProofBackend, ProofError, PublicData, Witness};
use umis_stark::config::FriPreset;
use umis_stark::config::koala_bear::{Config, Val, config};
use umis_task::{Bytes, PortId, ZoneTask};

mod alloc_shim {
    pub use std::borrow::Cow;
}

pub const BACKEND_ID: &str = "plonky3";
pub const MEM_BUS: &str = "w3d_mem";
pub const REM_BUS: &str = "w3d_rem";
pub const BYTE_BUS: &str = "w3d_byte";
/// `qn + Q_OFFSET` must fit 16 bits.
pub const Q_OFFSET: i64 = 1 << 15;

pub mod col {
    pub const Q: usize = 0;
    pub const NB: usize = 4;
    pub const QN: usize = 28;
    pub const REM: usize = 32;
    pub const LO: usize = 36;
    pub const HI: usize = 40;
    pub const M_REM: usize = 44;
    pub const M_BYTE: usize = 45;
    pub const WIDTH: usize = 46;
}

pub mod pre {
    pub const ADDR: usize = 0;
    pub const ACTIVE: usize = 1;
    pub const FIRST: usize = 2;
    pub const LAST: usize = 3;
    pub const HAS: usize = 4;
    pub const MULT: usize = 10;
    pub const IN_MULT: usize = 11;
    pub const WV: usize = 12;
    pub const WC: usize = 19;
    pub const FV: usize = 26;
    pub const FC: usize = 29;
    pub const T_REM: usize = 30;
    pub const T_BYTE: usize = 31;
    pub const WIDTH: usize = 32;
}

/// Periodic column indices: `x_in[f]`, `x_out[f]`, `halo[d][f]`.
const P_XIN: usize = 0;
const P_XOUT: usize = 4;
const P_HALO: usize = 8;
const N_PERIODIC: usize = 32;
const N_PUBLIC: usize = 17;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub n: usize,
    pub cells: usize,
    pub k_sub: usize,
    pub shift: u32,
    pub height: usize,
}

impl Shape {
    pub fn of(p: &Water3dParams) -> Self {
        let n = p.n();
        let cells = n * n * n;
        let k_sub = p.k_sub as usize;
        let height = (k_sub * cells).max(1 << p.shift).max(256).next_power_of_two();
        Self { n, cells, k_sub, shift: p.shift, height }
    }

    fn active_rows(&self) -> usize {
        self.k_sub * self.cells
    }

    /// Address offset of neighbour direction `d` (face port order).
    fn offset(&self, d: usize) -> i64 {
        let o = OFFSETS[d + 1];
        o[0] + o[1] * self.n as i64 + o[2] * (self.n * self.n) as i64
    }

    /// Whether cell `i` has an in-zone neighbour in direction `d`.
    fn has(&self, i: usize, d: usize) -> bool {
        let n = self.n as i64;
        let (x, y, z) = ((i % self.n) as i64, ((i / self.n) % self.n) as i64, (i / (self.n * self.n)) as i64);
        let o = OFFSETS[d + 1];
        let (a, b, c) = (x + o[0], y + o[1], z + o[2]);
        (0..n).contains(&a) && (0..n).contains(&b) && (0..n).contains(&c)
    }
}

/// The AIR of one slot transition of one zone.
#[derive(Clone)]
pub struct WaterAir {
    pub shape: Shape,
    coeffs: Arc<Coeffs>,
    periodic: Vec<Vec<Val>>,
}

fn fv(v: i64) -> Val {
    Val::from_i64(v)
}

impl WaterAir {
    /// The AIR of a claim; `periodic`/`public` come from [`claim_data`].
    fn new(shape: Shape, coeffs: Arc<Coeffs>, periodic: Vec<Vec<Val>>) -> Self {
        Self { shape, coeffs, periodic }
    }
}

impl BaseAir<Val> for WaterAir {
    fn width(&self) -> usize {
        col::WIDTH
    }

    fn preprocessed_width(&self) -> usize {
        pre::WIDTH
    }

    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<Val>> {
        let s = self.shape;
        let c = &self.coeffs;
        let mut v = Val::zero_vec(s.height * pre::WIDTH);
        for (r, row) in v.chunks_exact_mut(pre::WIDTH).enumerate() {
            row[pre::ADDR] = Val::from_usize(r);
            row[pre::T_REM] = Val::from_usize(r % (1 << s.shift));
            row[pre::T_BYTE] = Val::from_usize(r % 256);
            if r >= s.active_rows() {
                continue;
            }
            let (t, i) = (r / s.cells, r % s.cells);
            row[pre::ACTIVE] = Val::ONE;
            row[pre::FIRST] = Val::from_bool(t == 0);
            row[pre::LAST] = Val::from_bool(t + 1 == s.k_sub);
            let mut readers = 1;
            for d in 0..6 {
                let h = s.has(i, d);
                row[pre::HAS + d] = Val::from_bool(h);
                readers += h as usize;
            }
            row[pre::MULT] = if t + 1 < s.k_sub { Val::from_usize(readers) } else { Val::ZERO };
            row[pre::IN_MULT] = if t == 0 { Val::from_usize(readers) } else { Val::ZERO };
            for k in 0..7 {
                row[pre::WV + k] = fv(c.wv[i][k]);
                row[pre::WC + k] = fv(c.wc[i][k]);
            }
            for f in 0..3 {
                row[pre::FV + f] = fv(c.fv[i][f]);
            }
            row[pre::FC] = fv(c.fc[i]);
        }
        Some(RowMajorMatrix::new(v, pre::WIDTH))
    }

    fn main_next_row_columns(&self) -> Vec<usize> {
        vec![]
    }

    fn preprocessed_next_row_columns(&self) -> Vec<usize> {
        vec![]
    }

    fn num_periodic_columns(&self) -> usize {
        N_PERIODIC
    }

    fn periodic_columns(&self) -> Cow<'_, [Vec<Val>]> {
        Cow::Borrowed(&self.periodic)
    }

    fn num_public_values(&self) -> usize {
        N_PUBLIC
    }
}

impl<AB> Air<AB> for WaterAir
where
    AB: PermutationAirBuilder<F = Val> + InteractionBuilder,
{
    fn eval(&self, builder: &mut AB) {
        let c = |v: i64| AB::Expr::from_i64(v);
        let main = builder.main();
        let m = main.current_slice();
        let prep = builder.preprocessed().clone();
        let p = prep.current_slice();
        let per: Vec<AB::Expr> = builder.periodic_values().iter().map(|x| (*x).into()).collect();
        let tide: AB::Expr = builder.public_values()[0].into();
        let one = AB::Expr::ONE;
        let s = self.shape;

        for f in 0..FIELDS {
            let (w, forcing) = if f < 3 { (pre::WV, p[pre::FV + f]) } else { (pre::WC, p[pre::FC]) };
            let mut num = tide.clone() * forcing + p[w] * m[col::Q + f];
            for d in 0..6 {
                num += p[w + 1 + d] * m[col::NB + 4 * d + f];
            }
            builder.assert_eq(num, m[col::QN + f] * c(1 << s.shift) + m[col::REM + f]);
            builder.assert_eq(m[col::QN + f] + c(Q_OFFSET), m[col::LO + f] + m[col::HI + f] * c(256));
            for d in 0..6 {
                let gate = p[pre::ACTIVE] * (one.clone() - p[pre::HAS + d]);
                builder.assert_zero(gate * (m[col::NB + 4 * d + f] - per[P_HALO + 4 * d + f].clone()));
            }
            builder.assert_zero(p[pre::FIRST] * (m[col::Q + f] - per[P_XIN + f].clone()));
            builder.assert_zero(p[pre::LAST] * (m[col::QN + f] - per[P_XOUT + f].clone()));
        }

        let key = |a: AB::Expr, base: usize| -> [AB::Expr; 5] {
            [a, m[base].into(), m[base + 1].into(), m[base + 2].into(), m[base + 3].into()]
        };
        let addr: AB::Expr = p[pre::ADDR].into();
        let mem = LookupBus::new(MEM_BUS);
        mem.table_entry(builder, key(addr.clone() + c(s.cells as i64), col::QN), p[pre::MULT]);
        mem.table_entry(builder, key(addr.clone(), col::Q), p[pre::IN_MULT]);
        mem.lookup_key(builder, key(addr.clone(), col::Q), Count::bounded(p[pre::ACTIVE].into(), 1));
        for d in 0..6 {
            mem.lookup_key(
                builder,
                key(addr.clone() + c(s.offset(d)), col::NB + 4 * d),
                Count::bounded(p[pre::HAS + d].into(), 1),
            );
        }

        let rem = LookupBus::new(REM_BUS);
        for f in 0..FIELDS {
            rem.lookup_key(builder, [m[col::REM + f]], Count::bounded(p[pre::ACTIVE].into(), 1));
        }
        rem.table_entry(builder, [p[pre::T_REM]], m[col::M_REM]);
        let byte = LookupBus::new(BYTE_BUS);
        for f in 0..FIELDS {
            byte.lookup_key(builder, [m[col::LO + f]], 1);
            byte.lookup_key(builder, [m[col::HI + f]], 1);
        }
        byte.table_entry(builder, [p[pre::T_BYTE]], m[col::M_BYTE]);
    }
}

fn decode(b: &[u8]) -> Vec<Cell> {
    b.chunks_exact(CELL_BYTES)
        .map(|c| std::array::from_fn(|f| i32::from_le_bytes(c[f * 4..f * 4 + 4].try_into().expect("4 bytes"))))
        .collect()
}

/// The 6 halos (input faces; the empty-neighbour boundary when absent).
fn halos(p: &Water3dParams, inputs: &BTreeMap<PortId, Bytes>) -> [Vec<Cell>; 6] {
    std::array::from_fn(|d| match inputs.get(&(d as PortId)) {
        Some(b) if b.len() == p.face_bytes() => decode(b),
        _ => vec![[0; FIELDS]; p.face_cells()],
    })
}

/// Periodic columns and public values of a claim.
fn claim_data(
    ctx: &Ctx,
    params_hash: &Hash,
    slot: u64,
    x_in: &[Cell],
    x_out: &[Cell],
    halo: &[Vec<Cell>; 6],
) -> (Vec<Vec<Val>>, Vec<Val>) {
    let s = Shape::of(&ctx.p);
    let mut per = vec![vec![Val::ZERO; s.cells]; N_PERIODIC];
    let n = s.n;
    for i in 0..s.cells {
        for f in 0..FIELDS {
            per[P_XIN + f][i] = Val::from_i64(x_in[i][f] as i64);
            per[P_XOUT + f][i] = Val::from_i64(x_out[i][f] as i64);
        }
        let (x, y, z) = (i % n, (i / n) % n, i / (n * n));
        for (d, h) in halo.iter().enumerate() {
            if !s.has(i, d) {
                let cell = h[face_index(n, d, x, y, z)];
                for f in 0..FIELDS {
                    per[P_HALO + 4 * d + f][i] = Val::from_i64(cell[f] as i64);
                }
            }
        }
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&params_hash.0);
    bytes.extend_from_slice(&slot.to_be_bytes());
    for cells in [x_in, x_out].into_iter().chain(halo.iter().map(|h| h.as_slice())) {
        for c in cells {
            for v in c {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    let digest = keccak256_concat(&[b"water3d-air", &bytes]);
    let tide = ctx.gw.tide(slot * ctx.p.slot_ticks as u64);
    let mut public = vec![Val::from_i64(tide)];
    for k in 0..16 {
        public.push(Val::from_u64(u16::from_be_bytes([digest.0[2 * k], digest.0[2 * k + 1]]) as u64));
    }
    (per, public)
}

/// The main trace of the honest transition, and `X_out`.
fn generate_trace(ctx: &Ctx, x_in: &[Cell], halo: &[Vec<Cell>; 6], slot: u64) -> (RowMajorMatrix<Val>, Vec<Cell>) {
    let s = Shape::of(&ctx.p);
    let c = &ctx.coeffs;
    let tide = ctx.gw.tide(slot * ctx.p.slot_ticks as u64);
    let n = s.n;
    let mut v = Val::zero_vec(s.height * col::WIDTH);
    let mut cur = x_in.to_vec();
    let mut rem_counts = vec![0u64; 1 << s.shift];
    let mut byte_counts = [0u64; 256];
    for t in 0..s.k_sub {
        let mut next = vec![[0i32; FIELDS]; s.cells];
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    let i = idx(n, x, y, z);
                    let row = &mut v[(t * s.cells + i) * col::WIDTH..][..col::WIDTH];
                    let mut nb = [[0i32; FIELDS]; 7];
                    nb[0] = cur[i];
                    for d in 0..6 {
                        let o = OFFSETS[d + 1];
                        nb[d + 1] = if s.has(i, d) {
                            cur[idx(
                                n,
                                (x as i64 + o[0]) as usize,
                                (y as i64 + o[1]) as usize,
                                (z as i64 + o[2]) as usize,
                            )]
                        } else {
                            halo[d][face_index(n, d, x, y, z)]
                        };
                    }
                    for f in 0..FIELDS {
                        let vals: [i64; 7] = std::array::from_fn(|k| nb[k][f] as i64);
                        let (w, b) = if f < 3 { (&c.wv[i], tide * c.fv[i][f]) } else { (&c.wc[i], tide * c.fc[i]) };
                        let num = numerator(w, &vals, b);
                        let qn = num >> s.shift;
                        let rem = num - (qn << s.shift);
                        next[i][f] = qn as i32;
                        let shifted = qn + Q_OFFSET;
                        let (lo, hi) = (shifted & 0xff, shifted >> 8);
                        row[col::Q + f] = fv(cur[i][f] as i64);
                        for d in 0..6 {
                            row[col::NB + 4 * d + f] = fv(nb[d + 1][f] as i64);
                        }
                        row[col::QN + f] = fv(qn);
                        row[col::REM + f] = fv(rem);
                        row[col::LO + f] = fv(lo);
                        row[col::HI + f] = fv(hi);
                        rem_counts[rem as usize] += 1;
                        byte_counts[lo as usize] += 1;
                        byte_counts[hi.clamp(0, 255) as usize] += 1;
                    }
                }
            }
        }
        cur = next;
    }
    // Padding rows: qn = 0 → lo = 0, hi = 128 (byte lookups are not gated).
    for r in s.active_rows()..s.height {
        let row = &mut v[r * col::WIDTH..][..col::WIDTH];
        for f in 0..FIELDS {
            row[col::HI + f] = fv(Q_OFFSET >> 8);
            byte_counts[0] += 1;
            byte_counts[(Q_OFFSET >> 8) as usize] += 1;
        }
    }
    for (r, row) in v.chunks_exact_mut(col::WIDTH).enumerate() {
        row[col::M_REM] = Val::from_u64(rem_counts.get(r).copied().unwrap_or(0));
        row[col::M_BYTE] = Val::from_u64(byte_counts.get(r).copied().unwrap_or(0));
    }
    (RowMajorMatrix::new(v, col::WIDTH), cur)
}

struct ZoneSetup {
    ctx: Ctx,
    coeffs: Arc<Coeffs>,
    params_hash: Hash,
    prover_data: ProverData<Config>,
}

/// The Plonky3 backend for water3d zones. Per zone (params) it caches the
/// task context and the prover data (the preprocessed commitment, which is
/// also the verifying key).
pub struct Water3dStark {
    preset: FriPreset,
    config: Config,
    setups: Mutex<HashMap<(Hash, Hash), Arc<ZoneSetup>>>,
}

impl Water3dStark {
    pub fn new(preset: FriPreset) -> Self {
        Self { preset, config: config(preset), setups: Mutex::new(HashMap::new()) }
    }

    /// From a genesis `stark_config_id`: `koalabear` (benchmark FRI) or
    /// `koalabear-test` (few queries, fast and insecure, for tests).
    pub fn from_config_id(id: Option<&str>) -> Result<Self, String> {
        match id.unwrap_or("koalabear") {
            "koalabear" => Ok(Self::new(FriPreset::Benchmark)),
            "koalabear-test" => Ok(Self::new(FriPreset::Test)),
            other => Err(format!("unknown stark_config_id {other}")),
        }
    }

    pub fn preset(&self) -> FriPreset {
        self.preset
    }

    fn setup(&self, params: &[u8], seed: &Hash) -> Result<Arc<ZoneSetup>, ProofError> {
        let key = (umis_core::keccak256(params), *seed);
        if let Some(s) = self.setups.lock().expect("setups").get(&key) {
            return Ok(s.clone());
        }
        let p: Water3dParams = borsh::from_slice(params).map_err(|e| ProofError::Prover(e.to_string()))?;
        Water3d::validate_params(&p).map_err(|e| ProofError::Prover(e.to_string()))?;
        let ctx = Water3d::prepare(&p, seed).map_err(|e| ProofError::Prover(e.to_string()))?;
        let coeffs = Arc::new(ctx.coeffs.clone());
        let shape = Shape::of(&p);
        let air = WaterAir::new(shape, coeffs.clone(), vec![vec![Val::ZERO; shape.cells]; N_PERIODIC]);
        let log_h = shape.height.trailing_zeros() as usize;
        let prover_data = ProverData::from_airs_and_degrees_with_lookup_budgets(
            &self.config,
            std::slice::from_ref(&air),
            &[log_h],
            &[3],
            1,
        )
        .map_err(|e| ProofError::Prover(format!("setup: {e}")))?;
        let params_hash = umis_task::params_hash(crate::TASK_ID, crate::LAYER, params);
        let s = Arc::new(ZoneSetup { ctx, coeffs, params_hash, prover_data });
        self.setups.lock().expect("setups").insert(key, s.clone());
        Ok(s)
    }
}

fn check_roots(
    ctx: &Ctx,
    stmt: &TransitionStatement,
    prev: &[u8],
    next: &[u8],
) -> Result<(Vec<Cell>, Vec<Cell>), ProofError> {
    let bad = |m: &str| Err(ProofError::Invalid(m.into()));
    if prev.len() != ctx.p.state_bytes() || next.len() != ctx.p.state_bytes() {
        return bad("state size");
    }
    if bmt::file_address(prev) != stmt.prev_state_root {
        return bad("prev state does not match prev_state_root");
    }
    if bmt::file_address(next) != stmt.state_root {
        return bad("next state does not match state_root");
    }
    Ok((decode(prev), decode(next)))
}

impl ProofBackend for Water3dStark {
    fn id(&self) -> &'static str {
        BACKEND_ID
    }

    fn is_async(&self) -> bool {
        true
    }

    fn prove(&self, stmt: &TransitionStatement, w: &Witness<'_>, _prover: &SecretKey) -> Result<Bytes, ProofError> {
        self.prove_with(stmt, w, |_| {})
    }

    fn verify(&self, stmt: &TransitionStatement, pd: &PublicData<'_>, proof: &[u8]) -> Result<NodeId, ProofError> {
        self.verify_inner(stmt, pd, proof)
    }
}

impl Water3dStark {
    /// Proves with a hook that may alter the trace (tests tamper with it).
    pub fn prove_with(
        &self,
        stmt: &TransitionStatement,
        w: &Witness<'_>,
        tamper: impl FnOnce(&mut RowMajorMatrix<Val>),
    ) -> Result<Bytes, ProofError> {
        let z = self.setup(w.params, &w.genesis_seed)?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return Err(ProofError::Prover("statement must cover one slot of this zone".into()));
        }
        let x_in = decode(w.prev_state);
        if x_in.len() != z.ctx.p.cells() {
            return Err(ProofError::Prover("prev state size".into()));
        }
        let halo = halos(&z.ctx.p, w.inputs);
        let (mut trace, x_out) = generate_trace(&z.ctx, &x_in, &halo, stmt.from_slot);
        tamper(&mut trace);
        let (per, public) = claim_data(&z.ctx, &z.params_hash, stmt.from_slot, &x_in, &x_out, &halo);
        let air = WaterAir::new(Shape::of(&z.ctx.p), z.coeffs.clone(), per);
        let proof = prove_batch(
            &self.config,
            &[StarkInstance { air: &air, trace: &trace, public_values: public }],
            &z.prover_data,
        )
        .map_err(|e| ProofError::Prover(format!("{e:?}")))?;
        postcard::to_allocvec(&proof).map_err(|e| ProofError::Prover(e.to_string()))
    }

    fn verify_inner(
        &self,
        stmt: &TransitionStatement,
        pd: &PublicData<'_>,
        proof: &[u8],
    ) -> Result<NodeId, ProofError> {
        let (Some(params), Some(seed), Some(prev), Some(next), Some(inputs)) =
            (pd.params, pd.genesis_seed, pd.prev_state, pd.next_state, pd.inputs)
        else {
            return Err(ProofError::Invalid("STARK verification needs the states and inputs".into()));
        };
        let z = self.setup(params, &seed).map_err(|e| ProofError::Invalid(e.to_string()))?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return Err(ProofError::Invalid("statement does not match the zone".into()));
        }
        let (x_in, x_out) = check_roots(&z.ctx, stmt, prev, next)?;
        // Outputs and inputs must be the ones the statement commits to.
        let outs = <Water3d as ZoneTask>::outputs(&z.ctx, &crate::State { cells: x_out.clone() });
        if umis_task_outputs_root(&outs) != stmt.outputs_root {
            return Err(ProofError::Invalid("outputs_root".into()));
        }
        if let Some(refs) = pd.input_refs {
            if umis_core::records::list_root_of(refs) != stmt.inputs_commitment {
                return Err(ProofError::Invalid("inputs_commitment".into()));
            }
            for r in refs {
                if inputs.get(&r.link_id).map(|b| bmt::file_address(b)) != Some(r.payload_address) {
                    return Err(ProofError::Invalid("input payload does not match its InputRef".into()));
                }
            }
            if inputs.len() != refs.len() {
                return Err(ProofError::Invalid("unreferenced input payload".into()));
            }
        } else {
            return Err(ProofError::Invalid("STARK verification needs the InputRefs".into()));
        }
        let halo = halos(&z.ctx.p, inputs);
        let (per, public) = claim_data(&z.ctx, &z.params_hash, stmt.from_slot, &x_in, &x_out, &halo);
        let air = WaterAir::new(Shape::of(&z.ctx.p), z.coeffs.clone(), per);
        let proof: BatchProof<Config> =
            postcard::from_bytes(proof).map_err(|e| ProofError::Malformed(e.to_string()))?;
        verify_batch(&self.config, std::slice::from_ref(&air), &proof, &[public], &z.prover_data.common)
            .map_err(|e| ProofError::Invalid(format!("{e:?}")))?;
        Ok(NodeId::default())
    }
}

/// `outputs_root` over port payloads (same as the protocol's).
fn umis_task_outputs_root(outputs: &BTreeMap<PortId, Bytes>) -> Hash {
    let leaves: Vec<Hash> = outputs.values().map(|p| bmt::file_address(p)).collect();
    bmt::list_root(&leaves)
}

/// The proof backend a genesis selects: `mock`, or `plonky3` with its
/// `stark_config_id`.
pub fn backend_for(g: &umis_core::genesis::Genesis) -> Result<Arc<dyn ProofBackend>, String> {
    match g.proof_backend.as_str() {
        umis_proof::MOCK_BACKEND => Ok(Arc::new(umis_proof::MockBackend)),
        BACKEND_ID => Ok(Arc::new(Water3dStark::from_config_id(g.stark_config_id.as_deref())?)),
        other => Err(format!("unknown proof backend {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use umis_task::{DynZoneTask, TaskAdapter, ZoneInstance};

    struct Fixture {
        params: Vec<u8>,
        seed: Hash,
        inst: Arc<dyn ZoneInstance>,
        prev: Vec<u8>,
        inputs: BTreeMap<PortId, Bytes>,
        refs: Vec<umis_core::records::InputRef>,
        next: Vec<u8>,
        stmt: TransitionStatement,
    }

    fn fixture(n_loc: u32) -> Fixture {
        let mut p = Water3dParams::default_for([0, 0, 0], 4);
        p.n_loc = n_loc;
        let params = borsh::to_vec(&p).unwrap();
        let seed = umis_core::keccak256(b"air-test");
        let inst = TaskAdapter::<Water3d>::new().instantiate(&params, &seed).unwrap();
        let prev = inst.initial_state(0);
        let mut face = vec![0u8; p.face_bytes()];
        face[12..16].copy_from_slice(&700i32.to_le_bytes());
        face[0..4].copy_from_slice(&(-300i32).to_le_bytes());
        let inputs: BTreeMap<PortId, Bytes> = [(1u32, face.clone())].into_iter().collect();
        let refs = vec![umis_core::records::InputRef {
            link_id: 1,
            src_zone: 9,
            src_slot: 0,
            src_block_hash: Hash::ZERO,
            src_outputs_root: Hash::ZERO,
            payload_address: bmt::file_address(&face),
        }];
        let slot = 3;
        let next = inst.step(&prev, &inputs, slot).unwrap();
        let outs = inst.outputs(&next).unwrap();
        let stmt = TransitionStatement {
            zone: 0,
            from_slot: slot,
            to_slot: slot + 1,
            params_hash: umis_task::params_hash(crate::TASK_ID, crate::LAYER, &params),
            prev_state_root: bmt::file_address(&prev),
            state_root: bmt::file_address(&next),
            inputs_commitment: umis_core::records::list_root_of(&refs),
            outputs_root: umis_task_outputs_root(&outs),
        };
        Fixture { params, seed, inst, prev, inputs, refs, next, stmt }
    }

    impl Fixture {
        fn witness(&self) -> Witness<'_> {
            Witness {
                instance: self.inst.as_ref(),
                params: &self.params,
                genesis_seed: self.seed,
                prev_state: &self.prev,
                inputs: &self.inputs,
            }
        }

        fn public<'a>(&'a self, next: &'a [u8], inputs: &'a BTreeMap<PortId, Bytes>) -> PublicData<'a> {
            PublicData {
                params: Some(&self.params),
                genesis_seed: Some(self.seed),
                prev_state: Some(&self.prev),
                next_state: Some(next),
                inputs: Some(inputs),
                input_refs: Some(&self.refs),
            }
        }
    }

    #[test]
    fn trace_matches_step() {
        let f = fixture(4);
        let b = Water3dStark::new(FriPreset::Test);
        let z = b.setup(&f.params, &f.seed).unwrap();
        let (_, x_out) = generate_trace(&z.ctx, &decode(&f.prev), &halos(&z.ctx.p, &f.inputs), 3);
        assert_eq!(x_out, decode(&f.next), "AIR witness must match step() bit for bit");
    }

    #[test]
    fn honest_proof_verifies_and_tampering_is_rejected() {
        let f = fixture(4);
        let b = Water3dStark::new(FriPreset::Test);
        let proof = b.prove(&f.stmt, &f.witness(), &SecretKey::from_label("p")).unwrap();
        b.verify(&f.stmt, &f.public(&f.next, &f.inputs), &proof).unwrap();

        // A different X_out whose roots the statement agrees with: only the STARK can catch it.
        let mut next2 = f.next.clone();
        next2[3 * CELL_BYTES] ^= 1;
        let mut stmt2 = f.stmt.clone();
        stmt2.state_root = bmt::file_address(&next2);
        let outs2 = f.inst.outputs(&next2).unwrap();
        stmt2.outputs_root = umis_task_outputs_root(&outs2);
        assert!(b.verify(&stmt2, &f.public(&next2, &f.inputs), &proof).is_err(), "tampered X_out");

        // A different halo (with a matching InputRef).
        let mut inputs2 = f.inputs.clone();
        inputs2.get_mut(&1).unwrap()[12] ^= 4;
        let mut f2 = fixture(4);
        f2.refs[0].payload_address = bmt::file_address(&inputs2[&1]);
        let mut stmt3 = f.stmt.clone();
        stmt3.inputs_commitment = umis_core::records::list_root_of(&f2.refs);
        f2.stmt = stmt3.clone();
        assert!(b.verify(&stmt3, &f2.public(&f.next, &inputs2), &proof).is_err(), "tampered halo");

        // A wrong statement root is caught natively.
        let mut stmt4 = f.stmt.clone();
        stmt4.prev_state_root = Hash::ZERO;
        assert!(b.verify(&stmt4, &f.public(&f.next, &f.inputs), &proof).is_err());
    }

    fn rejected(tamper: impl FnOnce(&mut RowMajorMatrix<Val>) + std::panic::UnwindSafe) -> bool {
        let f = fixture(4);
        let b = Water3dStark::new(FriPreset::Test);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let proof = b.prove_with(&f.stmt, &f.witness(), tamper)?;
            b.verify(&f.stmt, &f.public(&f.next, &f.inputs), &proof).map(|_| ())
        }));
        !matches!(r, Ok(Ok(())))
    }

    #[test]
    fn tampered_traces_are_rejected() {
        // a cell value read by a neighbour
        assert!(rejected(|t| t.values[col::Q + 5 * col::WIDTH] += Val::ONE));
        // a remainder (keeping the stencil identity by shifting qn is impossible: qn is pinned)
        assert!(rejected(|t| {
            t.values[col::REM] += Val::from_u64(1 << 12);
            t.values[col::QN] -= Val::ONE;
        }));
        // a neighbour value not matching the memory
        assert!(rejected(|t| t.values[col::NB + 7 * col::WIDTH] += Val::ONE));
        // a fractional output: rem chosen so that qn is not an integer
        assert!(rejected(|t| t.values[col::REM + 20 * col::WIDTH] += Val::ONE));
    }
}
