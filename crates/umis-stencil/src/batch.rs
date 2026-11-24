//! The stencil leaf as a batch STARK with lookups (ROADMAP §0 S5a; INV-STENCIL-BATCH-*).
//!
//! Three AIRs in one proof, linked by LogUp buses (p3-lookup):
//!
//! - **Cells**: the generations and the rule's constraints, as in [`crate::air`], plus the
//!   committed limbs of every digest row. Each 8-limb block is sent on [`HASH_BUS`] as
//!   `(region, seq, block)` when its row belongs to the region; `seq = row·P + m` is the
//!   block's position in the canonical absorption order. Every range request `(x, b)`,
//!   `2 ≤ b ≤ 16`, is one lookup of the pair `(x, b)` on [`RANGE_BUS`]; `b = 1` is a booleanity
//!   constraint (INV-STENCIL-BATCH-3).
//! - **Hash**: one Poseidon2 permutation per row, the three sponges one after the other in
//!   canonical order. Each active row is a table entry `(region, seq, block)` with
//!   multiplicity 1, its capacity chains from the previous row (zero at a region's first
//!   row), and a region's last row outputs its digest (the public values).
//! - **Range**, in one of two forms ([`RangeTable`], chosen per shape):
//!   - `Pairs`: the pairs `(v, b)`, `v < 2^b`, for every `b` in `2..=16` (2^17 rows). One
//!     lookup per request.
//!   - `Single`: the column `0..2^16`. A request looks up `x` and, if `b < 16`, `x·2^(16−b)`:
//!     both in `[0, 2^16)` ⇔ `x ∈ [0, 2^b)`, since `x < 2^16` keeps the product below
//!     `2^30 < p` for `b ≥ 2`. Half the table, but two lookups per narrow request.
//!
//!   `Pairs` is chosen when a slot makes at least 2^15 narrow (`b < 16`) requests, where the
//!   lookups it saves outweigh the larger table (LBM 32×32: verify −23 %, proof −16 %; heat
//!   64×64 would prove 75 % slower with it; measured 2026-11-22).
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
use p3_maybe_rayon::prelude::*;
use std::borrow::Cow;
use umis_stark::config::{FriPreset, koala_bear};

pub const HASH_BUS: &str = "stencil-hash";
pub const RANGE_BUS: &str = "stencil-range";
/// The widest range request.
pub const RANGE_BITS: usize = 16;
/// The range table's height: `Σ_{b=2}^{16} 2^b` rows, padded to `2^17`.
const RANGE_LOG_HEIGHT: usize = RANGE_BITS + 1;

/// The row of `(v, b)` in the range table.
fn range_row(v: u64, b: usize) -> usize {
    ((1usize << b) - 4) + v as usize
}
/// Lookup degree budget per AIR: the largest constraint degree the preset's blowup supports
/// (`blowup + 1`, at most 5). A larger budget packs more lookups per LogUp column (LBM at P100:
/// budget 5 instead of 3 gives 35 % smaller proofs; measured 2026-11-22). `UMIS_LOOKUP_BUDGET`
/// overrides it, for measurements.
fn lookup_budget(preset: FriPreset) -> usize {
    lookup_budget_for(preset.log_blowup())
}

/// The LogUp degree budget at FRI blowup `2^log_blowup`: `blowup + 1`, between 3 and 5.
pub fn lookup_budget_for(log_blowup: usize) -> usize {
    std::env::var("UMIS_LOOKUP_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(((1usize << log_blowup) + 1).clamp(3, 5))
}

// Hash AIR columns.
const H_IN: usize = 0;
const H_PERM: usize = WIDTH;
const H_WIDTH: usize = WIDTH + PERM_COLS;
// Hash AIR preprocessed columns (for `m` slots): active, first, one last-row flag per
// (slot, region), the tag `3·slot + region` and the block's seq.
const HP_ACTIVE: usize = 0;
const HP_FIRST: usize = 1;
const HP_LAST: usize = 2; // + 3·slot + region
const fn hp_tag(m: usize) -> usize {
    HP_LAST + 3 * m
}
const fn hp_seq(m: usize) -> usize {
    hp_tag(m) + 1
}
const fn hp_width(m: usize) -> usize {
    hp_seq(m) + 1
}

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
        self.hash_height_for(1)
    }
    /// Hash rows one slot uses.
    fn slot_hash_rows(&self) -> usize {
        let (s, l) = self.hash_span(Region::Next);
        s + l
    }
    /// The hash table's height for `m` slots (slot `j`'s rows start at `j·slot_hash_rows`).
    pub fn hash_height_for(&self, m: usize) -> usize {
        (m * self.slot_hash_rows()).next_power_of_two().max(16)
    }
    /// The board row slot `i` of trace row `r` absorbs into region `g`, if any.
    fn absorbed_row(&self, g: Region, r: usize, i: usize) -> Option<usize> {
        let y = self.rpr * r + i;
        let on = if g == Region::Halo { self.in_e(r, i) } else { self.zone(r, i) };
        on.then_some(y)
    }
}

/// One of the AIRs: a cells AIR per slot, one hash table, one range table.
#[derive(Clone)]
pub enum BatchAir<R> {
    /// The generations of slot `slot` (its blocks carry the tag `3·slot + region`).
    Cells {
        rule: R,
        shape: Shape,
        periodic: Vec<Vec<Val>>,
        table: RangeTable,
        slot: usize,
    },
    /// The sponges of `slots` slots, slot by slot.
    Hash {
        shape: Shape,
        slots: usize,
    },
    Range(RangeTable),
}

/// The form of the range table (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeTable {
    Single,
    Pairs,
}

impl RangeTable {
    fn log_height(self) -> usize {
        match self {
            Self::Single => RANGE_BITS,
            Self::Pairs => RANGE_LOG_HEIGHT,
        }
    }
    /// The table rows request `(x, b)` uses (`None`: out of range, no row).
    fn rows(self, x: u64, b: usize) -> Vec<Option<usize>> {
        match self {
            Self::Pairs => vec![(x < 1 << b).then(|| range_row(x, b))],
            Self::Single => {
                let mut v = vec![(x < 1 << RANGE_BITS).then_some(x as usize)];
                if b < RANGE_BITS {
                    let y = x << (RANGE_BITS - b);
                    v.push((y < 1 << RANGE_BITS).then_some(y as usize));
                }
                v
            }
        }
    }
}

impl<R: Rule + Clone> BaseAir<Val> for BatchAir<R> {
    fn width(&self) -> usize {
        match self {
            Self::Cells { shape, .. } => shape.batch_cells_width(),
            Self::Hash { .. } => H_WIDTH,
            Self::Range(_) => 1,
        }
    }
    fn preprocessed_width(&self) -> usize {
        match self {
            Self::Cells { shape, .. } => 2 * REGIONS.len() * shape.rpr,
            Self::Hash { slots, .. } => hp_width(*slots),
            Self::Range(RangeTable::Pairs) => 2,
            Self::Range(RangeTable::Single) => 1,
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
            Self::Hash { shape: s, slots: m } => {
                let (m, w) = (*m, hp_width(*m));
                let h = s.hash_height_for(m);
                let mut v = Val::zero_vec(h * w);
                for j in 0..m {
                    for g in REGIONS {
                        let (start, len) = s.hash_span(g);
                        let start = j * s.slot_hash_rows() + start;
                        let tag = 3 * j + g as usize;
                        for k in 0..len {
                            let row = &mut v[(start + k) * w..][..w];
                            row[HP_ACTIVE] = Val::ONE;
                            row[HP_FIRST] = Val::from_bool(k == 0);
                            row[HP_LAST + tag] = Val::from_bool(k + 1 == len);
                            row[hp_tag(m)] = Val::from_usize(tag);
                            row[hp_seq(m)] = Val::from_usize(k);
                        }
                    }
                }
                RowMajorMatrix::new(v, w)
            }
            Self::Range(RangeTable::Single) => {
                RowMajorMatrix::new((0..1u32 << RANGE_BITS).map(Val::from_u32).collect(), 1)
            }
            Self::Range(RangeTable::Pairs) => {
                // (v, b) for b = 2..=16, v < 2^b; padding rows repeat (0, 2) with multiplicity 0
                let mut v = Vec::with_capacity(2 << RANGE_LOG_HEIGHT);
                for b in 2..=RANGE_BITS {
                    for x in 0..1u32 << b {
                        v.extend([Val::from_u32(x), Val::from_usize(b)]);
                    }
                }
                while v.len() < 2 << RANGE_LOG_HEIGHT {
                    v.extend([Val::ZERO, Val::from_usize(2)]);
                }
                RowMajorMatrix::new(v, 2)
            }
        })
    }
    fn main_next_row_columns(&self) -> Vec<usize> {
        match self {
            Self::Cells { shape, .. } => (0..shape.cells_width()).collect(),
            Self::Hash { .. } => (H_IN + RATE..H_IN + WIDTH).collect(),
            Self::Range(_) => vec![],
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
            Self::Hash { slots, .. } => N_PUBLIC * slots,
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
            Self::Cells { rule, shape: s, table, slot, .. } => {
                let main = builder.main();
                let (cur, next) = (main.current_slice(), main.next_slice());
                let prep = builder.preprocessed().clone();
                let p = prep.current_slice();
                // Every builder declares the periodic columns (the P3R layouts too: vendor/p3r-periodic-layout.patch).
                let per: Vec<AB::Expr> = builder.periodic_values().iter().map(|x| (*x).into()).collect();
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
                    match table {
                        RangeTable::Pairs => range.lookup_key(builder, [x, AB::Expr::from_usize(b)], 1),
                        RangeTable::Single => {
                            if b < RANGE_BITS {
                                range.lookup_key(builder, [x.clone() * AB::Expr::from_u64(1 << (RANGE_BITS - b))], 1);
                            }
                            range.lookup_key(builder, [x], 1);
                        }
                    }
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
                            let key: Vec<AB::Expr> = [
                                AB::Expr::from_usize(3 * slot + g as usize),
                                p[s.pre_seq(g, i)].into() + AB::Expr::from_usize(m),
                            ]
                            .into_iter()
                            .chain((0..RATE).map(|j| limb(m * RATE + j)))
                            .collect();
                            hash.lookup_key(builder, key, Count::bounded(p[s.pre_gate(g, i)].into(), 1));
                        }
                    }
                }
            }
            Self::Hash { slots, .. } => {
                let m = *slots;
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
                // INV-STENCIL-BATCH-4: slot `s`'s digests are its public values `[24s, 24s + 24)`.
                for tag in 0..3 * m {
                    for j in 0..DIGEST {
                        let d = public[tag * DIGEST + j].clone();
                        builder.assert_zero(p[HP_LAST + tag] * (po[j].clone() - d));
                    }
                }
                let key: Vec<AB::Expr> = [p[hp_tag(m)].into(), p[hp_seq(m)].into()]
                    .into_iter()
                    .chain((0..RATE).map(|j| cur[H_IN + j].into()))
                    .collect();
                LookupBus::new(HASH_BUS).table_entry(builder, key, p[HP_ACTIVE]);
            }
            Self::Range(t) => {
                let main = builder.main();
                let m = main.current_slice();
                let prep = builder.preprocessed().clone();
                let p = prep.current_slice();
                match t {
                    RangeTable::Pairs => LookupBus::new(RANGE_BUS).table_entry(builder, [p[0], p[1]], m[0]),
                    RangeTable::Single => LookupBus::new(RANGE_BUS).table_entry(builder, [p[0]], m[0]),
                }
            }
        }
    }
}

/// The batch prover of rule `R` on one zone shape and `slots` slots per proof (setup cached).
pub struct StencilBatch<R> {
    pub rule: R,
    pub shape: Shape,
    pub preset: FriPreset,
    /// Slots per proof: one cells AIR each, one shared hash and range table (INV-STENCIL-BATCH-4).
    pub slots: usize,
    config: koala_bear::Config,
    airs: Vec<BatchAir<R>>,
    degrees: Vec<usize>,
    prover_data: ProverData<koala_bear::Config>,
}

impl<R: Rule + Clone> StencilBatch<R> {
    /// One slot per proof.
    pub fn new(rule: R, n: usize, k: usize, rpr: usize, preset: FriPreset) -> Result<Self, String> {
        Self::new_multi(rule, n, k, rpr, preset, 1)
    }

    /// `slots` consecutive slots of one zone per proof.
    pub fn new_multi(rule: R, n: usize, k: usize, rpr: usize, preset: FriPreset, slots: usize) -> Result<Self, String> {
        if slots == 0 {
            return Err("a proof covers at least one slot".into());
        }
        let shape = Shape::new(rule.spec(), n, k, rpr)?;
        let periodic: Vec<Vec<Val>> =
            (0..rpr).map(|i| (0..shape.height).map(|r| Val::from_bool(shape.inner0(r, i))).collect()).collect();
        // The range table only when the rule requests ranges (it is a fixed 2^16-row cost).
        let ranged = shape.spec.state_ranges + shape.spec.step_ranges > 0;
        let table = Self::choose_table(&rule, &shape, slots);
        let mut airs: Vec<BatchAir<R>> = (0..slots)
            .map(|slot| BatchAir::Cells { rule: rule.clone(), shape, periodic: periodic.clone(), table, slot })
            .collect();
        airs.push(BatchAir::Hash { shape, slots });
        let mut degrees = vec![shape.height.trailing_zeros() as usize; slots];
        degrees.push(shape.hash_height_for(slots).trailing_zeros() as usize);
        if ranged {
            airs.push(BatchAir::Range(table));
            degrees.push(table.log_height());
        }
        let config = koala_bear::config(preset);
        let prover_data = ProverData::from_airs_and_degrees_with_lookup_budgets(
            &config,
            &airs,
            &degrees,
            &vec![lookup_budget(preset); airs.len()],
            preset.log_blowup(),
        )
        .map_err(|e| format!("setup: {e:?}"))?;
        Ok(Self { rule, shape, preset, slots, config, airs, degrees, prover_data })
    }

    /// The AIRs of the leaf (a cells AIR per slot, the hash table, and the range table if the
    /// rule requests ranges), for proving the same relation under another STARK configuration
    /// (recursion: `umis-agg`).
    pub fn airs(&self) -> &[BatchAir<R>] {
        &self.airs
    }

    /// log₂ trace heights of [`Self::airs`].
    pub fn degrees(&self) -> &[usize] {
        &self.degrees
    }

    /// Per-AIR public values of a leaf with claim `public` (the hash AIR carries the claim:
    /// `N_PUBLIC` values per slot).
    pub fn publics(&self, public: &[Val]) -> Vec<Vec<Val>> {
        (0..self.airs.len()).map(|a| if a == self.slots { public.to_vec() } else { Vec::new() }).collect()
    }

    /// The range table for a shape: `Pairs` when a proof makes at least 2^15 narrow requests.
    /// The count comes from the rule's own range requests on one zero cell.
    fn choose_table(rule: &R, s: &Shape, slots: usize) -> RangeTable {
        let v = s.spec.values;
        let zero = vec![Val::ZERO; v];
        let (mut out, mut state, mut step) = (Vec::new(), Vec::new(), Vec::new());
        rule.eval_state(&zero, &vec![Val::ZERO; s.spec.aux0], &mut out, &mut state);
        let nb_cells = vec![zero.clone(); (2 * s.spec.radius + 1).pow(2)];
        let nb: Vec<&[Val]> = nb_cells.iter().map(|c| c.as_slice()).collect();
        rule.eval_step(&nb, &zero, &vec![Val::ZERO; s.spec.aux], &mut out, &mut step);
        let narrow = |r: &[(Val, usize)]| r.iter().filter(|(_, b)| (2..RANGE_BITS).contains(b)).count();
        let rows = s.rpr * s.height;
        let cone_cells: usize = (1..=s.k).map(|t| s.cone(t).1).sum();
        let total = slots * rows * (s.we * narrow(&state) + cone_cells * narrow(&step));
        if total >= 1 << 15 { RangeTable::Pairs } else { RangeTable::Single }
    }

    /// The range table this leaf uses.
    pub fn range_table(&self) -> Option<RangeTable> {
        self.airs.iter().find_map(|a| if let BatchAir::Range(t) = a { Some(*t) } else { None })
    }

    /// The traces, the public values and the board after `k` generations of one slot.
    pub fn generate(&self, ext: &Grid) -> (Vec<RowMajorMatrix<Val>>, Vec<Val>, Grid) {
        let (t, p, mut next) = self.generate_multi(std::slice::from_ref(ext));
        (t, p, next.pop().expect("one slot"))
    }

    /// [`Self::generate`] for `slots` extended boards (one per slot, in order): the traces, the
    /// public values (slot by slot) and each slot's next board.
    pub fn generate_multi(&self, exts: &[Grid]) -> (Vec<RowMajorMatrix<Val>>, Vec<Val>, Vec<Grid>) {
        assert_eq!(exts.len(), self.slots, "one extended board per slot");
        let s = &self.shape;
        let uni = StencilAir::new(self.rule.clone(), s.n, s.k, s.rpr).expect("shape");
        let (cells, next): (Vec<_>, Vec<_>) = exts.par_iter().map(|e| uni.generate_cells(e)).unzip();
        let (traces, public) = self.assemble_multi(&cells);
        (traces, public, next)
    }

    /// The limbs, the hash table, the range multiplicities and the public values of any cells
    /// trace (cell columns only), exactly as the AIRs read them: honest for an honest trace;
    /// for a tampered one, the claim the tampered cells make (tests forge with it).
    pub fn assemble(&self, cells: &RowMajorMatrix<Val>) -> (Vec<RowMajorMatrix<Val>>, Vec<Val>) {
        self.assemble_multi(std::slice::from_ref(cells))
    }

    /// [`Self::assemble`] for one cells trace per slot.
    pub fn assemble_multi(&self, cells: &[RowMajorMatrix<Val>]) -> (Vec<RowMajorMatrix<Val>>, Vec<Val>) {
        let order: Vec<usize> = (0..cells.len()).collect();
        self.assemble_ordered(cells, &order)
    }

    /// [`Self::assemble_multi`] with the hash table's slot `j` absorbing the blocks of cells
    /// trace `order[j]` (honest: the identity). Tests forge with other orders.
    #[doc(hidden)]
    pub fn assemble_ordered(
        &self,
        cells: &[RowMajorMatrix<Val>],
        order: &[usize],
    ) -> (Vec<RowMajorMatrix<Val>>, Vec<Val>) {
        assert_eq!(cells.len(), self.slots, "one cells trace per slot");
        let s = &self.shape;
        let (cw, bw, h) = (s.cells_width(), s.batch_cells_width(), s.height);
        let table = match &self.airs[0] {
            BatchAir::Cells { table, .. } => *table,
            _ => unreachable!("cells AIRs first"),
        };
        // Per slot, in parallel: the cells trace with limbs, its blocks [g] in canonical order
        // ((trace row, slot row) with the gate on) and its range multiplicities.
        let per_slot = |cells: &RowMajorMatrix<Val>| {
            let mut mult = vec![0u32; 1 << table.log_height()];
            let mut v = Val::zero_vec(h * bw);
            let mut b: [Vec<Vec<Val>>; 3] = Default::default();
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
                            b[g as usize].push(limbs);
                        }
                    }
                }
                let (mut out, mut ranges) = (Vec::new(), Vec::new());
                rule_rows::<Val, Val, R>(&self.rule, s, &view, &mut out, &mut ranges);
                for (x, bits) in ranges.into_iter().filter(|(_, b)| *b > 1) {
                    let x = x.as_canonical_u32() as u64;
                    // An out-of-range value has no table row: the bus cannot balance.
                    for r in table.rows(x, bits).into_iter().flatten() {
                        mult[r] += 1;
                    }
                }
            }
            (RowMajorMatrix::new(v, bw), b, mult)
        };
        let parts: Vec<_> = cells.par_iter().map(per_slot).collect();
        let mut mult = vec![0u32; 1 << table.log_height()];
        let mut traces = Vec::with_capacity(self.airs.len());
        let mut blocks: Vec<[Vec<Vec<Val>>; 3]> = Vec::with_capacity(cells.len());
        for (t, b, m) in parts {
            for (a, x) in mult.iter_mut().zip(m) {
                *a += x;
            }
            traces.push(t);
            blocks.push(b);
        }
        let m = self.slots;
        let hh = s.hash_height_for(m);
        let mut hv = Val::zero_vec(hh * H_WIDTH);
        let mut public = Vec::with_capacity(N_PUBLIC * m);
        for (j, &src) in order.iter().enumerate() {
            for g in REGIONS {
                let (start, len) = s.hash_span(g);
                let start = j * s.slot_hash_rows() + start;
                let mut st = [Val::ZERO; WIDTH];
                let mut k = 0;
                for limbs in &blocks[src][g as usize] {
                    for q in 0..s.perms(g) {
                        st[..RATE].copy_from_slice(&limbs[q * RATE..][..RATE]);
                        let row = &mut hv[(start + k) * H_WIDTH..][..H_WIDTH];
                        row[H_IN..H_IN + WIDTH].copy_from_slice(&st);
                        st = permute_into(st, &mut row[H_PERM..H_PERM + PERM_COLS]);
                        k += 1;
                    }
                }
                debug_assert_eq!(k, len);
                public.extend_from_slice(&st[..DIGEST]);
            }
        }
        for r in m * s.slot_hash_rows()..hh {
            let row = &mut hv[r * H_WIDTH..][..H_WIDTH];
            let _ = permute_into([Val::ZERO; WIDTH], &mut row[H_PERM..H_PERM + PERM_COLS]);
        }
        traces.push(RowMajorMatrix::new(hv, H_WIDTH));
        if self.airs.len() == m + 2 {
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
        let publics = self.publics(public);
        let instances: Vec<StarkInstance<'_, koala_bear::Config, BatchAir<R>>> = self
            .airs
            .iter()
            .zip(traces)
            .zip(publics)
            .map(|((air, trace), public_values)| StarkInstance { air, trace, public_values })
            .collect();
        prove_batch(&self.config, &instances, &self.prover_data).map_err(|e| format!("{e:?}"))
    }

    pub fn verify(&self, proof: &BatchProof<koala_bear::Config>, public: &[Val]) -> Result<(), String> {
        if public.len() != N_PUBLIC * self.slots {
            return Err(format!("{} public values for {} slots", public.len(), self.slots));
        }
        let publics = self.publics(public);
        verify_batch(&self.config, &self.airs, proof, &publics, &self.prover_data.common).map_err(|e| format!("{e:?}"))
    }

    /// The shape of a batch proof for the security calculator, read from the proof: its
    /// tallest instance, every opened codeword (base columns 1, EF4 columns 4), the degree its
    /// quotient chunking allows, and upper estimates for what it does not carry (constraints:
    /// 4 per opened codeword; LogUp: 4 interactions of width 10 per permutation column).
    pub fn security_shape(proof: &BatchProof<koala_bear::Config>) -> umis_stark::security::ProofShape {
        security_shape_of(proof, 4)
    }

    /// Widths of the AIRs (cells per slot, hash, and the range table if present).
    pub fn widths(&self) -> Vec<usize> {
        self.airs.iter().map(BaseAir::<Val>::width).collect()
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

/// [`StencilBatch::security_shape`] for a batch proof under any configuration whose challenge
/// field has degree `ext` over KoalaBear.
pub fn security_shape_of<SC: p3_batch_stark::StarkGenericConfig>(
    proof: &BatchProof<SC>,
    ext: usize,
) -> umis_stark::security::ProofShape {
    let log_height = proof.degree_bits.iter().copied().max().unwrap_or(0);
    let (mut openings, mut perm_cols, mut chunks) = (0usize, 0usize, 1usize);
    for inst in &proof.opened_values.instances {
        let b = &inst.base_opened_values;
        openings += b.trace_local.len() + b.trace_next.as_ref().map_or(0, Vec::len);
        if let Some(pre) = &b.preprocessed {
            openings += pre.local.len() + pre.next.as_ref().map_or(0, Vec::len);
        }
        openings += ext * b.quotient_chunks.iter().map(Vec::len).sum::<usize>();
        openings += ext * (inst.permutation_local.len() + inst.permutation_next.len());
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
