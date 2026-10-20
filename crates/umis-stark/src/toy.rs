//! Toy AIR spike: a 1D integer stencil proven with Plonky3 batch-STARK + LogUp.
//!
//! # Statement
//!
//! Starting from `X_in` (N cells), apply `T` steps of
//!
//! ```text
//!     L        = w0*q[i] + w1*q[i-1] + w2*q[i+1] + b      (out-of-range neighbours = HALO)
//!     q'[i]    = floor(L / 2^s),  rem = L mod 2^s          (Euclidean, rem in [0, 2^s))
//! ```
//!
//! and end at `X_out`. Values stay in `[-2^(Q-1), 2^(Q-1))` because the weights sum to `2^s`.
//!
//! # Layout ("tall": one row per (step t, cell i), row r = t*N + i, height N*T)
//!
//! - main: `q, left, right, q_next, rem, mult, rs_mult, rq_mult, dummy...`
//! - preprocessed (fixed per shape, committed once): `addr = r`, `has_left = [i>0]`,
//!   `has_right = [i<N-1]`, `first = [t==0]`, `last = [t==T-1]`, `rs_table = r mod 2^s`,
//!   `rq_table = r mod 2^Q`
//! - periodic (period N, not committed): `x_in[i]`, `x_out[i]`
//! - public values: `X_in ++ X_out` (only to bind them into Fiat-Shamir, see below)
//!
//! # Constraints
//!
//! - stencil: `w0*q + w1*left + w2*right + b = 2^s*q_next + rem`
//! - halo: `(1-has_left)*(left-HALO) = 0`, same for right
//! - IO: `first*(q - x_in) = 0`, `last*(q_next - x_out) = 0`
//! - memory bus `mem` (value at address a = t*N+i is cell i at step t):
//!   - provide `(addr+N, q_next)` with multiplicity column `mult` (honest: number of readers)
//!   - provide `(addr, q)` with multiplicity `first*(1+has_left+has_right)` (inputs)
//!   - read `(addr, q)`, `(addr-1, left)` if has_left, `(addr+1, right)` if has_right
//! - range buses: `rem in [0,2^s)` and `q_next + 2^(Q-1) in [0, 2^Q)` against the table columns
//!
//! The output range check matters: without it, any `rem` in range admits a (field-valued)
//! `q_next = (L - rem)/2^s`.
//!
//! # Why public values *and* periodic columns
//!
//! LogUp trace generation in p3-lookup 0.8 does not see periodic columns (its row builder returns
//! an empty periodic slice), so anything a bus message or count uses must be main or preprocessed.
//! Periodic columns are fine in plain constraints, which is how `X_in`/`X_out` reach the rows.
//! But batch-stark does **not** absorb periodic data into the transcript, so the vectors are also
//! passed as public values (which are absorbed); the verifier builds the AIR from those same values.

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;

use p3_air::symbolic::SymbolicExpressionExt;
use p3_air::{Air, BaseAir, PermutationAirBuilder, WindowAccess};
use p3_batch_stark::config::PcsProverError;
use p3_batch_stark::{
    BatchProof, BatchVerificationError, Domain, PcsError, ProverData, ProvingError, StarkGenericConfig, StarkInstance,
    Val, prove_batch, verify_batch,
};
use p3_challenger::GrindingChallenger;
use p3_commit::Pcs;
use p3_field::{Algebra, Field, PrimeCharacteristicRing, PrimeField64};
use p3_lookup::{Count, InteractionBuilder, LookupBus};
use p3_matrix::dense::RowMajorMatrix;

/// Remainder bits `s`.
pub const SHIFT: u32 = 4;
/// Value bits `Q`: every state value lies in `[-2^(Q-1), 2^(Q-1))`.
pub const Q_BITS: u32 = 8;
/// Stencil weights `(w_self, w_left, w_right)`; they sum to `2^SHIFT`.
pub const WEIGHTS: [i64; 3] = [8, 3, 5];
/// Additive bias `b` (must be in `[0, 2^SHIFT)` to keep values bounded).
pub const BIAS: i64 = 5;
/// Value read for a neighbour outside the grid.
pub const HALO: i64 = 2;

const Q_OFFSET: i64 = 1 << (Q_BITS - 1);

pub const MEM_BUS: &str = "mem";
pub const RANGE_REM_BUS: &str = "range_rem";
pub const RANGE_Q_BUS: &str = "range_q";

/// Main-trace column indices.
pub mod col {
    pub const Q: usize = 0;
    pub const LEFT: usize = 1;
    pub const RIGHT: usize = 2;
    pub const Q_NEXT: usize = 3;
    pub const REM: usize = 4;
    pub const MULT: usize = 5;
    pub const RS_MULT: usize = 6;
    pub const RQ_MULT: usize = 7;
    /// First dummy (padding) column.
    pub const DUMMY: usize = 8;
}

/// Preprocessed column indices.
pub mod pre {
    pub const ADDR: usize = 0;
    pub const HAS_LEFT: usize = 1;
    pub const HAS_RIGHT: usize = 2;
    pub const FIRST: usize = 3;
    pub const LAST: usize = 4;
    pub const RS_TABLE: usize = 5;
    pub const RQ_TABLE: usize = 6;
    pub const WIDTH: usize = 7;
}

/// Instance shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToyParams {
    /// Cells N (power of two).
    pub n_cells: usize,
    /// Steps T (power of two).
    pub n_steps: usize,
    /// Extra degree-2 dummy columns, to emulate a wider AIR.
    pub extra_cols: usize,
}

impl ToyParams {
    pub fn new(n_cells: usize, n_steps: usize, extra_cols: usize) -> Self {
        assert!(n_cells.is_power_of_two() && n_steps.is_power_of_two());
        let p = Self { n_cells, n_steps, extra_cols };
        // The range-table columns list every table value at least once.
        assert!(p.height() >= 1 << Q_BITS, "need at least 2^Q_BITS rows for the range table");
        p
    }

    pub const fn height(&self) -> usize {
        self.n_cells * self.n_steps
    }

    pub const fn width(&self) -> usize {
        col::DUMMY + self.extra_cols
    }
}

/// One stencil step on plain integers.
pub fn step_reference(state: &[i64]) -> Vec<i64> {
    let n = state.len();
    (0..n)
        .map(|i| {
            let left = if i > 0 { state[i - 1] } else { HALO };
            let right = if i + 1 < n { state[i + 1] } else { HALO };
            stencil_sum(state[i], left, right).div_euclid(1 << SHIFT)
        })
        .collect()
}

const fn stencil_sum(q: i64, left: i64, right: i64) -> i64 {
    WEIGHTS[0] * q + WEIGHTS[1] * left + WEIGHTS[2] * right + BIAS
}

/// All states `X_0 = x_in, ..., X_T` of the reference run.
pub fn run_reference(x_in: &[i64], n_steps: usize) -> Vec<Vec<i64>> {
    let mut states = vec![x_in.to_vec()];
    for _ in 0..n_steps {
        let next = step_reference(states.last().expect("non-empty"));
        states.push(next);
    }
    states
}

/// The toy AIR for one `(X_in, X_out)` claim.
#[derive(Clone, Debug)]
pub struct ToyAir<F> {
    pub params: ToyParams,
    /// `[x_in, x_out]`, each of length N (period N).
    periodic: Vec<Vec<F>>,
}

impl<F: PrimeCharacteristicRing + Clone> ToyAir<F> {
    pub fn new(params: ToyParams, x_in: &[i64], x_out: &[i64]) -> Self {
        let to_f = |v: &[i64]| v.iter().map(|&x| F::from_i64(x)).collect::<Vec<_>>();
        Self::from_field(params, to_f(x_in), to_f(x_out))
    }

    /// Like [`ToyAir::new`] with field-valued vectors (lets tests claim non-integer outputs).
    pub fn from_field(params: ToyParams, x_in: Vec<F>, x_out: Vec<F>) -> Self {
        assert_eq!(x_in.len(), params.n_cells);
        assert_eq!(x_out.len(), params.n_cells);
        Self { params, periodic: vec![x_in, x_out] }
    }

    pub fn x_in(&self) -> &[F] {
        &self.periodic[0]
    }

    pub fn x_out(&self) -> &[F] {
        &self.periodic[1]
    }

    /// Public values `X_in ++ X_out`.
    pub fn public_values(&self) -> Vec<F> {
        self.periodic.concat()
    }
}

impl<F: Field> BaseAir<F> for ToyAir<F> {
    fn width(&self) -> usize {
        self.params.width()
    }

    fn preprocessed_trace(&self) -> Option<RowMajorMatrix<F>> {
        let ToyParams { n_cells, n_steps, .. } = self.params;
        let mut values = Vec::with_capacity(self.params.height() * pre::WIDTH);
        for r in 0..self.params.height() {
            let (t, i) = (r / n_cells, r % n_cells);
            values.extend([
                F::from_usize(r),
                F::from_bool(i > 0),
                F::from_bool(i + 1 < n_cells),
                F::from_bool(t == 0),
                F::from_bool(t + 1 == n_steps),
                F::from_usize(r % (1 << SHIFT)),
                F::from_usize(r % (1 << Q_BITS)),
            ]);
        }
        Some(RowMajorMatrix::new(values, pre::WIDTH))
    }

    fn preprocessed_width(&self) -> usize {
        pre::WIDTH
    }

    // Every constraint is row-local, so no column needs opening at the next row.
    fn main_next_row_columns(&self) -> Vec<usize> {
        vec![]
    }

    fn preprocessed_next_row_columns(&self) -> Vec<usize> {
        vec![]
    }

    fn num_periodic_columns(&self) -> usize {
        self.periodic.len()
    }

    fn periodic_columns(&self) -> Cow<'_, [Vec<F>]> {
        Cow::Borrowed(&self.periodic)
    }

    fn num_public_values(&self) -> usize {
        2 * self.params.n_cells
    }
}

impl<AB> Air<AB> for ToyAir<AB::F>
where
    AB: PermutationAirBuilder + InteractionBuilder,
    AB::F: Field,
{
    fn eval(&self, builder: &mut AB) {
        let c = |v: i64| AB::Expr::from_i64(v);
        let main = builder.main();
        let m = main.current_slice();
        let prep = builder.preprocessed().clone();
        let p = prep.current_slice();
        let (x_in, x_out): (AB::Expr, AB::Expr) = {
            let pv = builder.periodic_values();
            (pv[0].into(), pv[1].into())
        };

        let (q, left, right, q_next, rem) = (m[col::Q], m[col::LEFT], m[col::RIGHT], m[col::Q_NEXT], m[col::REM]);
        let (addr, has_left, has_right, first, last) =
            (p[pre::ADDR], p[pre::HAS_LEFT], p[pre::HAS_RIGHT], p[pre::FIRST], p[pre::LAST]);

        // Stencil step with remainder.
        let lhs = q * c(WEIGHTS[0]) + left * c(WEIGHTS[1]) + right * c(WEIGHTS[2]) + c(BIAS);
        builder.assert_eq(lhs, q_next * c(1 << SHIFT) + rem);

        // Halo at the grid boundary; IO binding at the first / last step.
        builder.assert_zero((AB::Expr::ONE - has_left) * (left - c(HALO)));
        builder.assert_zero((AB::Expr::ONE - has_right) * (right - c(HALO)));
        builder.assert_zero(first * (q - x_in));
        builder.assert_zero(last * (q_next - x_out));

        // Dummy degree-2 chain: d_0 = q*left + 1, d_k = d_{k-1}*right + q + k.
        let mut expected: AB::Expr = q * left + AB::Expr::ONE;
        for k in 0..self.params.extra_cols {
            let d = m[col::DUMMY + k];
            builder.assert_eq(d, expected);
            expected = d * right + q + c(k as i64 + 1);
        }

        // Memory bus: every read of (address, value) must hit a provided entry.
        let n = c(self.params.n_cells as i64);
        let mem = LookupBus::new(MEM_BUS);
        mem.table_entry(builder, [addr + n, q_next.into()], m[col::MULT]);
        mem.table_entry(builder, [addr.into(), q.into()], first * (AB::Expr::ONE + has_left + has_right));
        mem.lookup_key(builder, [addr.into(), q.into()], 1);
        mem.lookup_key(builder, [addr - AB::Expr::ONE, left.into()], Count::bounded(has_left.into(), 1));
        mem.lookup_key(builder, [addr + AB::Expr::ONE, right.into()], Count::bounded(has_right.into(), 1));

        // Range checks.
        let rs = LookupBus::new(RANGE_REM_BUS);
        rs.lookup_key(builder, [rem], 1);
        rs.table_entry(builder, [p[pre::RS_TABLE]], m[col::RS_MULT]);
        let rq = LookupBus::new(RANGE_Q_BUS);
        rq.lookup_key(builder, [q_next + c(Q_OFFSET)], 1);
        rq.table_entry(builder, [p[pre::RQ_TABLE]], m[col::RQ_MULT]);
    }
}

/// Build the main trace from the reference states `X_0..X_T`.
pub fn generate_trace<F: PrimeField64>(params: &ToyParams, states: &[Vec<i64>]) -> RowMajorMatrix<F> {
    let ToyParams { n_cells, n_steps, extra_cols } = *params;
    assert_eq!(states.len(), n_steps + 1);
    let width = params.width();
    let mut values = F::zero_vec(params.height() * width);
    for (r, row) in values.chunks_exact_mut(width).enumerate() {
        let (t, i) = (r / n_cells, r % n_cells);
        let (has_left, has_right) = (i > 0, i + 1 < n_cells);
        let q = states[t][i];
        let left = if has_left { states[t][i - 1] } else { HALO };
        let right = if has_right { states[t][i + 1] } else { HALO };
        let sum = stencil_sum(q, left, right);
        let q_next = sum.div_euclid(1 << SHIFT);
        assert_eq!(q_next, states[t + 1][i], "states are not a reference run");
        let readers = if t + 1 < n_steps { 1 + has_left as i64 + has_right as i64 } else { 0 };

        row[col::Q] = F::from_i64(q);
        row[col::LEFT] = F::from_i64(left);
        row[col::RIGHT] = F::from_i64(right);
        row[col::Q_NEXT] = F::from_i64(q_next);
        row[col::REM] = F::from_i64(sum.rem_euclid(1 << SHIFT));
        row[col::MULT] = F::from_i64(readers);

        let (fq, fl, fr) = (row[col::Q], row[col::LEFT], row[col::RIGHT]);
        let mut expected = fq * fl + F::ONE;
        for k in 0..extra_cols {
            row[col::DUMMY + k] = expected;
            expected = expected * fr + fq + F::from_usize(k + 1);
        }
    }
    let mut trace = RowMajorMatrix::new(values, width);
    fill_range_multiplicities(params, &mut trace);
    trace
}

/// Recompute the range-table multiplicity columns from the `rem` / `q_next` columns.
///
/// Each table value `v` is credited on row `v` (its first occurrence); out-of-range values are
/// not credited (so they stay unmatched on the bus).
pub fn fill_range_multiplicities<F: PrimeField64>(params: &ToyParams, trace: &mut RowMajorMatrix<F>) {
    let width = params.width();
    let mut rs = [0u64; 1 << SHIFT];
    let mut rq = [0u64; 1 << Q_BITS];
    for row in trace.values.chunks_exact(width) {
        if let Some(n) = rs.get_mut(row[col::REM].as_canonical_u64() as usize) {
            *n += 1;
        }
        let shifted = row[col::Q_NEXT] + F::from_i64(Q_OFFSET);
        if let Some(n) = rq.get_mut(shifted.as_canonical_u64() as usize) {
            *n += 1;
        }
    }
    for (r, row) in trace.values.chunks_exact_mut(width).enumerate() {
        row[col::RS_MULT] = F::from_u64(rs.get(r).copied().unwrap_or(0));
        row[col::RQ_MULT] = F::from_u64(rq.get(r).copied().unwrap_or(0));
    }
}

/// Prover setup (preprocessed commitment + packed lookups). Depends only on `params`, so it can be
/// reused for every claim of the same shape; `common` is what the verifier needs.
pub fn setup<SC>(config: &SC, air: &ToyAir<Val<SC>>) -> ProverData<SC>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField64,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SC::Challenge>,
{
    let log_height = air.params.height().trailing_zeros() as usize;
    ProverData::from_airs_and_degrees(config, core::slice::from_ref(air), &[log_height])
        .unwrap_or_else(|e| panic!("toy setup failed: {e}"))
}

/// Like [`setup`], but folds same-bus lookups into shared LogUp columns up to fraction-pin degree
/// `lookup_degree` (fewer permutation columns, possibly more quotient chunks; must stay within
/// `2^log_blowup` chunks).
pub fn setup_with_lookup_budget<SC>(
    config: &SC,
    air: &ToyAir<Val<SC>>,
    lookup_degree: usize,
    log_blowup: usize,
) -> ProverData<SC>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField64,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SC::Challenge>,
{
    let log_height = air.params.height().trailing_zeros() as usize;
    ProverData::from_airs_and_degrees_with_lookup_budgets(
        config,
        core::slice::from_ref(air),
        &[log_height],
        &[lookup_degree],
        log_blowup,
    )
    .unwrap_or_else(|e| panic!("toy setup failed: {e}"))
}

/// Prove one claim. In debug builds Plonky3 checks constraints and bus balance first and panics on
/// a bad witness.
pub fn prove<SC>(
    config: &SC,
    air: &ToyAir<Val<SC>>,
    trace: &RowMajorMatrix<Val<SC>>,
    prover_data: &ProverData<SC>,
) -> Result<BatchProof<SC>, ProvingError<PcsProverError<SC>>>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField64,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SC::Challenge>,
    Domain<SC>: Send + Sync,
    SC::Pcs: Sync,
    PcsProverError<SC>: Send,
    SC::Challenger: GrindingChallenger<Witness = Val<SC>>,
    <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::ProverData: Sync,
    <SC::Pcs as Pcs<SC::Challenge, SC::Challenger>>::Commitment: Sync,
{
    let instance = StarkInstance { air, trace, public_values: air.public_values() };
    prove_batch(config, &[instance], prover_data)
}

/// Verify one claim; `air` must be rebuilt by the verifier from its own `(X_in, X_out)`.
pub fn verify<SC>(
    config: &SC,
    air: &ToyAir<Val<SC>>,
    proof: &BatchProof<SC>,
    prover_data: &ProverData<SC>,
) -> Result<(), BatchVerificationError<PcsError<SC>>>
where
    SC: StarkGenericConfig,
    Val<SC>: PrimeField64,
    SymbolicExpressionExt<Val<SC>, SC::Challenge>: Algebra<SC::Challenge>,
    SC::Challenger: GrindingChallenger<Witness = Val<SC>>,
{
    verify_batch(config, core::slice::from_ref(air), proof, &[air.public_values()], &prover_data.common)
}

/// Serialized proof size in bytes (postcard).
pub fn proof_size<SC: StarkGenericConfig>(proof: &BatchProof<SC>) -> usize {
    postcard::to_allocvec(proof).expect("proof serializes").len()
}

/// Deterministic pseudo-random input in `[-2^(Q-1), 2^(Q-1))` (splitmix64).
pub fn sample_input(n_cells: usize, seed: u64) -> Vec<i64> {
    let mut s = seed;
    (0..n_cells)
        .map(|_| {
            s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^= z >> 31;
            (z % (1 << Q_BITS)) as i64 - Q_OFFSET
        })
        .collect()
}

/// A complete honest instance: AIR + trace.
pub fn honest_instance<F: PrimeField64>(params: ToyParams, seed: u64) -> (ToyAir<F>, RowMajorMatrix<F>) {
    let x_in = sample_input(params.n_cells, seed);
    let states = run_reference(&x_in, params.n_steps);
    let air = ToyAir::new(params, &x_in, &states[params.n_steps]);
    (air, generate_trace(&params, &states))
}
