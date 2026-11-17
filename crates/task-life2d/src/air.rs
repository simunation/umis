//! The life2d transition AIR (Plonky3 0.8 uni-STARK, KoalaBear) and its proof
//! backend.
//!
//! A proof covers one advancing zone slot: `k` generations of Conway's rule
//! on the zone's extended board `E` (`(n+2k)²`: the zone plus the `k`-wide
//! halo from its 8 neighbours), with absent halo regions forced dead. Hold
//! and birth slots copy or reset the board; the verifier checks them natively
//! and their "proof" is the single byte `0` (see [`ProofTag`]).
//!
//! # Why this layout (no lookups, no memory bus)
//!
//! A cell's next state depends on 9 cells of the previous generation: 3 in its
//! own row and 3 in each adjacent row. Put **whole board rows in one trace
//! row** and **generations side by side in columns**: then the row above and
//! below are in the same or the next trace row, and every constraint is a
//! local two-row constraint. No permutation argument or LogUp bus is needed
//! (the water AIR spends most of its columns and its extension-field work on
//! them), so the prover commits only the cells and one inverse witness per
//! cell and generation.
//!
//! - `E` is cut into `S` vertical **strips** of `w = n/S` inner columns plus
//!   `k` halo columns on each side (`Ws = w + 2k`). Strips are stacked one
//!   above the other: stacked row `y` is row `y mod Y` of strip `⌊y/Y⌋`,
//!   `Y = n + 2k`. The stack (padded with dead rows) is a vertical torus of
//!   `R = rpr·H` rows, `rpr` rows per trace row, `H` a power of two.
//! - Generation `t` of trace row `r`, slot `i < rpr`, is stacked row
//!   `rpr·r + t + i` (each generation is shifted down one row, so the three
//!   rows a cell of generation `t+1` reads are slots `i, i+1, i+2` of
//!   generation `t`, wrapping into the next trace row).
//! - **Light cone:** generation `t` is only computed on columns
//!   `[t, Ws − t)`; generation `k` on exactly the `w` inner columns. Halo and
//!   strip-boundary garbage (the torus seam, rows of the neighbouring strip)
//!   never reaches the inner region, whose rows are at distance ≥ `k` from
//!   every strip edge.
//!
//! # Columns
//!
//! - main: generation 0 (`rpr × Ws` cells, committed so that the verifier
//!   only needs a few packed limbs per row), then for every generation
//!   `t = 1..=k`, slot `i`, column `x` of the cone: `a` (the cell) and `v` (an
//!   inverse witness);
//! - periodic (evaluated by the verifier, one value per trace row):
//!   `x0[i][q]` (generation-0 cells of slot `i`, bit-packed 30 per limb),
//!   `out[i][q]` (the claimed `X_out` row, packed likewise), `sel[i]` (slot
//!   `i` of generation `k` is an inner row), and `mask[o][c]` (the cell at
//!   stacked offset `o` and column class `c` is in an absent halo region);
//! - public: 16 × 16-bit limbs of a keccak digest binding the claim (periodic
//!   columns are not absorbed into Fiat–Shamir).
//!
//! Few periodic columns matter: the prover extends each one with its own DFT
//! and the verifier interpolates each one at the out-of-domain point, both
//! per column. Packing generation 0 and `X_out` and collapsing the mask to
//! three column classes cut them from `O(rpr·Ws + k·S)` to
//! `O(rpr·Ws/30 + k)`.
//!
//! # Constraints (degree ≤ 3, no transition gating: they hold on the torus)
//!
//! With `c` the live-neighbour count (a linear sum of the 8 cells of the
//! previous generation), `a` the previous cell, and `m ∈ {0,1}` the mask
//! (periodic; the verifier folds the public `absent` flags into it):
//!
//! ```text
//! b·(b − 1) = 0                        (generation-0 cells, b)
//! Σ_j 2^j·b_{30q+j} = x0[i][q]         (generation 0 equals the claim)
//! f   = (c − 3)·(c − 3 + a) + m        (≥ 0 over the integers; 0 ⇔ alive next)
//! a'·f = 0
//! f·v  = 1 − a'
//! sel·(Σ_j 2^j·a_k[k + 30q + j] − out[i][q]) = 0
//! ```
//!
//! `(c−3)(c−3+a)` has roots `c = 3` when dead and `c ∈ {2, 3}` when alive:
//! exactly Conway's B3/S23. It is a non-negative integer below 31, so adding
//! the mask keeps it non-zero on masked cells, and nothing wraps mod p. If
//! `f ≠ 0` the first constraint forces `a' = 0` (and `v = 1/f`); if `f = 0`
//! the second forces `a' = 1`. So `a'` is boolean and unique, given boolean
//! generation-0 cells. A sum of at most 30 bits is below `2^30 < p`, so a
//! packed limb fixes its bits.

use crate::board::{Board, packed_len};
use crate::{Ctx, Decision, Life2d, Life2dParams, PORTS, State, apply, decide, decode_state, halo_region};
use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::{Field, PrimeCharacteristicRing};
use p3_matrix::dense::RowMajorMatrix;
use p3_maybe_rayon::prelude::*;
use p3_uni_stark::{prove, verify};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use umis_core::records::TransitionStatement;
use umis_core::{Hash, NodeId, SecretKey, SegRecord, bmt, keccak256_concat};
use umis_proof::{ProofBackend, ProofError, PublicData, Witness};
use umis_stark::config::FriPreset;
use umis_stark::config::koala_bear::Val;
use umis_stark::config::{koala_bear, koala_bear_keccak};
use umis_task::topology::port_dir;
use umis_task::{Bytes, PortId, ZoneTask};

pub const BACKEND_ID: &str = "plonky3";
const DIGEST_LIMBS: usize = 16;
pub const N_PUBLIC: usize = DIGEST_LIMBS;
/// Cells per packed periodic limb (`2^30 < p`, KoalaBear).
const LIMB_BITS: usize = 30;

/// First byte of a life2d proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ProofTag {
    /// A hold or birth slot: no STARK, the verifier checks it natively.
    Native = 0,
    /// An advancing slot: a postcard-encoded uni-STARK proof follows.
    Stark = 1,
    /// An advancing slot proved in a batch with other slots of its proof
    /// commitment: the member statement addresses and the batch STARK follow
    /// (the head is its own first member).
    BatchHead = 2,
    /// Another member of a batch: the head's keccak hash and the member index.
    BatchRef = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    pub n: usize,
    pub k: usize,
    pub strips: usize,
    pub rpr: usize,
    /// Inner columns per strip.
    pub w: usize,
    /// Columns per strip, halo included.
    pub ws: usize,
    /// Rows per strip, halo included.
    pub y: usize,
    /// Trace height.
    pub height: usize,
}

impl Shape {
    pub fn of(p: &Life2dParams) -> Self {
        let (n, k, strips, rpr) = (p.n(), p.k(), p.strips as usize, p.rows_per_trace_row as usize);
        let w = n / strips;
        let y = n + 2 * k;
        let height = (strips * y).div_ceil(rpr).next_power_of_two().max(4);
        Self { n, k, strips, rpr, w, ws: w + 2 * k, y, height }
    }

    /// Torus rows.
    pub fn rows(&self) -> usize {
        self.rpr * self.height
    }

    /// Cone width of generation `t ≥ 1`.
    pub fn cone(&self, t: usize) -> usize {
        self.ws - 2 * t
    }

    /// First main column of generation `t ≥ 1` (generation 0 comes first).
    pub fn base(&self, t: usize) -> usize {
        self.rpr * self.ws + (1..t).map(|u| 2 * self.rpr * self.cone(u)).sum::<usize>()
    }

    pub fn width(&self) -> usize {
        self.base(self.k + 1)
    }

    /// Main column of generation-0 cell at slot `i`, strip column `x`.
    pub fn col0(&self, i: usize, x: usize) -> usize {
        i * self.ws + x
    }

    /// Main column of cell `a` (`+1`: its `v`) at generation `t ≥ 1`, slot `i`,
    /// strip column `x` (inside the cone).
    pub fn col(&self, t: usize, i: usize, x: usize) -> usize {
        debug_assert!(t >= 1 && x >= t && x < self.ws - t);
        self.base(t) + 2 * (i * self.cone(t) + x - t)
    }

    /// Packed limbs of a generation-0 row (`Ws` cells) and of an output row (`w`).
    fn limbs0(&self) -> usize {
        self.ws.div_ceil(LIMB_BITS)
    }
    fn limbs_out(&self) -> usize {
        self.w.div_ceil(LIMB_BITS)
    }

    /// Number of mask offsets `o = t + i` for generations `1..=k`.
    fn offsets(&self) -> usize {
        self.k + self.rpr
    }

    // periodic column indices
    fn p_x0(&self, i: usize, q: usize) -> usize {
        i * self.limbs0() + q
    }
    fn p_out(&self, i: usize, q: usize) -> usize {
        self.rpr * self.limbs0() + i * self.limbs_out() + q
    }
    fn p_sel(&self, i: usize) -> usize {
        self.rpr * (self.limbs0() + self.limbs_out()) + i
    }
    /// Mask at stacked-row offset `o` (`1 ≤ o < k + rpr`), column class `c`.
    fn p_mask(&self, o: usize, c: usize) -> usize {
        self.rpr * (self.limbs0() + self.limbs_out() + 1) + (o - 1) * 3 + c
    }
    pub fn num_periodic(&self) -> usize {
        self.p_mask(self.offsets(), 0)
    }

    /// Exact number of constraints [`LifeAir::eval`] asserts.
    pub fn num_constraints(&self) -> usize {
        let cells: usize = (1..=self.k).map(|t| self.cone(t)).sum();
        self.rpr * (self.ws + self.limbs0() + 2 * cells + self.limbs_out())
    }

    /// `(strip, row in the strip)` of a stacked row, `None` in the padding.
    fn locate(&self, y: usize) -> Option<(usize, usize)> {
        let y = y % self.rows();
        (y < self.strips * self.y).then(|| (y / self.y, y % self.y))
    }

    /// Column class of strip column `x`: 0 left halo, 1 inner, 2 right halo.
    fn class(&self, x: usize) -> usize {
        if x < self.k {
            0
        } else if x < self.w + self.k {
            1
        } else {
            2
        }
    }

    /// Column band (0, 1, 2) on the extended board of strip `s`'s columns of
    /// class `c`. Exact because `k ≤ w` (`validate_params`): only strip 0
    /// reaches the left halo and only the last strip the right one.
    fn class_band(&self, s: usize, c: usize) -> usize {
        match c {
            0 if s == 0 => 0,
            2 if s + 1 == self.strips => 2,
            _ => 1,
        }
    }

    fn row_band(&self, yy: usize) -> usize {
        if yy < self.k {
            0
        } else if yy < self.k + self.n {
            1
        } else {
            2
        }
    }

    /// Mask of stacked row `y` for each column class.
    fn row_mask(&self, absent: &[bool; 8], y: usize) -> [bool; 3] {
        match self.locate(y) {
            Some((st, yy)) => {
                let by = self.row_band(yy);
                [0, 1, 2].map(|c| band_absent(absent, by, self.class_band(st, c)))
            }
            None => [false; 3],
        }
    }
}

/// The `absent` flag of extended-board band `(row band, column band)` (the
/// centre band is the zone itself), from the per-port flags.
fn band_absent(absent: &[bool; 8], by: usize, bx: usize) -> bool {
    PORTS.iter().enumerate().any(|(pi, p)| {
        let d = port_dir(*p).expect("port");
        absent[pi] && (d[1] + 1) as usize == by && (d[0] + 1) as usize == bx
    })
}

/// The AIR of one claim; `periodic` comes from [`claim_data`].
#[derive(Clone)]
pub struct LifeAir {
    pub shape: Shape,
    periodic: Vec<Vec<Val>>,
}

impl BaseAir<Val> for LifeAir {
    fn width(&self) -> usize {
        self.shape.width()
    }

    fn num_periodic_columns(&self) -> usize {
        self.shape.num_periodic()
    }

    fn periodic_columns(&self) -> Cow<'_, [Vec<Val>]> {
        Cow::Borrowed(&self.periodic)
    }

    fn num_public_values(&self) -> usize {
        N_PUBLIC
    }

    fn num_constraints(&self) -> Option<usize> {
        Some(self.shape.num_constraints())
    }

    fn max_constraint_degree(&self) -> Option<usize> {
        Some(3)
    }
}

impl<AB: AirBuilder<F = Val>> Air<AB> for LifeAir {
    fn eval(&self, builder: &mut AB) {
        let s = self.shape;
        let main = builder.main();
        let (cur, next) = (main.current_slice(), main.next_slice());
        let per: Vec<AB::Expr> = builder.periodic_values().iter().map(|x| (*x).into()).collect();
        let three = AB::Expr::from_u64(3);
        let one = AB::Expr::ONE;
        let bases: Vec<usize> = (0..=s.k).map(|t| if t == 0 { 0 } else { s.base(t) }).collect();
        // Cell of generation t at slot j (0 ≤ j < rpr + 2; j ≥ rpr is the next
        // trace row), strip column x.
        let cell = |t: usize, j: usize, x: usize| -> AB::Expr {
            let (row, j) = if j < s.rpr { (cur, j) } else { (next, j - s.rpr) };
            let c = if t == 0 { s.col0(j, x) } else { bases[t] + 2 * (j * s.cone(t) + x - t) };
            row[c].into()
        };
        // Σ_j 2^j·cells[j] (Horner, doublings only).
        let pack = |cells: &mut dyn DoubleEndedIterator<Item = AB::Expr>| -> AB::Expr {
            cells.rev().fold(AB::Expr::ZERO, |acc, b| acc.double() + b)
        };
        // Generation 0: boolean, and equal to the claimed extended board.
        for i in 0..s.rpr {
            for x in 0..s.ws {
                let b = cell(0, i, x);
                builder.assert_zero(b.clone() * (b - one.clone()));
            }
            for q in 0..s.limbs0() {
                let xs = q * LIMB_BITS..((q + 1) * LIMB_BITS).min(s.ws);
                let packed = pack(&mut xs.map(|x| cell(0, i, x)));
                builder.assert_zero(packed - per[s.p_x0(i, q)].clone());
            }
        }
        for t in 0..s.k {
            for i in 0..s.rpr {
                let masks = [0, 1, 2].map(|c| per[s.p_mask(t + 1 + i, c)].clone());
                for x in (t + 1)..(s.ws - t - 1) {
                    let mut c = AB::Expr::ZERO;
                    for j in 0..3 {
                        for dx in 0..3 {
                            if j != 1 || dx != 1 {
                                c += cell(t, i + j, x + dx - 1);
                            }
                        }
                    }
                    let a = cell(t, i + 1, x);
                    let f = (c.clone() - three.clone()) * (c - three.clone() + a) + masks[s.class(x)].clone();
                    let col = bases[t + 1] + 2 * (i * s.cone(t + 1) + x - t - 1);
                    let an: AB::Expr = cur[col].into();
                    let v: AB::Expr = cur[col + 1].into();
                    builder.assert_zero(an.clone() * f.clone());
                    builder.assert_zero(f * v - (one.clone() - an));
                }
            }
        }
        // Generation k: the inner columns of inner rows equal the claimed X_out.
        for i in 0..s.rpr {
            for q in 0..s.limbs_out() {
                let xs = q * LIMB_BITS..((q + 1) * LIMB_BITS).min(s.w);
                let packed = pack(&mut xs.map(|x| cell(s.k, i, s.k + x)));
                builder.assert_zero(per[s.p_sel(i)].clone() * (packed - per[s.p_out(i, q)].clone()));
            }
        }
    }
}

/// Generation-0 value of stacked row `y`, strip column `x`.
fn ext_value(s: &Shape, ext: &Board, y: usize, x: usize) -> u8 {
    match s.locate(y) {
        Some((st, yy)) => ext.cells[yy * ext.w + st * s.w + x],
        None => 0,
    }
}

/// `Σ_j 2^j·bits[j]` as a field element (`bits.len() ≤ 30`).
fn pack_limb(bits: impl DoubleEndedIterator<Item = u8>) -> Val {
    Val::from_u32(bits.rev().fold(0u32, |acc, b| (acc << 1) | b as u32))
}

/// Periodic columns and public values of a claim.
pub fn claim_data(
    s: &Shape,
    params_hash: &Hash,
    slot: u64,
    ext: &Board,
    absent: &[bool; 8],
    x_out: &Board,
) -> (Vec<Vec<Val>>, Vec<Val>) {
    let h = s.height;
    let mut per = vec![vec![Val::ZERO; h]; s.num_periodic()];
    for r in 0..h {
        for i in 0..s.rpr {
            let y = s.rpr * r + i;
            for q in 0..s.limbs0() {
                let xs = q * LIMB_BITS..((q + 1) * LIMB_BITS).min(s.ws);
                per[s.p_x0(i, q)][r] = pack_limb(xs.map(|x| ext_value(s, ext, y, x)));
            }
            // generation k, slot i: stacked row rpr·r + k + i
            if let Some((st, yy)) = s.locate(y + s.k)
                && s.row_band(yy) == 1
            {
                per[s.p_sel(i)][r] = Val::ONE;
                let row = &x_out.cells[(yy - s.k) * s.n + st * s.w..][..s.w];
                for q in 0..s.limbs_out() {
                    let xs = q * LIMB_BITS..((q + 1) * LIMB_BITS).min(s.w);
                    per[s.p_out(i, q)][r] = pack_limb(xs.map(|x| row[x]));
                }
            }
        }
        for o in 1..s.offsets() {
            let m = s.row_mask(absent, s.rpr * r + o);
            for c in 0..3 {
                per[s.p_mask(o, c)][r] = Val::from_bool(m[c]);
            }
        }
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&params_hash.0);
    bytes.extend_from_slice(&slot.to_be_bytes());
    bytes.extend_from_slice(&crate::board::pack(&ext.cells));
    bytes.extend_from_slice(&crate::board::pack(&x_out.cells));
    bytes.extend(absent.iter().map(|a| *a as u8));
    let digest = keccak256_concat(&[b"life2d-air", &bytes]);
    let public: Vec<Val> =
        (0..DIGEST_LIMBS).map(|i| Val::from_u16(u16::from_be_bytes([digest.0[2 * i], digest.0[2 * i + 1]]))).collect();
    (per, public)
}

/// The main trace of the honest transition, and the inner board after `k`
/// generations. Each generation is computed row-parallel on the stacked
/// torus, then trace rows are filled in parallel.
pub fn generate_trace(s: &Shape, ext: &Board, absent: &[bool; 8]) -> (RowMajorMatrix<Val>, Board) {
    let (rows, ws) = (s.rows(), s.ws);
    // gens[t][y·ws + x] (cell) and fs[t][y·ws + x] (f of generation t's cell).
    let mut gens: Vec<Vec<u8>> = Vec::with_capacity(s.k + 1);
    let mut fs: Vec<Vec<u8>> = vec![Vec::new()];
    let mut g0 = vec![0u8; rows * ws];
    g0.par_chunks_mut(ws).enumerate().for_each(|(y, row)| {
        for (x, c) in row.iter_mut().enumerate() {
            *c = ext_value(s, ext, y, x);
        }
    });
    gens.push(g0);
    for t in 0..s.k {
        let g = &gens[t];
        let mut ng = vec![0u8; rows * ws];
        let mut nf = vec![0u8; rows * ws];
        ng.par_chunks_mut(ws).zip(nf.par_chunks_mut(ws)).enumerate().for_each(|(y, (row, frow))| {
            let m = s.row_mask(absent, y);
            let (up, mid, down) =
                (&g[((y + rows - 1) % rows) * ws..][..ws], &g[y * ws..][..ws], &g[((y + 1) % rows) * ws..][..ws]);
            for x in (t + 1)..(ws - t - 1) {
                let c = (up[x - 1] + up[x] + up[x + 1] + mid[x - 1] + mid[x + 1] + down[x - 1] + down[x] + down[x + 1])
                    as i32;
                let a = mid[x] as i32;
                let f = (c - 3) * (c - 3 + a) + m[s.class(x)] as i32;
                row[x] = (f == 0) as u8;
                frow[x] = f as u8;
            }
        });
        gens.push(ng);
        fs.push(nf);
    }
    let width = s.width();
    // f is a small non-negative integer (< 32): inverses from a table.
    let inv: Vec<Val> = (0..32u64).map(|f| if f == 0 { Val::ZERO } else { Val::from_u64(f).inverse() }).collect();
    let mut v = Val::zero_vec(s.height * width);
    v.par_chunks_mut(width).enumerate().for_each(|(r, trow)| {
        for i in 0..s.rpr {
            let y = (s.rpr * r + i) % rows;
            for x in 0..ws {
                trow[s.col0(i, x)] = Val::from_u8(gens[0][y * ws + x]);
            }
        }
        for t in 1..=s.k {
            let base = s.base(t);
            for i in 0..s.rpr {
                // generation t, slot i: stacked row rpr·r + t + i
                let y = (s.rpr * r + t + i) % rows;
                let (g, f) = (&gens[t][y * ws..][..ws], &fs[t][y * ws..][..ws]);
                let start = base + 2 * i * s.cone(t);
                for x in t..ws - t {
                    let col = start + 2 * (x - t);
                    trow[col] = Val::from_u8(g[x]);
                    trow[col + 1] = inv[f[x] as usize];
                }
            }
        }
    });
    // Inner board: generation k at stacked rows (strip st, yy ∈ [k, k+n)).
    let mut out = Board::dead(s.n, s.n);
    let gk = &gens[s.k];
    for st in 0..s.strips {
        for yy in s.k..s.k + s.n {
            for x in 0..s.w {
                out.cells[(yy - s.k) * s.n + st * s.w + x] = gk[(st * s.y + yy) * ws + s.k + x];
            }
        }
    }
    (RowMajorMatrix::new(v, width), out)
}

struct ZoneSetup {
    ctx: Ctx,
    shape: Shape,
    params_hash: Hash,
}

/// The Plonky3 backend for life2d zones (per zone it caches the task context).
pub struct Life2dStark {
    preset: FriPreset,
    config: StarkCfg,
    /// Keccak config only: prove on the GPU (`gpu::GpuProver`, device-resident; same
    /// proof format, verified with `config`). A local prover choice, not part of the genesis.
    #[cfg(feature = "cuda")]
    gpu: Option<gpu::GpuProver>,
    setups: Mutex<HashMap<(Hash, Hash), Arc<ZoneSetup>>>,
    /// Prove a commitment's advancing slots as one batch STARK (default on).
    batch: bool,
}

/// The STARK configuration: the same field, FRI and AIR; Merkle and transcript
/// hash differ (see `umis_stark::config`).
pub enum StarkCfg {
    /// Poseidon2 (width 16) Merkle tree and duplex transcript.
    Poseidon2(koala_bear::Config),
    /// Keccak-f Merkle tree (AVX2, 4 rows per permutation), Poseidon2 transcript.
    Keccak(koala_bear_keccak::Config),
}

/// Merkle/transcript hash of a [`StarkCfg`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MerkleHash {
    Poseidon2,
    Keccak,
}

/// Runs `$body` with `$c` bound to the concrete config (proof types differ per config).
macro_rules! with_cfg {
    ($cfg:expr, |$c:ident| $body:expr) => {
        match $cfg {
            StarkCfg::Poseidon2($c) => $body,
            StarkCfg::Keccak($c) => $body,
        }
    };
}

impl Life2dStark {
    pub fn new(preset: FriPreset) -> Self {
        Self::with_hash(preset, MerkleHash::Poseidon2)
    }

    pub fn with_hash(preset: FriPreset, hash: MerkleHash) -> Self {
        let config = match hash {
            MerkleHash::Poseidon2 => StarkCfg::Poseidon2(koala_bear::config(preset)),
            MerkleHash::Keccak => StarkCfg::Keccak(koala_bear_keccak::config(preset)),
        };
        Self {
            preset,
            config,
            #[cfg(feature = "cuda")]
            gpu: None,
            setups: Mutex::new(HashMap::new()),
            batch: std::env::var("UMIS_LIFE_BATCH").as_deref() != Ok("0"),
        }
    }

    /// Keccak config with the `cuda` feature: proves with the coset LDE on the GPU
    /// when `on` and a GPU is usable. Returns whether the GPU prover is active.
    pub fn use_gpu(&mut self, on: bool) -> bool {
        #[cfg(feature = "cuda")]
        {
            self.gpu = (on && matches!(self.config, StarkCfg::Keccak(_)) && umis_gpu_available())
                .then(|| gpu::GpuProver::new(self.preset));
            self.gpu.is_some()
        }
        #[cfg(not(feature = "cuda"))]
        {
            let _ = on;
            false
        }
    }

    /// From a genesis `stark_config_id`: `koalabear` (benchmark FRI, Poseidon2),
    /// `koalabear-keccak` (benchmark FRI, Keccak Merkle tree and transcript), and
    /// their `-test` variants (few queries, fast and insecure, for tests).
    pub fn from_config_id(id: Option<&str>) -> Result<Self, String> {
        match id.unwrap_or("koalabear") {
            "koalabear" => Ok(Self::new(FriPreset::Benchmark)),
            "koalabear-test" => Ok(Self::new(FriPreset::Test)),
            "koalabear-keccak" | "koalabear-keccak-test" => {
                let preset = if id == Some("koalabear-keccak") { FriPreset::Benchmark } else { FriPreset::Test };
                let mut b = Self::with_hash(preset, MerkleHash::Keccak);
                // UMIS_GPU_PROVE=1: this node proves with the GPU coset LDE (`cuda` builds).
                b.use_gpu(std::env::var("UMIS_GPU_PROVE").as_deref() == Ok("1"));
                Ok(b)
            }
            other => Err(format!("unknown stark_config_id {other}")),
        }
    }

    /// Whether zones are proved a commitment at a time (one batch STARK).
    pub fn set_batch(&mut self, on: bool) {
        self.batch = on;
    }

    pub fn preset(&self) -> FriPreset {
        self.preset
    }

    pub fn merkle_hash(&self) -> MerkleHash {
        match self.config {
            StarkCfg::Poseidon2(_) => MerkleHash::Poseidon2,
            StarkCfg::Keccak(_) => MerkleHash::Keccak,
        }
    }

    /// A tagged STARK proof (`ProofTag::Stark` ‖ postcard proof) of `trace`
    /// against an arbitrary claim (`air`, `public`); tests prove adversarial claims with it.
    pub fn prove_claim(&self, air: &LifeAir, trace: RowMajorMatrix<Val>, public: &[Val]) -> Result<Bytes, String> {
        let mut out = vec![ProofTag::Stark as u8];
        #[cfg(feature = "cuda")]
        if let Some(g) = &self.gpu {
            let proof = g.prove(air, trace, public)?;
            out.extend(postcard::to_allocvec(&proof).map_err(|e| e.to_string())?);
            return Ok(out);
        }
        with_cfg!(&self.config, |c| {
            let proof = prove(c, air, trace, public).map_err(|e| format!("{e:?}"))?;
            out.extend(postcard::to_allocvec(&proof).map_err(|e| e.to_string())?);
        });
        Ok(out)
    }

    fn setup(&self, params: &[u8], seed: &Hash) -> Result<Arc<ZoneSetup>, String> {
        let key = (umis_core::keccak256(params), *seed);
        if let Some(s) = self.setups.lock().expect("setups").get(&key) {
            return Ok(s.clone());
        }
        let p: Life2dParams = borsh::from_slice(params).map_err(|e| e.to_string())?;
        Life2d::validate_params(&p).map_err(|e| e.to_string())?;
        let ctx = Life2d::prepare(&p, seed).map_err(|e| e.to_string())?;
        let shape = Shape::of(&p);
        let params_hash = umis_task::params_hash(crate::TASK_ID, crate::LAYER, params);
        let s = Arc::new(ZoneSetup { ctx, shape, params_hash });
        self.setups.lock().expect("setups").insert(key, s.clone());
        Ok(s)
    }

    /// The advancing claim of a slot (AIR, honest trace, public values), or
    /// `None` for a hold/birth slot.
    fn claim_for_prove(
        &self,
        stmt: &TransitionStatement,
        w: &Witness<'_>,
    ) -> Result<Option<(LifeAir, RowMajorMatrix<Val>, Vec<Val>)>, ProofError> {
        let z = self.setup(w.params, &w.genesis_seed).map_err(ProofError::Prover)?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return Err(ProofError::Prover("statement must cover one slot of this zone".into()));
        }
        let s_in = decode_state(&z.ctx.p, w.prev_state).ok_or_else(|| ProofError::Prover("prev state".into()))?;
        let Decision::Advance { ext, absent } = decide(&z.ctx, &s_in, w.inputs, stmt.from_slot) else {
            return Ok(None);
        };
        let (trace, x_out) = generate_trace(&z.shape, &ext, &absent);
        let (per, public) = claim_data(&z.shape, &z.params_hash, stmt.from_slot, &ext, &absent, &x_out);
        Ok(Some((LifeAir { shape: z.shape, periodic: per }, trace, public)))
    }

    /// Proves with a hook that may alter the trace (tests tamper with it).
    pub fn prove_with(
        &self,
        stmt: &TransitionStatement,
        w: &Witness<'_>,
        tamper: impl FnOnce(&mut RowMajorMatrix<Val>),
    ) -> Result<Bytes, ProofError> {
        let z = self.setup(w.params, &w.genesis_seed).map_err(ProofError::Prover)?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return Err(ProofError::Prover("statement must cover one slot of this zone".into()));
        }
        let s_in = decode_state(&z.ctx.p, w.prev_state).ok_or_else(|| ProofError::Prover("prev state".into()))?;
        let Decision::Advance { ext, absent } = decide(&z.ctx, &s_in, w.inputs, stmt.from_slot) else {
            return Ok(vec![ProofTag::Native as u8]);
        };
        let (mut trace, x_out) = generate_trace(&z.shape, &ext, &absent);
        tamper(&mut trace);
        let (per, public) = claim_data(&z.shape, &z.params_hash, stmt.from_slot, &ext, &absent, &x_out);
        let air = LifeAir { shape: z.shape, periodic: per };
        self.prove_claim(&air, trace, &public).map_err(ProofError::Prover)
    }

    /// The native part of verification: roots, canonical states, outputs,
    /// inputs, the decision and every header field. Hold and birth slots are
    /// fully checked here (`proof` must be `[ProofTag::Native]`); an advancing
    /// slot returns the STARK claim (AIR and public values) left to check.
    fn check_claim(
        &self,
        stmt: &TransitionStatement,
        pd: &PublicData<'_>,
        proof: &[u8],
    ) -> Result<Option<(LifeAir, Vec<Val>, usize)>, ProofError> {
        let bad = |m: &str| Err(ProofError::Invalid(m.into()));
        let (Some(params), Some(seed), Some(prev), Some(next), Some(inputs), Some(refs)) =
            (pd.params, pd.genesis_seed, pd.prev_state, pd.next_state, pd.inputs, pd.input_refs)
        else {
            return bad("life2d verification needs the states, inputs and InputRefs");
        };
        let z = self.setup(params, &seed).map_err(ProofError::Invalid)?;
        if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
            return bad("statement does not match the zone");
        }
        if bmt::file_address(prev) != stmt.prev_state_root || bmt::file_address(next) != stmt.state_root {
            return bad("states do not match the statement roots");
        }
        // Canonical decoding: one encoding per state (INV-LIFE-CANON).
        let (Some(s_in), Some(s_out)) = (decode_state(&z.ctx.p, prev), decode_state(&z.ctx.p, next)) else {
            return bad("non-canonical state");
        };
        let outs = <Life2d as ZoneTask>::outputs(&z.ctx, &s_out);
        if outputs_root(&outs) != stmt.outputs_root {
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
        let Decision::Advance { ext, absent } = decision else {
            // Hold / birth: fully determined, checked natively.
            if proof != [ProofTag::Native as u8] || s_out != expect {
                return bad("hold/birth slot does not match");
            }
            return Ok(None);
        };
        // Everything but the board is native; the board is the STARK claim.
        if !header_matches(&s_out, &expect) {
            return bad("advanced state header/prev board");
        }
        let (per, public) = claim_data(&z.shape, &z.params_hash, stmt.from_slot, &ext, &absent, &s_out.grid);
        Ok(Some((LifeAir { shape: z.shape, periodic: per }, public, z.shape.height.trailing_zeros() as usize)))
    }

    fn verify_inner(
        &self,
        stmt: &TransitionStatement,
        pd: &PublicData<'_>,
        proof: &[u8],
    ) -> Result<NodeId, ProofError> {
        let Some((air, public, _)) = self.check_claim(stmt, pd, proof)? else { return Ok(NodeId::default()) };
        let Some((&tag, body)) = proof.split_first() else { return Err(ProofError::Invalid("empty proof".into())) };
        if tag == ProofTag::BatchHead as u8 || tag == ProofTag::BatchRef as u8 {
            return Err(ProofError::Invalid("a batched proof verifies only with its group".into()));
        }
        if tag != ProofTag::Stark as u8 {
            return Err(ProofError::Invalid("advancing slot needs a STARK".into()));
        }
        with_cfg!(&self.config, |c| {
            let proof = postcard::from_bytes(body).map_err(|e| ProofError::Malformed(e.to_string()))?;
            verify(c, &air, &proof, &public).map_err(|e| ProofError::Invalid(format!("{e:?}")))?;
        });
        Ok(NodeId::default())
    }

    /// One batch STARK over several advancing claims (INV-LIFE-BATCH-1).
    fn prove_claims_batch(
        &self,
        airs: &[LifeAir],
        traces: Vec<RowMajorMatrix<Val>>,
        publics: &[Vec<Val>],
    ) -> Result<Vec<u8>, String> {
        use p3_batch_stark::{ProverData, StarkInstance, prove_batch};
        let degrees: Vec<usize> = airs.iter().map(|a| a.shape.height.trailing_zeros() as usize).collect();
        let tr: Vec<&RowMajorMatrix<Val>> = traces.iter().collect();
        macro_rules! go {
            ($c:expr) => {{
                let c = $c;
                let pd = ProverData::from_airs_and_degrees(c, airs, &degrees).map_err(|e| format!("{e:?}"))?;
                let inst = StarkInstance::new_multiple(airs, &tr, publics);
                let proof = prove_batch(c, &inst, &pd).map_err(|e| format!("{e:?}"))?;
                postcard::to_allocvec(&proof).map_err(|e| e.to_string())
            }};
        }
        with_cfg!(&self.config, |c| go!(c))
    }

    fn verify_claims_batch(
        &self,
        airs: &[LifeAir],
        degrees: &[usize],
        body: &[u8],
        publics: &[Vec<Val>],
    ) -> Result<(), ProofError> {
        use p3_batch_stark::{ProverData, verify_batch};
        with_cfg!(&self.config, |c| {
            let pd = ProverData::from_airs_and_degrees(c, airs, degrees)
                .map_err(|e| ProofError::Invalid(format!("{e:?}")))?;
            let proof = postcard::from_bytes(body).map_err(|e| ProofError::Malformed(e.to_string()))?;
            verify_batch(c, airs, &proof, publics, &pd.common).map_err(|e| ProofError::Invalid(format!("{e:?}")))
        })
    }

    /// Verifies a batch head (tag 2) and its references (tag 3) found in `items`.
    /// Returns the item indices it covers, all verified.
    fn verify_batch_group(
        &self,
        items: &[(&TransitionStatement, PublicData<'_>, &[u8])],
        head: usize,
    ) -> Result<Vec<usize>, ProofError> {
        let bad = |m: &str| Err(ProofError::Invalid(m.into()));
        let (members, body) = parse_batch_head(items[head].2)?;
        let head_hash = umis_core::keccak256(items[head].2);
        if items[head].0.address() != members[0] {
            return bad("batch head is not its own first member");
        }
        let mut idx = vec![head];
        for (k, m) in members.iter().enumerate().skip(1) {
            let want = batch_ref(&head_hash, k);
            match items.iter().position(|(s, _, p)| s.address() == *m && *p == want.as_slice()) {
                Some(i) => idx.push(i),
                None => return bad("batch member missing from the group"),
            }
        }
        let (mut airs, mut publics, mut degrees) = (Vec::new(), Vec::new(), Vec::new());
        for &i in &idx {
            let (s, pd, p) = items[i];
            let Some((air, public, deg)) = self.check_claim(s, &pd, p)? else {
                return bad("batch member is not an advancing slot");
            };
            airs.push(air);
            publics.push(public);
            degrees.push(deg);
        }
        self.verify_claims_batch(&airs, &degrees, body, &publics)?;
        Ok(idx)
    }
}

/// `ProofTag::BatchHead ‖ count (u16 BE) ‖ count member statement addresses ‖ postcard batch proof`.
fn batch_head(members: &[Hash], body: &[u8]) -> Bytes {
    let mut out = vec![ProofTag::BatchHead as u8];
    out.extend((members.len() as u16).to_be_bytes());
    for m in members {
        out.extend_from_slice(&m.0);
    }
    out.extend_from_slice(body);
    out
}

fn parse_batch_head(p: &[u8]) -> Result<(Vec<Hash>, &[u8]), ProofError> {
    let bad = || ProofError::Malformed("batch head".into());
    let (&tag, rest) = p.split_first().ok_or_else(bad)?;
    if tag != ProofTag::BatchHead as u8 || rest.len() < 2 {
        return Err(bad());
    }
    let n = u16::from_be_bytes([rest[0], rest[1]]) as usize;
    if n < 2 || rest.len() < 2 + 32 * n {
        return Err(bad());
    }
    let members = (0..n).map(|k| Hash(rest[2 + 32 * k..2 + 32 * (k + 1)].try_into().expect("32"))).collect();
    Ok((members, &rest[2 + 32 * n..]))
}

/// `ProofTag::BatchRef ‖ keccak(head proof bytes) ‖ member index (u16 BE)`.
fn batch_ref(head_hash: &Hash, index: usize) -> Bytes {
    let mut out = vec![ProofTag::BatchRef as u8];
    out.extend_from_slice(&head_hash.0);
    out.extend((index as u16).to_be_bytes());
    out
}

fn header_matches(a: &State, b: &State) -> bool {
    a.born == b.born
        && a.generation == b.generation
        && a.birth == b.birth
        && a.next_slot == b.next_slot
        && a.start_slot == b.start_slot
        && a.prev == b.prev
}

/// `outputs_root` over port payloads (same as the protocol's).
fn outputs_root(outputs: &BTreeMap<PortId, Bytes>) -> Hash {
    let leaves: Vec<Hash> = outputs.values().map(|p| bmt::file_address(p)).collect();
    bmt::list_root(&leaves)
}

#[cfg(feature = "cuda")]
impl Life2dStark {
    /// `prove_batch` on the GPU: advancing slots of one shape go into one batch STARK
    /// (head and references as on the CPU, INV-LIFE-BATCH-1..2); otherwise one proof each.
    fn prove_batch_gpu(
        &self,
        g: &gpu::GpuProver,
        items: &[(&TransitionStatement, Witness<'_>)],
        prover: &SecretKey,
    ) -> Result<Vec<Bytes>, ProofError> {
        let mut out: Vec<Option<Bytes>> = vec![None; items.len()];
        let (mut idx, mut claims, mut shapes) = (Vec::new(), Vec::new(), Vec::new());
        for (i, (stmt, w)) in items.iter().enumerate() {
            let z = self.setup(w.params, &w.genesis_seed).map_err(ProofError::Prover)?;
            if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
                return Err(ProofError::Prover("statement must cover one slot of this zone".into()));
            }
            let s_in = decode_state(&z.ctx.p, w.prev_state).ok_or_else(|| ProofError::Prover("prev state".into()))?;
            match decide(&z.ctx, &s_in, w.inputs, stmt.from_slot) {
                Decision::Advance { ext, absent } => {
                    idx.push(i);
                    shapes.push(z.shape);
                    claims.push((z.params_hash, stmt.from_slot, ext, absent));
                }
                _ => out[i] = Some(vec![ProofTag::Native as u8]),
            }
        }
        if idx.len() < 2 || !self.batch || shapes.iter().any(|s| *s != shapes[0]) {
            for &i in &idx {
                out[i] = Some(self.prove(items[i].0, &items[i].1, prover)?);
            }
        } else {
            let input: Vec<_> = claims.iter().map(|(h, sl, e, a)| (*h, *sl, e, *a)).collect();
            let proof = g.prove_ext_batch(&shapes[0], &input).map_err(ProofError::Prover)?;
            let body = postcard::to_allocvec(&proof).map_err(|e| ProofError::Prover(e.to_string()))?;
            let members: Vec<Hash> = idx.iter().map(|&i| items[i].0.address()).collect();
            let head = batch_head(&members, &body);
            let h = umis_core::keccak256(&head);
            for (k, &i) in idx.iter().enumerate() {
                out[i] = Some(if k == 0 { head.clone() } else { batch_ref(&h, k) });
            }
        }
        Ok(out.into_iter().map(|p| p.expect("every item")).collect())
    }
}

impl ProofBackend for Life2dStark {
    fn id(&self) -> &'static str {
        BACKEND_ID
    }

    fn is_async(&self) -> bool {
        true
    }

    fn prove(&self, stmt: &TransitionStatement, w: &Witness<'_>, _prover: &SecretKey) -> Result<Bytes, ProofError> {
        #[cfg(feature = "cuda")]
        if let Some(g) = &self.gpu {
            let z = self.setup(w.params, &w.genesis_seed).map_err(ProofError::Prover)?;
            if stmt.to_slot != stmt.from_slot + 1 || stmt.params_hash != z.params_hash {
                return Err(ProofError::Prover("statement must cover one slot of this zone".into()));
            }
            let s_in = decode_state(&z.ctx.p, w.prev_state).ok_or_else(|| ProofError::Prover("prev state".into()))?;
            let Decision::Advance { ext, absent } = decide(&z.ctx, &s_in, w.inputs, stmt.from_slot) else {
                return Ok(vec![ProofTag::Native as u8]);
            };
            let proof =
                g.prove_ext(&z.shape, &z.params_hash, stmt.from_slot, &ext, &absent).map_err(ProofError::Prover)?;
            let mut out = vec![ProofTag::Stark as u8];
            out.extend(postcard::to_allocvec(&proof).map_err(|e| ProofError::Prover(e.to_string()))?);
            return Ok(out);
        }
        self.prove_with(stmt, w, |_| {})
    }

    fn verify(&self, stmt: &TransitionStatement, pd: &PublicData<'_>, proof: &[u8]) -> Result<NodeId, ProofError> {
        self.verify_inner(stmt, pd, proof)
    }

    fn batches(&self, task_id: &str) -> bool {
        self.batch && task_id == crate::TASK_ID
    }

    fn prove_batch(
        &self,
        items: &[(&TransitionStatement, Witness<'_>)],
        prover: &SecretKey,
    ) -> Result<Vec<Bytes>, ProofError> {
        #[cfg(feature = "cuda")]
        if let Some(g) = &self.gpu {
            return self.prove_batch_gpu(g, items, prover);
        }
        let _ = prover;
        let mut out: Vec<Option<Bytes>> = vec![None; items.len()];
        let (mut idx, mut airs, mut traces, mut publics) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (i, (stmt, w)) in items.iter().enumerate() {
            match self.claim_for_prove(stmt, w)? {
                None => out[i] = Some(vec![ProofTag::Native as u8]),
                Some((air, trace, public)) => {
                    idx.push(i);
                    airs.push(air);
                    traces.push(trace);
                    publics.push(public);
                }
            }
        }
        if idx.len() == 1 || !self.batch {
            for (k, i) in idx.iter().enumerate() {
                out[*i] = Some(self.prove_claim(&airs[k], traces[k].clone(), &publics[k]).map_err(ProofError::Prover)?);
            }
        } else if idx.len() > 1 {
            let body = self.prove_claims_batch(&airs, traces, &publics).map_err(ProofError::Prover)?;
            let members: Vec<Hash> = idx.iter().map(|&i| items[i].0.address()).collect();
            let head = batch_head(&members, &body);
            let h = umis_core::keccak256(&head);
            for (k, &i) in idx.iter().enumerate() {
                out[i] = Some(if k == 0 { head.clone() } else { batch_ref(&h, k) });
            }
        }
        Ok(out.into_iter().map(|p| p.expect("every item")).collect())
    }

    fn verify_group(&self, items: &[(&TransitionStatement, PublicData<'_>, &[u8])]) -> Vec<Result<NodeId, ProofError>> {
        let mut out: Vec<Option<Result<NodeId, ProofError>>> = (0..items.len()).map(|_| None).collect();
        for (i, (s, pd, p)) in items.iter().enumerate() {
            match p.first() {
                Some(&t) if t == ProofTag::BatchHead as u8 => match self.verify_batch_group(items, i) {
                    Ok(idx) => {
                        for j in idx {
                            out[j] = Some(Ok(NodeId::default()));
                        }
                    }
                    Err(e) => {
                        if out[i].is_none() {
                            out[i] = Some(Err(e));
                        }
                    }
                },
                Some(&t) if t == ProofTag::BatchRef as u8 => {}
                _ => out[i] = Some(self.verify_inner(s, pd, p)),
            }
        }
        out.into_iter()
            .map(|r| r.unwrap_or_else(|| Err(ProofError::Invalid("batch reference without a verified head".into()))))
            .collect()
    }
}

/// Committed cells per advancing slot (main trace area), for cost reports.
pub fn trace_cells(p: &Life2dParams) -> usize {
    let s = Shape::of(p);
    s.width() * s.height
}

/// Bytes of the generation-0 data the verifier rebuilds (diagnostics).
pub fn claim_bytes(p: &Life2dParams) -> usize {
    packed_len((p.n() + 2 * p.k()).pow(2)) + packed_len(p.cells())
}

/// The region a port's halo occupies on the extended board (re-export for tests).
pub fn halo(p: &Life2dParams, pi: usize) -> ([usize; 2], [usize; 2]) {
    halo_region(p, pi)
}

#[cfg(test)]
mod tests;

#[cfg(feature = "cuda")]
pub mod gpu;

#[cfg(feature = "cuda")]
fn umis_gpu_available() -> bool {
    umis_gpu::gpu_available()
}
