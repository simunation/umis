//! The stencil leaf as a batch STARK with lookups (ROADMAP §0 S5a; INV-STENCIL-BATCH-*).
//!
//! Three AIRs in one proof, linked by LogUp buses (p3-lookup):
//!
//! - **Cells**: the generations and the rule's constraints, as in [`crate::air`], plus the
//!   committed limbs of every digest row. Each 8-limb block is sent on [`HASH_BUS`] as
//!   `(region, seq, block)` when its row belongs to the region; `seq = row·P + m` is the
//!   block's position in the canonical absorption order. Every range request `(x, b)` is
//!   looked up on [`RANGE_BUS`]: `x`, and `x·2^(16−b)` when `b < 16`. For `2 ≤ b ≤ 16` both in
//!   `[0, 2^16)` ⇔ `x ∈ [0, 2^b)`: `x < 2^16` makes `x·2^(16−b) < 2^(32−b) ≤ 2^30 < p`, so the
//!   product cannot wrap. `b = 1` would wrap (65025·2^15 − p = 2^15 − 1), so it is a
//!   booleanity constraint instead (INV-STENCIL-BATCH-3).
//! - **Hash**: one Poseidon2 permutation per row, the three sponges one after the other in
//!   canonical order. Each active row is a table entry `(region, seq, block)` with
//!   multiplicity 1, its capacity chains from the previous row (zero at a region's first
//!   row), and a region's last row outputs its digest (the public values).
//! - **Range**: the table `0..2^16` with a multiplicity column.
//!
//! Bus keys and counts use only main and preprocessed columns: rc.2's LogUp trace builder
//! cannot see periodic columns.

use crate::air::{Cells, N_PUBLIC, REGIONS, Region, StencilAir, pack, region_values};
use crate::poseidon::{DIGEST, PERM_COLS, RATE, WIDTH, eval_permutation, permute_into};
use crate::{Grid, Rule, Shape, Val};
use p3_air::{Air, AirBuilder, BaseAir, PermutationAirBuilder, WindowAccess};
use p3_batch_stark::{BatchProof, ProverData, StarkInstance, prove_batch, verify_batch};
use p3_field::{PrimeCharacteristicRing, PrimeField32};
use p3_lookup::{Count, InteractionBuilder, LookupBus};
use p3_matrix::dense::RowMajorMatrix;
use std::borrow::Cow;
use umis_stark::config::{FriPreset, koala_bear};

pub const HASH_BUS: &str = "stencil-hash";
pub const RANGE_BUS: &str = "stencil-range";
/// The range table holds `0..2^RANGE_BITS`.
pub const RANGE_BITS: usize = 16;
/// Lookup degree budget per AIR.
const LOOKUP_BUDGET: usize = 3;

// Hash AIR columns.
const H_IN: usize = 0;
const H_PERM: usize = WIDTH;
const H_WIDTH: usize = WIDTH + PERM_COLS;
// Hash AIR preprocessed columns.
const HP_ACTIVE: usize = 0;
const HP_FIRST: usize = 1;
const HP_LAST: usize = 2; // + region
const HP_TAG: usize = 5;
const HP_SEQ: usize = 6;
const HP_WIDTH: usize = 7;

impl Shape {
    fn limb_base(&self, g: Region) -> usize {
        self.cells_width() + REGIONS[..g as usize].iter().map(|r| self.rpr * self.limbs(*r)).sum::<usize>()
    }
    fn limb_col(&self, g: Region, i: usize, q: usize) -> usize {
        self.limb_base(g) + i * self.limbs(g) + q
    }
    fn batch_cells_width(&self) -> usize {
        self.limb_base(Region::Next) + self.rpr * self.limbs(Region::Next)
    }
    // cells preprocessed: per (region, slot) a gate and the first block's seq
    fn pre_gate(&self, g: Region, i: usize) -> usize {
        2 * (g as usize * self.rpr + i)
    }
    fn pre_seq(&self, g: Region, i: usize) -> usize {
        self.pre_gate(g, i) + 1
    }
    fn region_rows(&self, g: Region) -> usize {
        if g == Region::Halo { self.we } else { self.n }
    }
    /// Hash rows used by region `g`, and the first of them.
    fn hash_span(&self, g: Region) -> (usize, usize) {
        let start = REGIONS[..g as usize].iter().map(|r| self.region_rows(*r) * self.perms(*r)).sum();
        (start, self.region_rows(g) * self.perms(g))
    }
    pub fn hash_height(&self) -> usize {
        let (s, l) = self.hash_span(Region::Next);
        (s + l).next_power_of_two().max(16)
    }
    /// The board row slot `i` of trace row `r` absorbs into region `g`, if any.
    fn absorbed_row(&self, g: Region, r: usize, i: usize) -> Option<usize> {
        let y = self.rpr * r + i;
        let on = if g == Region::Halo { self.in_e(r, i) } else { self.zone(r, i) };
        on.then_some(y)
    }
}

/// One of the three AIRs.
#[derive(Clone)]
pub enum BatchAir<R> {
    Cells { rule: R, shape: Shape, periodic: Vec<Vec<Val>> },
    Hash { shape: Shape },
    Range,
}

impl<R: Rule + Clone> BaseAir<Val> for BatchAir<R> {
    fn width(&self) -> usize {
        match self {
            Self::Cells { shape, .. } => shape.batch_cells_width(),
            Self::Hash { .. } => H_WIDTH,
            Self::Range => 1,
        }
    }
    fn preprocessed_width(&self) -> usize {
        match self {
            Self::Cells { shape, .. } => 2 * REGIONS.len() * shape.rpr,
            Self::Hash { .. } => HP_WIDTH,
            Self::Range => 1,
        }
    }
    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<Val>> {
        Some(match self {
            Self::Cells { shape: s, .. } => {
                let w = 2 * REGIONS.len() * s.rpr;
                let mut v = Val::zero_vec(s.height * w);
                for (r, row) in v.chunks_mut(w).enumerate() {
                    for g in REGIONS {
                        for i in 0..s.rpr {
                            if let Some(y) = s.absorbed_row(g, r, i) {
                                row[s.pre_gate(g, i)] = Val::ONE;
                                row[s.pre_seq(g, i)] = Val::from_usize(y * s.perms(g));
                            }
                        }
                    }
                }
                RowMajorMatrix::new(v, w)
            }
            Self::Hash { shape: s } => {
                let h = s.hash_height();
                let mut v = Val::zero_vec(h * HP_WIDTH);
                for g in REGIONS {
                    let (start, len) = s.hash_span(g);
                    for k in 0..len {
                        let row = &mut v[(start + k) * HP_WIDTH..][..HP_WIDTH];
                        row[HP_ACTIVE] = Val::ONE;
                        row[HP_FIRST] = Val::from_bool(k == 0);
                        row[HP_LAST + g as usize] = Val::from_bool(k + 1 == len);
                        row[HP_TAG] = Val::from_usize(g as usize);
                        row[HP_SEQ] = Val::from_usize(k);
                    }
                }
                RowMajorMatrix::new(v, HP_WIDTH)
            }
            Self::Range => RowMajorMatrix::new((0..1u32 << RANGE_BITS).map(Val::from_u32).collect(), 1),
        })
    }
    fn main_next_row_columns(&self) -> Vec<usize> {
        match self {
            Self::Cells { shape, .. } => (0..shape.cells_width()).collect(),
            Self::Hash { .. } => (H_IN + RATE..H_IN + WIDTH).collect(),
            Self::Range => vec![],
        }
    }
    fn preprocessed_next_row_columns(&self) -> Vec<usize> {
        match self {
            Self::Hash { .. } => vec![HP_ACTIVE, HP_FIRST],
            _ => vec![],
        }
    }
    fn num_periodic_columns(&self) -> usize {
        match self {
            Self::Cells { shape, .. } => shape.rpr,
            _ => 0,
        }
    }
    fn periodic_columns(&self) -> Cow<'_, [Vec<Val>]> {
        match self {
            Self::Cells { periodic, .. } => Cow::Borrowed(periodic),
            _ => Cow::Owned(Vec::new()),
        }
    }
    fn num_public_values(&self) -> usize {
        match self {
            Self::Hash { .. } => N_PUBLIC,
            _ => 0,
        }
    }
}

/// The rule's constraints and range requests on the current and next row.
fn rule_rows<E, V, R>(rule: &R, s: &Shape, cells: &Cells<'_, V>, out: &mut Vec<E>, ranges: &mut Vec<(E, usize)>)
where
    E: PrimeCharacteristicRing + Clone + From<V> + From<Val>,
    V: Copy,
    R: Rule,
{
    let (r, v) = (s.spec.radius, s.spec.values);
    for i in 0..s.rpr {
        for x in 0..s.we {
            let c: Vec<E> = (0..v).map(|j| cells.get(0, i, x, j)).collect();
            let a: Vec<E> = (0..s.spec.aux0).map(|j| cells.get(0, i, x, v + j)).collect();
            rule.eval_state(&c, &a, out, ranges);
        }
    }
    for t in 0..s.k {
        let (x0, w) = s.cone(t + 1);
        for i in 0..s.rpr {
            for x in x0..x0 + w {
                let nb_vals: Vec<Vec<E>> = (0..2 * r + 1)
                    .flat_map(|dy| (0..2 * r + 1).map(move |dx| (dx, dy)))
                    .map(|(dx, dy)| (0..v).map(|j| cells.get(t, i + dy, x + dx - r, j)).collect())
                    .collect();
                let nb: Vec<&[E]> = nb_vals.iter().map(|c| c.as_slice()).collect();
                let nx: Vec<E> = (0..v).map(|j| cells.get(t + 1, i, x, j)).collect();
                let aux: Vec<E> = (0..s.spec.aux).map(|j| cells.get(t + 1, i, x, v + j)).collect();
                rule.eval_step(&nb, &nx, &aux, out, ranges);
            }
        }
    }
}

impl<AB, R> Air<AB> for BatchAir<R>
where
    AB: PermutationAirBuilder<F = Val> + InteractionBuilder,
    R: Rule + Clone,
{
    fn eval(&self, builder: &mut AB) {
        match self {
            Self::Cells { rule, shape: s, .. } => {
                let main = builder.main();
                let (cur, next) = (main.current_slice(), main.next_slice());
                let prep = builder.preprocessed().clone();
                let p = prep.current_slice();
                let mut per: Vec<AB::Expr> = builder.periodic_values().iter().map(|x| (*x).into()).collect();
                if per.is_empty() {
                    per = vec![AB::Expr::ZERO; s.rpr]; // see `StencilAir::eval`
                }
                let cells = Cells { s, cur, next };
                let (mut out, mut ranges) = (Vec::new(), Vec::new());
                rule_rows::<AB::Expr, AB::Var, R>(rule, s, &cells, &mut out, &mut ranges);
                for c in out {
                    builder.assert_zero(c);
                }
                // INV-STENCIL-BATCH-3: range requests.
                let range = LookupBus::new(RANGE_BUS);
                for (x, b) in ranges {
                    assert!((1..=RANGE_BITS).contains(&b), "range requests have 1..=16 bits");
                    if b == 1 {
                        builder.assert_zero(x.clone() * (x - AB::Expr::ONE));
                        continue;
                    }
                    if b < RANGE_BITS {
                        range.lookup_key(builder, [x.clone() * AB::Expr::from_u64(1 << (RANGE_BITS - b))], 1);
                    }
                    range.lookup_key(builder, [x], 1);
                }
                // INV-STENCIL-BATCH-1: limbs bound to the cells, blocks sent to the hash table.
                let hash = LookupBus::new(HASH_BUS);
                for g in REGIONS {
                    for i in 0..s.rpr {
                        let vals = region_values::<AB::Expr, AB::Var>(s, g, &cells, i, &per[i]);
                        let packed = pack(s, &vals, s.perms(g));
                        for q in 0..s.limbs(g) {
                            builder.assert_eq(cur[s.limb_col(g, i, q)], packed[q].clone());
                        }
                        let limb = |q: usize| -> AB::Expr {
                            if q < s.limbs(g) { cur[s.limb_col(g, i, q)].into() } else { AB::Expr::ZERO }
                        };
                        for m in 0..s.perms(g) {
                            let key: Vec<AB::Expr> =
                                [AB::Expr::from_usize(g as usize), p[s.pre_seq(g, i)].into() + AB::Expr::from_usize(m)]
                                    .into_iter()
                                    .chain((0..RATE).map(|j| limb(m * RATE + j)))
                                    .collect();
                            hash.lookup_key(builder, key, Count::bounded(p[s.pre_gate(g, i)].into(), 1));
                        }
                    }
                }
            }
            Self::Hash { .. } => {
                let main = builder.main();
                let (cur, next) = (main.current_slice(), main.next_slice());
                let prep = builder.preprocessed().clone();
                let (p, pn) = (prep.current_slice(), prep.next_slice());
                let public: Vec<AB::Expr> = builder.public_values().iter().map(|x| (*x).into()).collect();
                let input: [AB::Expr; WIDTH] = core::array::from_fn(|j| cur[H_IN + j].into());
                let po = eval_permutation(builder, input, &cur[H_PERM..H_PERM + PERM_COLS]);
                // INV-STENCIL-BATCH-2: zero capacity at a region's start, chaining otherwise.
                for j in RATE..WIDTH {
                    builder.assert_zero(p[HP_FIRST] * cur[H_IN + j]);
                    let chain = pn[HP_ACTIVE] - pn[HP_FIRST];
                    builder.when_transition().assert_zero(chain * (next[H_IN + j] - po[j].clone()));
                }
                for g in REGIONS {
                    for j in 0..DIGEST {
                        let d = public[g as usize * DIGEST + j].clone();
                        builder.assert_zero(p[HP_LAST + g as usize] * (po[j].clone() - d));
                    }
                }
                let key: Vec<AB::Expr> = [p[HP_TAG].into(), p[HP_SEQ].into()]
                    .into_iter()
                    .chain((0..RATE).map(|j| cur[H_IN + j].into()))
                    .collect();
                LookupBus::new(HASH_BUS).table_entry(builder, key, p[HP_ACTIVE]);
            }
            Self::Range => {
                let main = builder.main();
                let m = main.current_slice();
                let prep = builder.preprocessed().clone();
                let p = prep.current_slice();
                LookupBus::new(RANGE_BUS).table_entry(builder, [p[0]], m[0]);
            }
        }
    }
}

/// The batch prover of rule `R` on one zone shape (setup cached).
pub struct StencilBatch<R> {
    pub rule: R,
    pub shape: Shape,
    pub preset: FriPreset,
    config: koala_bear::Config,
    airs: Vec<BatchAir<R>>,
    prover_data: ProverData<koala_bear::Config>,
}

impl<R: Rule + Clone> StencilBatch<R> {
    pub fn new(rule: R, n: usize, k: usize, rpr: usize, preset: FriPreset) -> Result<Self, String> {
        let shape = Shape::new(rule.spec(), n, k, rpr)?;
        let periodic =
            (0..rpr).map(|i| (0..shape.height).map(|r| Val::from_bool(shape.inner0(r, i))).collect()).collect();
        // The range table only when the rule requests ranges (it is a fixed 2^16-row cost).
        let ranged = shape.spec.state_ranges + shape.spec.step_ranges > 0;
        let mut airs = vec![BatchAir::Cells { rule: rule.clone(), shape, periodic }, BatchAir::Hash { shape }];
        let mut degrees = vec![shape.height.trailing_zeros() as usize, shape.hash_height().trailing_zeros() as usize];
        if ranged {
            airs.push(BatchAir::Range);
            degrees.push(RANGE_BITS);
        }
        let config = koala_bear::config(preset);
        let prover_data = ProverData::from_airs_and_degrees_with_lookup_budgets(
            &config,
            &airs,
            &degrees,
            &vec![LOOKUP_BUDGET; airs.len()],
            preset.log_blowup(),
        )
        .map_err(|e| format!("setup: {e:?}"))?;
        Ok(Self { rule, shape, preset, config, airs, prover_data })
    }

    /// The three traces, the public values and the board after `k` generations.
    pub fn generate(&self, ext: &Grid) -> (Vec<RowMajorMatrix<Val>>, Vec<Val>, Grid) {
        let s = &self.shape;
        let uni = StencilAir::new(self.rule.clone(), s.n, s.k, s.rpr).expect("shape");
        let (cells, next) = uni.generate_cells(ext);
        let (traces, public) = self.assemble(&cells);
        (traces, public, next)
    }

    /// The limbs, the hash table, the range multiplicities and the public values of any cells
    /// trace (cell columns only), exactly as the AIRs read them: honest for an honest trace;
    /// for a tampered one, the claim the tampered cells make (tests forge with it).
    pub fn assemble(&self, cells: &RowMajorMatrix<Val>) -> (Vec<RowMajorMatrix<Val>>, Vec<Val>) {
        let s = &self.shape;
        let (cw, bw, h) = (s.cells_width(), s.batch_cells_width(), s.height);
        let mut v = Val::zero_vec(h * bw);
        let mut mult = vec![0u32; 1 << RANGE_BITS];
        // blocks[g] in canonical order: (trace row, slot) with the gate on
        let mut blocks: [Vec<Vec<Val>>; 3] = Default::default();
        for r in 0..h {
            let (cur, nxt) = (&cells.values[r * cw..][..cw], &cells.values[((r + 1) % h) * cw..][..cw]);
            let row = &mut v[r * bw..(r + 1) * bw];
            row[..cw].copy_from_slice(cur);
            let view = Cells { s, cur, next: nxt };
            for g in REGIONS {
                for i in 0..s.rpr {
                    let inner0 = Val::from_bool(s.inner0(r, i));
                    let limbs = pack(s, &region_values::<Val, Val>(s, g, &view, i, &inner0), s.perms(g));
                    for q in 0..s.limbs(g) {
                        row[s.limb_col(g, i, q)] = limbs[q];
                    }
                    if s.absorbed_row(g, r, i).is_some() {
                        blocks[g as usize].push(limbs);
                    }
                }
            }
            let (mut out, mut ranges) = (Vec::new(), Vec::new());
            rule_rows::<Val, Val, R>(&self.rule, s, &view, &mut out, &mut ranges);
            for (x, b) in ranges.into_iter().filter(|(_, b)| *b > 1) {
                let x = x.as_canonical_u32() as u64;
                for y in [Some(x), (b < RANGE_BITS).then(|| x << (RANGE_BITS - b))].into_iter().flatten() {
                    // An out-of-range value has no table row: the bus cannot balance.
                    if (y as usize) < mult.len() {
                        mult[y as usize] += 1;
                    }
                }
            }
        }
        let cells_trace = RowMajorMatrix::new(v, bw);
        let hh = s.hash_height();
        let mut hv = Val::zero_vec(hh * H_WIDTH);
        let mut public = Vec::with_capacity(N_PUBLIC);
        for g in REGIONS {
            let (start, len) = s.hash_span(g);
            let mut st = [Val::ZERO; WIDTH];
            let mut k = 0;
            for limbs in &blocks[g as usize] {
                for m in 0..s.perms(g) {
                    st[..RATE].copy_from_slice(&limbs[m * RATE..][..RATE]);
                    let row = &mut hv[(start + k) * H_WIDTH..][..H_WIDTH];
                    row[H_IN..H_IN + WIDTH].copy_from_slice(&st);
                    st = permute_into(st, &mut row[H_PERM..H_PERM + PERM_COLS]);
                    k += 1;
                }
            }
            debug_assert_eq!(k, len);
            public.extend_from_slice(&st[..DIGEST]);
        }
        let (s_end, l_end) = s.hash_span(Region::Next);
        for r in s_end + l_end..hh {
            let row = &mut hv[r * H_WIDTH..][..H_WIDTH];
            let _ = permute_into([Val::ZERO; WIDTH], &mut row[H_PERM..H_PERM + PERM_COLS]);
        }
        let mut traces = vec![cells_trace, RowMajorMatrix::new(hv, H_WIDTH)];
        if self.airs.len() == 3 {
            traces.push(RowMajorMatrix::new(mult.into_iter().map(Val::from_u32).collect(), 1));
        } else {
            debug_assert!(mult.iter().all(|m| *m == 0), "a rule without range requests");
        }
        (traces, public)
    }

    /// The cell columns of a cells trace.
    pub fn cells_only(&self, t: &RowMajorMatrix<Val>) -> RowMajorMatrix<Val> {
        let (bw, cw) = (self.shape.batch_cells_width(), self.shape.cells_width());
        RowMajorMatrix::new(t.values.chunks(bw).flat_map(|r| r[..cw].to_vec()).collect(), cw)
    }

    pub fn prove(
        &self,
        traces: &[RowMajorMatrix<Val>],
        public: &[Val],
    ) -> Result<BatchProof<koala_bear::Config>, String> {
        if traces.len() != self.airs.len() {
            return Err("one trace per AIR".into());
        }
        let instances: Vec<StarkInstance<'_, koala_bear::Config, BatchAir<R>>> = self
            .airs
            .iter()
            .zip(traces)
            .enumerate()
            .map(|(a, (air, trace))| StarkInstance {
                air,
                trace,
                public_values: if a == 1 { public.to_vec() } else { Vec::new() },
            })
            .collect();
        prove_batch(&self.config, &instances, &self.prover_data).map_err(|e| format!("{e:?}"))
    }

    pub fn verify(&self, proof: &BatchProof<koala_bear::Config>, public: &[Val]) -> Result<(), String> {
        let publics: Vec<Vec<Val>> =
            (0..self.airs.len()).map(|a| if a == 1 { public.to_vec() } else { Vec::new() }).collect();
        verify_batch(&self.config, &self.airs, proof, &publics, &self.prover_data.common).map_err(|e| format!("{e:?}"))
    }

    /// The shape of a batch proof for the security calculator, read from the proof: its
    /// tallest instance, every opened codeword (base columns 1, EF4 columns 4), the degree its
    /// quotient chunking allows, and upper estimates for what it does not carry (constraints:
    /// 4 per opened codeword; LogUp: 4 interactions of width 10 per permutation column).
    pub fn security_shape(proof: &BatchProof<koala_bear::Config>) -> umis_stark::security::ProofShape {
        const E: usize = 4;
        let log_height = proof.degree_bits.iter().copied().max().unwrap_or(0);
        let (mut openings, mut perm_cols, mut chunks) = (0usize, 0usize, 1usize);
        for inst in &proof.opened_values.instances {
            let b = &inst.base_opened_values;
            openings += b.trace_local.len() + b.trace_next.as_ref().map_or(0, Vec::len);
            if let Some(pre) = &b.preprocessed {
                openings += pre.local.len() + pre.next.as_ref().map_or(0, Vec::len);
            }
            openings += E * b.quotient_chunks.iter().map(Vec::len).sum::<usize>();
            openings += E * (inst.permutation_local.len() + inst.permutation_next.len());
            perm_cols += inst.permutation_local.len();
            chunks = chunks.max(b.quotient_chunks.len());
        }
        umis_stark::security::ProofShape {
            log_height,
            num_constraints: 4 * openings,
            max_degree: chunks + 1,
            batched_openings: openings,
            logup: (perm_cols > 0).then_some((4 * perm_cols, 2 + RATE)),
        }
    }

    /// Widths of the AIRs (cells, hash, and the range table if present).
    pub fn widths(&self) -> Vec<usize> {
        [self.shape.batch_cells_width(), H_WIDTH, 1][..self.airs.len()].to_vec()
    }
}

impl Shape {
    /// The width of the batch leaf's cells AIR.
    pub fn batch_width(&self) -> usize {
        self.batch_cells_width()
    }

    #[cfg(test)]
    pub(crate) fn test_limb_col(&self, g: Region, i: usize, q: usize) -> usize {
        self.limb_col(g, i, q)
    }
}

#[cfg(test)]
pub(crate) const HASH_IN: usize = H_IN;
