//! Device-resident STARK proving for KoalaBear with Keccak Merkle trees.
//!
//! Matrices committed here stay on the GPU: the trace LDE is computed there
//! and hashed into a Keccak-f Merkle tree byte-compatible with Plonky3's
//! `MerkleTreeMmcs<[F; 4], [u64; 4], SerializingHasher<PaddingFreeSponge<KeccakF,
//! 25, 17, 4>>, CompressionFunctionFromHasher<_, 2, 4>, 2, 4>`; the
//! out-of-domain evaluations and the alpha-compressed rows of FRI's input are
//! computed there too. Only digests, opened values, a few extension-field
//! vectors and the queried rows come back. Plonky3's own transcripts and FRI
//! prover (`p3_fri::prover::prove_fri`) drive the protocol, so the proof is the
//! CPU configuration's proof, bit for bit.

use crate::lde::{Device, device, from_raw_vec, launch_cfg, raw};
use cudarc::driver::{CudaSlice, LaunchConfig, PushKernelArg};
use p3_challenger::{CanObserve, FieldChallenger, GrindingChallenger};
use p3_commit::{BatchOpening, BatchOpeningRef, Mmcs, OpeningRequest};
use p3_field::coset::TwoAdicMultiplicativeCoset;
use p3_field::extension::BinomialExtensionField;
use p3_field::{BasedVectorSpace, Field, PrimeCharacteristicRing, batch_multiplicative_inverse, dot_product};
use p3_fri::{BatchMultiOpening, FriParameters, FriProof, PcsProverTranscript, PcsShape, TwoAdicFriFolding};
use p3_keccak::{KeccakF, VECTOR_LEN};
use p3_koala_bear::KoalaBear;
use p3_matrix::dense::RowMajorMatrix;
use p3_matrix::{Dimensions, Matrix};
use p3_maybe_rayon::prelude::*;
use p3_merkle_tree::{MerkleTreeError, MerkleTreeMmcs, PrunedMerklePaths};
use p3_symmetric::{CompressionFunctionFromHasher, MerkleCap, PaddingFreeSponge, SerializingHasher};
use p3_util::{log2_strict_usize, reverse_slice_index_bits};
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::Arc;

pub type Val = KoalaBear;
pub type Challenge = BinomialExtensionField<Val, 4>;
type Sponge = PaddingFreeSponge<KeccakF, 25, 17, 4>;
/// The CPU Merkle tree these device trees reproduce.
pub type CpuValMmcs = MerkleTreeMmcs<
    [Val; VECTOR_LEN],
    [u64; VECTOR_LEN],
    SerializingHasher<Sponge>,
    CompressionFunctionFromHasher<Sponge, 2, 4>,
    2,
    4,
>;
pub type Digest = [u64; 4];

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn with_device<R>(f: impl FnOnce(&mut Device) -> Result<R, String>) -> Result<R, String> {
    let dev = device().ok_or("no GPU")?;
    let mut d = dev.lock().map_err(err)?;
    f(&mut d)
}

fn ext_raw(x: &Challenge) -> [u32; 4] {
    let c = x.as_basis_coefficients_slice();
    [raw(c[0]), raw(c[1]), raw(c[2]), raw(c[3])]
}

fn ext_from_raw(v: &[u32]) -> Challenge {
    let c = from_raw_vec(v.to_vec());
    Challenge::from_basis_coefficients_slice(&c).expect("4 coordinates")
}

/// A row-major matrix on the GPU (canonical Montgomery u32 values).
pub struct DevMat {
    pub(crate) buf: CudaSlice<u32>,
    pub height: usize,
    pub width: usize,
}

/// A committed batch of equal-height device matrices and its Merkle tree (digest
/// layers kept on the host for authentication paths). The matrices are stored side
/// by side as one device matrix (`mat`), the concatenated rows the leaves hash, so
/// kernels handle all of them in one pass; `widths` delimits them.
pub struct GpuTree {
    pub mat: DevMat,
    pub widths: Vec<usize>,
    layers: Arc<Vec<Vec<Digest>>>,
}

impl GpuTree {
    pub fn root(&self) -> Digest {
        self.layers.last().expect("layers")[0]
    }

    pub fn commitment(&self) -> MerkleCap<Val, Digest> {
        MerkleCap::new(vec![self.root()])
    }

    pub fn height(&self) -> usize {
        self.mat.height
    }
}

/// Matrices of one height side by side in one device matrix.
fn concat(dev: &mut Device, mats: Vec<DevMat>) -> Result<DevMat, String> {
    let rows = mats.first().ok_or("no matrices")?.height;
    if mats.iter().any(|m| m.height != rows) {
        return Err("device trees hold matrices of one height".into());
    }
    if mats.len() == 1 {
        return Ok(mats.into_iter().next().expect("one"));
    }
    let total: usize = mats.iter().map(|m| m.width).sum();
    let s = dev.stream.clone();
    let mut out = s.alloc_zeros::<u32>(rows * total).map_err(err)?;
    let mut off = 0u32;
    for m in &mats {
        copy_block(dev, &m.buf, m.width, 0, &mut out, total, off as usize, m.width, rows)?;
        off += m.width as u32;
    }
    Ok(DevMat { buf: out, height: rows, width: total })
}

/// `dst[r][dst_off + c] = src[r][src_off + c]` for `c < cols`, `r < rows`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn copy_block(
    dev: &mut Device,
    src: &CudaSlice<u32>,
    src_stride: usize,
    src_off: usize,
    dst: &mut CudaSlice<u32>,
    dst_stride: usize,
    dst_off: usize,
    cols: usize,
    rows: usize,
) -> Result<(), String> {
    let v = [src_stride as u32, src_off as u32, dst_stride as u32, dst_off as u32, cols as u32, rows as u32];
    // SAFETY: the caller's offsets and strides keep both blocks inside their buffers.
    unsafe {
        dev.stream
            .launch_builder(&dev.copy_block)
            .arg(src)
            .arg(&v[0])
            .arg(&v[1])
            .arg(dst)
            .arg(&v[2])
            .arg(&v[3])
            .arg(&v[4])
            .arg(&v[5])
            .launch(launch_cfg((rows * cols) as u64))
            .map_err(err)?;
    }
    Ok(())
}

fn build_tree(dev: &mut Device, mats: Vec<DevMat>) -> Result<GpuTree, String> {
    let widths: Vec<usize> = mats.iter().map(|m| m.width).collect();
    let mat = concat(dev, mats)?;
    build_tree_of(dev, mat, widths)
}

/// The tree of `mat`, which holds matrices of `widths` side by side.
fn build_tree_of(dev: &mut Device, mat: DevMat, widths: Vec<usize>) -> Result<GpuTree, String> {
    let rows = mat.height;
    if !rows.is_power_of_two() || widths.iter().sum::<usize>() != mat.width {
        return Err("device tree: power-of-two height and matching widths needed".into());
    }
    let s = dev.stream.clone();
    let t_tree = std::time::Instant::now();
    let mut cur = s.alloc_zeros::<u64>(rows * 4).map_err(err)?;
    let (total, zero, r) = (mat.width as u32, 0u32, rows as u32);
    // One thread per row (a sequential sponge over the row); small blocks spread the
    // few rows of a trace over the SMs.
    let cfg = LaunchConfig { grid_dim: (r.div_ceil(32), 1, 1), block_dim: (32, 1, 1), shared_mem_bytes: 0 };
    // SAFETY: argument types match the kernel; reads stay inside the matrix.
    unsafe {
        s.launch_builder(&dev.leaf_hash)
            .arg(&mat.buf)
            .arg(&total)
            .arg(&mat.buf)
            .arg(&zero)
            .arg(&r)
            .arg(&mut cur)
            .launch(cfg)
            .map_err(err)?;
    }
    let mut dev_layers = vec![cur];
    let mut n = rows;
    while n > 1 {
        n /= 2;
        let mut next = s.alloc_zeros::<u64>(n * 4).map_err(err)?;
        let nn = n as u32;
        let cfg = LaunchConfig { grid_dim: (nn.div_ceil(128), 1, 1), block_dim: (128, 1, 1), shared_mem_bytes: 0 };
        // SAFETY: as above.
        unsafe {
            s.launch_builder(&dev.compress_layer)
                .arg(dev_layers.last().unwrap())
                .arg(&mut next)
                .arg(&nn)
                .launch(cfg)
                .map_err(err)?;
        }
        dev_layers.push(next);
    }
    if std::env::var_os("UMIS_GPU_PROFILE").is_some() {
        s.synchronize().map_err(err)?;
        eprintln!("    tree {rows}x{total} ({} matrices): hashed {:?}", widths.len(), t_tree.elapsed());
    }
    // All layers in one buffer, one download (each download waits for the device).
    let lens: Vec<usize> = dev_layers.iter().map(|l| l.len()).collect();
    let mut all = s.alloc_zeros::<u64>(lens.iter().sum()).map_err(err)?;
    let mut off = 0;
    for l in &dev_layers {
        s.memcpy_dtod(l, &mut all.slice_mut(off..off + l.len())).map_err(err)?;
        off += l.len();
    }
    let v: Vec<u64> = s.clone_dtoh(&all).map_err(err)?;
    let mut layers = Vec::with_capacity(lens.len());
    let mut off = 0;
    for len in lens {
        layers.push(v[off..off + len].chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect());
        off += len;
    }
    Ok(GpuTree { mat, widths, layers: Arc::new(layers) })
}

fn upload(dev: &mut Device, m: &RowMajorMatrix<Val>) -> Result<DevMat, String> {
    // SAFETY: repr(transparent) u32 values.
    let src: &[u32] = unsafe { std::slice::from_raw_parts(m.values.as_ptr() as *const u32, m.values.len()) };
    Ok(DevMat { buf: dev.stream.clone_htod(src).map_err(err)?, height: m.height(), width: m.width() })
}

/// Commits a trace: coset LDE (blowup `2^log_blowup`, `shift`) and Merkle tree on the GPU.
pub fn commit_trace(trace: &RowMajorMatrix<Val>, log_blowup: usize, shift: Val) -> Result<GpuTree, String> {
    commit_mats(vec![trace_lde(trace, log_blowup, shift)?])
}

/// Coset LDE of a host trace on the GPU (bit-reversed rows), left on the device.
pub fn trace_lde(trace: &RowMajorMatrix<Val>, log_blowup: usize, shift: Val) -> Result<DevMat, String> {
    with_device(|dev| {
        // SAFETY: repr(transparent) u32 values.
        let src: &[u32] =
            unsafe { std::slice::from_raw_parts(trace.values.as_ptr() as *const u32, trace.values.len()) };
        let buf = dev.coset_lde_device(src, trace.height(), trace.width(), log_blowup, shift)?;
        Ok(DevMat { buf, height: trace.height() << log_blowup, width: trace.width() })
    })
}

/// Commits device matrices (LDEs, bit-reversed rows, one height) as one tree; leaves
/// hash the concatenated rows in order, as `MerkleTreeMmcs` does.
pub fn commit_mats(mats: Vec<DevMat>) -> Result<GpuTree, String> {
    with_device(|dev| build_tree(dev, mats))
}

/// Commits host matrices (already LDEs, bit-reversed rows) as one device tree.
pub fn commit_host(mats: &[RowMajorMatrix<Val>]) -> Result<GpuTree, String> {
    let h = mats.first().ok_or("no matrices")?.height();
    if mats.iter().any(|m| m.height() != h) {
        return Err("device trees hold matrices of one height".into());
    }
    let widths: Vec<usize> = mats.iter().map(|m| m.width()).collect();
    let total: usize = widths.iter().sum();
    let mut v = Vec::with_capacity(h * total);
    for r in 0..h {
        for m in mats {
            v.extend_from_slice(&m.values[r * m.width()..(r + 1) * m.width()]);
        }
    }
    with_device(|dev| {
        let mat = upload(dev, &RowMajorMatrix::new(v, total.max(1)))?;
        build_tree_of(dev, mat, widths)
    })
}

fn gather(dev: &mut Device, m: &DevMat, idx: &[usize]) -> Result<Vec<Vec<Val>>, String> {
    let s = dev.stream.clone();
    let ix: Vec<u32> = idx.iter().map(|&i| i as u32).collect();
    let d_ix = s.clone_htod(&ix).map_err(err)?;
    let mut out = s.alloc_zeros::<u32>(idx.len() * m.width).map_err(err)?;
    let (w, n) = (m.width as u32, idx.len() as u32);
    // SAFETY: indices < height.
    unsafe {
        s.launch_builder(&dev.gather_rows)
            .arg(&m.buf)
            .arg(&w)
            .arg(&d_ix)
            .arg(&n)
            .arg(&mut out)
            .launch(launch_cfg(idx.len() as u64 * m.width as u64))
            .map_err(err)?;
    }
    let v = from_raw_vec(s.clone_dtoh(&out).map_err(err)?);
    Ok(v.chunks_exact(m.width.max(1)).map(|c| c.to_vec()).collect())
}

/// Rows `indices` of every matrix of `tree`: `[query][matrix][column]`.
fn open_rows(tree: &GpuTree, indices: &[usize]) -> Result<Vec<Vec<Vec<Val>>>, String> {
    let rows = with_device(|dev| gather(dev, &tree.mat, indices))?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mut off = 0;
            tree.widths
                .iter()
                .map(|&w| {
                    off += w;
                    row[off - w..off].to_vec()
                })
                .collect()
        })
        .collect())
}

/// `MerkleTreeMmcs`-compatible MMCS whose prover data stays on the GPU. Only
/// prover-side operations run here; verification delegates to the CPU tree.
#[derive(Clone)]
pub struct GpuMmcs {
    cpu: CpuValMmcs,
}

impl Default for GpuMmcs {
    fn default() -> Self {
        let sponge = Sponge::new(KeccakF);
        Self { cpu: CpuValMmcs::new(SerializingHasher::new(sponge), CompressionFunctionFromHasher::new(sponge), 0) }
    }
}

impl Mmcs<Val> for GpuMmcs {
    type ProverData<M> = GpuTree;
    type Commitment = MerkleCap<Val, Digest>;
    type Proof = Vec<Digest>;
    type MultiProof = PrunedMerklePaths<u64, 4>;
    type Error = MerkleTreeError;

    fn commit<M: Matrix<Val>>(&self, inputs: Vec<M>) -> (Self::Commitment, GpuTree) {
        let mats: Vec<RowMajorMatrix<Val>> = inputs.into_iter().map(|m| m.to_row_major_matrix()).collect();
        let tree = commit_host(&mats).expect("GPU commit");
        (tree.commitment(), tree)
    }

    fn open_batch<M: Matrix<Val>>(&self, index: usize, tree: &GpuTree) -> BatchOpening<Val, Self> {
        let rows = open_rows(tree, &[index]).expect("GPU open").pop().expect("one query");
        let mut proof = Vec::new();
        let mut idx = index;
        for layer in &tree.layers[..tree.layers.len() - 1] {
            proof.push(layer[idx ^ 1]);
            idx >>= 1;
        }
        BatchOpening::new(rows, proof)
    }

    fn get_matrices<'a, M: Matrix<Val>>(&self, _: &'a GpuTree) -> Vec<&'a M> {
        panic!("umis-gpu: committed matrices stay on the GPU")
    }

    fn get_matrix_heights<M: Matrix<Val>>(&self, tree: &GpuTree) -> Vec<usize> {
        vec![tree.mat.height; tree.widths.len()]
    }

    fn verify_batch(
        &self,
        commit: &Self::Commitment,
        dimensions: &[Dimensions],
        index: usize,
        opening: BatchOpeningRef<'_, Val, Self>,
    ) -> Result<(), Self::Error> {
        let (v, p) = opening.unpack();
        self.cpu.verify_batch(commit, dimensions, index, BatchOpeningRef::new(v, p))
    }

    fn open_multi_batch<M: Matrix<Val>>(
        &self,
        indices: &[usize],
        tree: &GpuTree,
    ) -> (Vec<Vec<Vec<Val>>>, Self::MultiProof) {
        let rows = open_rows(tree, indices).expect("GPU open");
        // Pruned binary paths, in Plonky3's frontier order (see `p3_merkle_tree::pruning`).
        let mut nodes: Vec<usize> = indices.to_vec();
        nodes.sort_unstable();
        nodes.dedup();
        let mut sibling_hashes = Vec::new();
        for layer in &tree.layers[..tree.layers.len() - 1] {
            let mut parents = Vec::with_capacity(nodes.len());
            let mut i = 0;
            while i < nodes.len() {
                let start = nodes[i] / 2 * 2;
                let mut member = i;
                for k in 0..2 {
                    if member < nodes.len() && nodes[member] == start + k {
                        member += 1;
                    } else {
                        sibling_hashes.push(layer[start + k]);
                    }
                }
                parents.push(start / 2);
                i = member;
            }
            nodes = parents;
        }
        (rows, PrunedMerklePaths { sibling_hashes })
    }

    fn verify_multi_batch<R: AsRef<[Val]> + PartialEq>(
        &self,
        commit: &Self::Commitment,
        dimensions: &[Dimensions],
        indices: &[usize],
        opened_values: &[Vec<R>],
        proof: &Self::MultiProof,
    ) -> Result<(), Self::Error> {
        self.cpu.verify_multi_batch(commit, dimensions, indices, opened_values, proof)
    }
}

/// `sum_r wgt[r] * m[r][c]` for every column `c` over the first `rows` rows
/// (`wgt`: 4 coordinates per row, on the device).
fn col_dot(dev: &mut Device, m: &DevMat, rows: usize, d_w: &CudaSlice<u32>) -> Result<Vec<Challenge>, String> {
    let s = dev.stream.clone();
    let chunks = 32u32.min(rows as u32).max(1);
    let mut part = s.alloc_zeros::<u32>(4 * m.width * chunks as usize).map_err(err)?;
    let mut out = s.alloc_zeros::<u32>(4 * m.width).map_err(err)?;
    let (ww, rr) = (m.width as u32, rows as u32);
    // SAFETY: rows <= height; buffers sized above.
    unsafe {
        s.launch_builder(&dev.ext_col_dot_part)
            .arg(&m.buf)
            .arg(&ww)
            .arg(&rr)
            .arg(d_w)
            .arg(&mut part)
            .launch(LaunchConfig { grid_dim: (ww.div_ceil(32), chunks, 1), block_dim: (32, 1, 1), shared_mem_bytes: 0 })
            .map_err(err)?;
        s.launch_builder(&dev.ext_col_dot_sum)
            .arg(&part)
            .arg(&ww)
            .arg(&chunks)
            .arg(&mut out)
            .launch(LaunchConfig { grid_dim: (ww.div_ceil(64), 1, 1), block_dim: (64, 1, 1), shared_mem_bytes: 0 })
            .map_err(err)?;
    }
    let v: Vec<u32> = s.clone_dtoh(&out).map_err(err)?;
    Ok(v.chunks_exact(4).map(ext_from_raw).collect())
}

/// `sum_c pw[c] * m[r][c]` for every row `r`, left on the device (4 coordinates per row).
fn row_dot(dev: &mut Device, m: &DevMat, pw: &[Challenge]) -> Result<CudaSlice<u32>, String> {
    let s = dev.stream.clone();
    let p: Vec<u32> = pw.iter().flat_map(ext_raw).collect();
    let d_p = s.clone_htod(&p).map_err(err)?;
    let mut out = s.alloc_zeros::<u32>(4 * m.height).map_err(err)?;
    let (ww, hh) = (m.width as u32, m.height as u32);
    let cfg = LaunchConfig { grid_dim: (hh, 1, 1), block_dim: (256, 1, 1), shared_mem_bytes: 0 };
    // SAFETY: pw has width entries.
    unsafe {
        s.launch_builder(&dev.ext_row_dot)
            .arg(&m.buf)
            .arg(&ww)
            .arg(&hh)
            .arg(&d_p)
            .arg(&mut out)
            .launch(cfg)
            .map_err(err)?;
    }
    Ok(out)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Ext4([u32; 4]);

// SAFETY: plain old data.
unsafe impl cudarc::driver::DeviceRepr for Ext4 {}

/// The FRI proof of [`open`]: the CPU configuration's proof type with this MMCS.
pub type GpuFriProof<FriMmcs> = FriProof<Challenge, FriMmcs, Val, Vec<BatchMultiOpening<Val, GpuMmcs>>>;

/// `TwoAdicFriPcs::open` for device trees: the same opened values, transcript
/// and FRI proof, with the out-of-domain evaluations and the compressed FRI
/// input rows computed on the GPU. Every matrix is an LDE at `fri.log_blowup`
/// on the coset `GENERATOR * K`, rows bit-reversed (as `commit_trace` makes them).
pub fn open<FriMmcs, Challenger>(
    fri: &FriParameters<FriMmcs>,
    mmcs: &GpuMmcs,
    rounds: Vec<(&GpuTree, Vec<Vec<Challenge>>)>,
    challenger: &mut Challenger,
) -> Result<(Vec<Vec<Vec<Vec<Challenge>>>>, GpuFriProof<FriMmcs>), String>
where
    FriMmcs: Mmcs<Challenge>,
    Challenger: FieldChallenger<Val> + GrindingChallenger<Witness = Val> + CanObserve<FriMmcs::Commitment>,
{
    let prof = std::env::var_os("UMIS_GPU_PROFILE").is_some();
    let t0 = std::time::Instant::now();
    let lap = |what: &str| {
        if prof {
            eprintln!("    open {what}: {:?}", t0.elapsed());
        }
    };
    for (t, points) in &rounds {
        if points.len() != t.widths.len() {
            return Err("one point list per committed matrix".into());
        }
    }
    let global_max_height = rounds.iter().map(|(t, _)| t.mat.height).max().ok_or("no matrices")?;
    let log_global_max_height = log2_strict_usize(global_max_height);
    let mut coset: Vec<Val> =
        TwoAdicMultiplicativeCoset::new(Val::GENERATOR, log_global_max_height).unwrap().iter().collect();
    reverse_slice_index_bits(&mut coset);
    // 1/(z - x) over the largest height each point is opened at (bit-reversed coset prefix).
    let mut max_log: BTreeMap<Vec<u32>, (Challenge, usize)> = BTreeMap::new();
    for (t, points) in &rounds {
        for z in points.iter().flatten() {
            let e = max_log.entry(ext_raw(z).to_vec()).or_insert((*z, 0));
            e.1 = e.1.max(log2_strict_usize(t.mat.height));
        }
    }
    lap("coset");
    let inv_denoms: BTreeMap<Vec<u32>, Vec<Challenge>> = max_log
        .into_iter()
        .map(|(k, (z, lh))| {
            (k, batch_multiplicative_inverse(&coset[..1 << lh].par_iter().map(|&x| z - x).collect::<Vec<_>>()))
        })
        .collect();
    // The distinct points of each tree, in first-use order.
    let tree_points: Vec<Vec<Challenge>> = rounds
        .iter()
        .map(|(_, points)| {
            let mut v: Vec<Challenge> = Vec::new();
            for z in points.iter().flatten() {
                if !v.contains(z) {
                    v.push(*z);
                }
            }
            v
        })
        .collect();
    // Out-of-domain values: Lagrange interpolation over the low coset (first h rows),
    // L_i(z) = Z(z) x_i / (h g^h (z - x_i)), Z(z) = z^h - g^h, g = GENERATOR; one
    // column dot product over all of a tree's matrices per point.
    // Weights per (tree, point), computed before taking the device (no rayon under its lock:
    // a pool worker may be waiting for the device).
    let weights: Vec<Vec<Vec<u32>>> = rounds
        .iter()
        .zip(&tree_points)
        .map(|((t, _), tp)| {
            let h = t.mat.height >> fri.log_blowup;
            tp.iter()
                .map(|z| {
                    let gh = Val::GENERATOR.exp_u64(h as u64);
                    let norm = (Val::from_u64(h as u64) * gh).inverse();
                    let zz = z.exp_u64(h as u64) - Challenge::from(gh);
                    let inv = &inv_denoms[&ext_raw(z).to_vec()];
                    (0..h).into_par_iter().flat_map_iter(|i| ext_raw(&(zz * inv[i] * (coset[i] * norm)))).collect()
                })
                .collect()
        })
        .collect();
    let opened: Vec<Vec<Vec<Vec<Challenge>>>> = with_device(|dev| {
        let mut out = Vec::with_capacity(rounds.len());
        for (((t, points), tp), tw) in rounds.iter().zip(&tree_points).zip(&weights) {
            let h = t.mat.height >> fri.log_blowup;
            let mut at_point: Vec<Vec<Challenge>> = Vec::with_capacity(tp.len());
            for wgt in tw {
                let d_w = dev.stream.clone_htod(wgt).map_err(err)?;
                let tk = std::time::Instant::now();
                at_point.push(col_dot(dev, &t.mat, h, &d_w)?);
                if prof {
                    eprintln!("      col_dot {h}x{}: {:?}", t.mat.width, tk.elapsed());
                }
            }
            let mut off = 0;
            let per_mat: Vec<Vec<Vec<Challenge>>> = t
                .widths
                .iter()
                .zip(points)
                .map(|(&w, pts)| {
                    off += w;
                    pts.iter()
                        .map(|z| {
                            let p = tp.iter().position(|y| y == z).expect("point");
                            at_point[p][off - w..off].to_vec()
                        })
                        .collect()
                })
                .collect();
            out.push(per_mat);
        }
        Ok(out)
    })?;
    lap("out-of-domain values");
    let mut transcript =
        PcsProverTranscript::<Challenger, Val, Challenge>::new(challenger, PcsShape::from_opened_values(fri, &opened));
    transcript.claimed_openings(&opened);
    let (alpha, batch_pow_witness) = transcript.batch_phase();
    let batch_pow_witness = batch_pow_witness.unwrap_or(Val::ZERO);

    // FRI inputs: per height, sum over (matrix, point) in Plonky3's order of
    // alpha^o (f(z) - f(x)) / (z - x), with o the running count of reduced columns.
    // Per tree and point: one row dot product with the alpha powers of every column
    // opened there, then (W - that) / (z - x) accumulated on the device.
    let mut num_reduced = [0usize; 33];
    let reduced: Vec<(usize, Vec<Challenge>)> = with_device(|dev| {
        let s = dev.stream.clone();
        let mut acc: BTreeMap<usize, CudaSlice<u32>> = BTreeMap::new();
        let mut d_inv: BTreeMap<Vec<u32>, CudaSlice<u32>> = BTreeMap::new();
        for (((t, points), opened_round), tp) in rounds.iter().zip(&opened).zip(&tree_points) {
            let lh = log2_strict_usize(t.mat.height);
            // per point: column weights and the constant W = sum alpha^o * reduced opening
            let mut pw: Vec<Vec<Challenge>> = vec![vec![Challenge::ZERO; t.mat.width]; tp.len()];
            let mut wsum: Vec<Challenge> = vec![Challenge::ZERO; tp.len()];
            let mut off = 0;
            for ((&w, pts), opened_mat) in t.widths.iter().zip(points).zip(opened_round) {
                for (z, openings) in pts.iter().zip(opened_mat) {
                    let p = tp.iter().position(|y| y == z).expect("point");
                    let o = alpha.exp_u64(num_reduced[lh] as u64);
                    let reduced_openings: Challenge = dot_product(alpha.powers(), openings.iter().copied());
                    wsum[p] += o * reduced_openings;
                    for (c, a) in alpha.powers().take(w).enumerate() {
                        pw[p][off + c] += o * a;
                    }
                    num_reduced[lh] += w;
                }
                off += w;
            }
            if let std::collections::btree_map::Entry::Vacant(e) = acc.entry(lh) {
                e.insert(s.alloc_zeros::<u32>(4 * t.mat.height).map_err(err)?);
            }
            for (p, z) in tp.iter().enumerate() {
                let tk = std::time::Instant::now();
                let compressed = row_dot(dev, &t.mat, &pw[p])?;
                if prof {
                    eprintln!("      row_dot {}x{}: {:?}", t.mat.height, t.mat.width, tk.elapsed());
                }
                let key = ext_raw(z).to_vec();
                if let std::collections::btree_map::Entry::Vacant(e) = d_inv.entry(key.clone()) {
                    let inv: Vec<u32> = inv_denoms[&key].iter().flat_map(ext_raw).collect();
                    e.insert(s.clone_htod(&inv).map_err(err)?);
                }
                let (wv, rv, n) = (Ext4(ext_raw(&wsum[p])), Ext4(ext_raw(&Challenge::ONE)), t.mat.height as u32);
                let ro = acc.get_mut(&lh).expect("accumulator");
                // SAFETY: all buffers hold `height` extension elements (inv_denoms at least).
                unsafe {
                    s.launch_builder(&dev.fri_acc)
                        .arg(ro)
                        .arg(&compressed)
                        .arg(&d_inv[&key])
                        .arg(&wv)
                        .arg(&rv)
                        .arg(&n)
                        .launch(LaunchConfig {
                            grid_dim: (n.div_ceil(128), 1, 1),
                            block_dim: (128, 1, 1),
                            shared_mem_bytes: 0,
                        })
                        .map_err(err)?;
                }
            }
        }
        acc.into_iter()
            .map(|(lh, buf)| {
                let v: Vec<u32> = s.clone_dtoh(&buf).map_err(err)?;
                Ok((lh, v.chunks_exact(4).map(ext_from_raw).collect()))
            })
            .collect()
    })?;
    lap("reduced openings");
    // Tallest first, as `TwoAdicFriPcs::open` orders them.
    let fri_input: Vec<Vec<Challenge>> = reduced.into_iter().rev().map(|(_, v)| v).collect();
    let folding = TwoAdicFriFolding::<Vec<BatchMultiOpening<Val, GpuMmcs>>, MerkleTreeError>(PhantomData);
    let requests: Vec<OpeningRequest<'_, GpuTree, Challenge>> =
        rounds.iter().map(|(t, p)| OpeningRequest { prover_data: *t, points: p.clone() }).collect();
    let fri_proof = transcript.delegate(|ch| {
        p3_fri::prover::prove_fri(
            &folding,
            fri,
            fri_input,
            ch,
            log_global_max_height,
            &requests,
            mmcs,
            batch_pow_witness,
        )
    });
    let fri_proof = match fri_proof {
        Ok(p) => p,
        Err(e) => {
            transcript.abort();
            return Err(format!("{e:?}"));
        }
    };
    transcript.finish();
    lap("FRI");
    Ok((opened, fri_proof))
}

/// Layout of the life2d trace for [`life_trace`] (`task_life2d::air::Shape`).
#[derive(Clone, Debug)]
pub struct LifeTraceShape {
    pub n: u32,
    pub k: u32,
    pub strips: u32,
    pub rpr: u32,
    pub w: u32,
    pub ws: u32,
    pub y: u32,
    pub height: u32,
    /// Main column `c`: `(kind, t, i, x)`, kind 0 generation-0 cell, 1 cell, 2 inverse witness.
    pub columns: Vec<(u8, u8, u8, u16)>,
    /// Absent flag of each extended-board band, `row band * 3 + column band`.
    pub band_absent: [bool; 9],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LifeTraceShapeRaw {
    n: u32,
    k: u32,
    strips: u32,
    rpr: u32,
    w: u32,
    ws: u32,
    y: u32,
    height: u32,
    rows: u32,
    ext_w: u32,
    band_absent: [u8; 9],
    _pad: [u8; 3],
}

// SAFETY: plain old data, passed by value to the kernels.
unsafe impl cudarc::driver::DeviceRepr for LifeTraceShapeRaw {}

/// The honest life2d trace on the device, committed (coset LDE + Merkle tree), and
/// generation `k` of the stacked torus on the host (`rows x ws` bytes) for the claim.
/// `ext` is the extended board (`ext_w x ext_w` bytes, 0/1).
pub fn life_trace_commit(
    s: &LifeTraceShape,
    ext: &[u8],
    ext_w: usize,
    log_blowup: usize,
    shift: Val,
) -> Result<(GpuTree, Vec<u8>), String> {
    let (m, gk) = life_trace_lde(s, ext, ext_w, log_blowup, shift)?;
    Ok((commit_mats(vec![m])?, gk))
}

/// As [`life_trace_commit`], without the tree: the trace LDE stays on the device.
pub fn life_trace_lde(
    s: &LifeTraceShape,
    ext: &[u8],
    ext_w: usize,
    log_blowup: usize,
    shift: Val,
) -> Result<(DevMat, Vec<u8>), String> {
    with_device(|dev| {
        let st = dev.stream.clone();
        let rows = (s.rpr * s.height) as usize;
        let ws = s.ws as usize;
        let gsz = rows * ws;
        let width = s.columns.len();
        let raw_s = LifeTraceShapeRaw {
            n: s.n,
            k: s.k,
            strips: s.strips,
            rpr: s.rpr,
            w: s.w,
            ws: s.ws,
            y: s.y,
            height: s.height,
            rows: rows as u32,
            ext_w: ext_w as u32,
            band_absent: s.band_absent.map(|b| b as u8),
            _pad: [0; 3],
        };
        let d_ext = st.clone_htod(ext).map_err(err)?;
        let kk = s.k as usize;
        let mut gens = st.alloc_zeros::<u8>((kk + 1) * gsz).map_err(err)?;
        let mut fs = st.alloc_zeros::<u8>((kk + 1) * gsz).map_err(err)?;
        // SAFETY (all launches below): argument types match the kernels; offsets stay in bounds.
        unsafe {
            let mut g0 = gens.slice_mut(0..gsz);
            st.launch_builder(&dev.life_gen0)
                .arg(&d_ext)
                .arg(&raw_s)
                .arg(&mut g0)
                .launch(launch_cfg(gsz as u64))
                .map_err(err)?;
        }
        for t in 0..kk {
            let (lo, mut hi) = gens.split_at_mut((t + 1) * gsz);
            let g = lo.slice(t * gsz..(t + 1) * gsz);
            let mut ng = hi.slice_mut(0..gsz);
            let mut nf = fs.slice_mut((t + 1) * gsz..(t + 2) * gsz);
            let tt = t as u32;
            unsafe {
                st.launch_builder(&dev.life_gen_step)
                    .arg(&g)
                    .arg(&mut ng)
                    .arg(&mut nf)
                    .arg(&raw_s)
                    .arg(&tt)
                    .launch(launch_cfg(gsz as u64))
                    .map_err(err)?;
            }
        }
        let colmap: Vec<u32> = s
            .columns
            .iter()
            .map(|&(kind, t, i, x)| kind as u32 | (t as u32) << 2 | (i as u32) << 10 | (x as u32) << 18)
            .collect();
        let d_map = st.clone_htod(&colmap).map_err(err)?;
        let inv: Vec<u32> = (0..32u64).map(|f| if f == 0 { 0 } else { raw(Val::from_u64(f).inverse()) }).collect();
        let d_inv = st.clone_htod(&inv).map_err(err)?;
        let h = s.height as usize;
        let mut trace = st.alloc_zeros::<u32>(h * width).map_err(err)?;
        let ww = width as u32;
        unsafe {
            st.launch_builder(&dev.life_fill)
                .arg(&gens)
                .arg(&fs)
                .arg(&d_map)
                .arg(&d_inv)
                .arg(&raw_s)
                .arg(&ww)
                .arg(&mut trace)
                .launch(launch_cfg((h * width) as u64))
                .map_err(err)?;
        }
        let prof = std::env::var_os("UMIS_GPU_PROFILE").is_some();
        let t0 = std::time::Instant::now();
        let gk: Vec<u8> = st.clone_dtoh(&gens.slice(kk * gsz..(kk + 1) * gsz)).map_err(err)?;
        let t1 = t0.elapsed();
        let lde = dev.coset_lde_on_device(trace, h, width, log_blowup, shift)?;
        if prof {
            st.synchronize().map_err(err)?;
        }
        if prof {
            eprintln!("  trace: gens+fill+download {t1:?}, LDE {:?}", t0.elapsed() - t1);
        }
        Ok((DevMat { buf: lde, height: h << log_blowup, width }, gk))
    })
}

/// Periodic columns for [`air_quotient`].
pub enum Periodic<'a> {
    None,
    /// Every column has full period (the trace height): extended to the quotient
    /// coset `shift * K` (blowup `q / h`) on the GPU.
    FullPeriod {
        columns: &'a [Vec<Val>],
        shift: Val,
    },
    /// Plonky3's periodic table on the quotient domain: `q` natural rows of `width` values.
    Table {
        values: &'a [Val],
        width: usize,
    },
}

/// Postfix bytecode of an AIR's base-field constraints, grouped by template (see
/// `air_quotient` in resident.cu).
pub struct Program {
    /// Template words: `op | operand << 4`.
    pub code: Vec<u32>,
    /// Per group: `[code_start, code_end, n_args, n_instances, args_offset]`.
    pub groups: Vec<u32>,
    /// Per group, `n_args + 1` rows of `n_instances` words (the last row: constraint index).
    pub args: Vec<u32>,
    pub n_constraints: usize,
    /// Device copies of `code`, `groups`, `args` (uploaded once).
    pub(crate) device: std::sync::OnceLock<[CudaSlice<u32>; 3]>,
    /// Whether to compile the kernel ([`crate::jit`]) rather than interpret.
    pub(crate) allow_jit: bool,
    /// The run-time compiled kernel, or `None`: interpret.
    jit: std::sync::OnceLock<Option<cudarc::driver::CudaFunction>>,
}

impl Program {
    pub fn new(code: Vec<u32>, groups: Vec<u32>, args: Vec<u32>, n_constraints: usize) -> Self {
        Self {
            code,
            groups,
            args,
            n_constraints,
            device: std::sync::OnceLock::new(),
            allow_jit: true,
            jit: std::sync::OnceLock::new(),
        }
    }
}

/// The quotient of `m` instances of one AIR on the quotient domain of size `2^log_q`,
/// left on the device chunk-major: chunk `j` (points `row * 2^log_chunks + j`, as
/// `split_evals`) is a `(2^(log_q - log_chunks)) x 4m` matrix, instance `k` in columns
/// `4k..4k+4`. Value: `inv_van[i] * sum_c apw[c] * C_c(x_i)`, the constraints of
/// `prog` over instance `k`'s trace LDE (columns `k * inst_width..` of `lde`), periodic
/// columns `k * n_per..` and public values `k * n_pub..` (`publics` holds all).
/// `sels` = (is_first_row, is_last_row, is_transition, inv_vanishing).
#[allow(clippy::too_many_arguments)]
pub fn air_quotient(
    lde: &DevMat,
    inst_width: usize,
    m: usize,
    prog: &Program,
    periodic: Periodic<'_>,
    n_per: usize,
    publics: &[Val],
    sels: [&[Val]; 4],
    apw: &[Challenge],
    next_step: usize,
    log_q: usize,
    log_chunks: usize,
) -> Result<CudaSlice<u32>, String> {
    if inst_width * m > lde.width || publics.len() % m.max(1) != 0 {
        return Err("air_quotient: instance layout does not fit".into());
    }
    with_device(|dev| {
        let s = dev.stream.clone();
        let q = 1usize << log_q;
        let up = |v: &[Val]| s.clone_htod(&v.iter().map(|&x| raw(x)).collect::<Vec<_>>()).map_err(err);
        let (d_per, brev) = match periodic {
            Periodic::None => (s.alloc_zeros::<u32>(1).map_err(err)?, 0u32),
            Periodic::FullPeriod { columns, shift } => {
                let h = columns.first().map_or(0, Vec::len);
                if h == 0 || columns.iter().any(|c| c.len() != h) || q % h != 0 || columns.len() != n_per * m {
                    return Err("full-period columns must all have the trace height".into());
                }
                let nc = columns.len();
                let mut rm = vec![0u32; h * nc];
                for (c, col) in columns.iter().enumerate() {
                    for (r, &v) in col.iter().enumerate() {
                        rm[r * nc + c] = raw(v);
                    }
                }
                let bits = (q / h).trailing_zeros() as usize;
                (dev.coset_lde_device(&rm, h, nc, bits, shift)?, 1u32)
            }
            Periodic::Table { values, width } => {
                if width != n_per * m {
                    return Err("periodic table width".into());
                }
                (up(values)?, 0u32)
            }
        };
        let n_per = if matches!(periodic, Periodic::None) { 0 } else { n_per };
        let d_pub = if publics.is_empty() { s.alloc_zeros::<u32>(1).map_err(err)? } else { up(publics)? };
        let n_pub = (publics.len() / m.max(1)) as u32;
        let (d_f, d_l, d_t, d_iv) = (up(sels[0])?, up(sels[1])?, up(sels[2])?, up(sels[3])?);
        let d_apw = s.clone_htod(&apw.iter().flat_map(ext_raw).collect::<Vec<_>>()).map_err(err)?;
        if prog.device.get().is_none() {
            let nz = |v: &[u32]| s.clone_htod(if v.is_empty() { &[0u32][..] } else { v }).map_err(err);
            let _ = prog.device.set([nz(&prog.code)?, nz(&prog.groups)?, nz(&prog.args)?]);
        }
        let [d_code, d_groups, d_args] = prog.device.get().ok_or("program upload")?;
        let mut out = s.alloc_zeros::<u32>(4 * q * m).map_err(err)?;
        let (ww, ns, ng, lq) = (inst_width as u32, next_step as u32, (prog.groups.len() / 5) as u32, log_q as u32);
        let (stride, lc, np) = (lde.width as u32, log_chunks as u32, n_per as u32);
        let cfg = LaunchConfig { grid_dim: (q as u32, m as u32, 1), block_dim: (256, 1, 1), shared_mem_bytes: 0 };
        let jit = prog.jit.get_or_init(|| match crate::jit::kernel(s.context(), prog) {
            Ok(f) => Some(f),
            Err(e) => {
                use std::io::Write;
                let _ = writeln!(std::io::stderr(), "umis-gpu: constraint kernel not compiled ({e}); interpreting");
                None
            }
        });
        // SAFETY: argument types and layout match `air_quotient_jit` (crate::jit) and
        // `air_quotient` (resident.cu); the instance layout was checked above.
        unsafe {
            let mut b = match jit {
                Some(f) => s.launch_builder(f),
                None => s.launch_builder(&dev.air_quotient),
            };
            b.arg(&lde.buf)
                .arg(&ww)
                .arg(&ns)
                .arg(&d_per)
                .arg(&np)
                .arg(&brev)
                .arg(&d_pub)
                .arg(&d_f)
                .arg(&d_l)
                .arg(&d_t)
                .arg(&d_iv)
                .arg(&d_apw);
            if jit.is_none() {
                b.arg(d_code).arg(d_groups).arg(&ng);
            }
            b.arg(d_args).arg(&lq).arg(&stride).arg(&n_pub).arg(&lc).arg(&mut out).launch(cfg).map_err(err)?;
        }
        Ok(out)
    })
}

/// Commits the quotient chunks of [`air_quotient`]'s output: each chunk's coset LDE
/// (blowup `2^log_blowup`, shift `GENERATOR / shifts[j]`, `shifts[j]` the chunk
/// domain's shift) on the device, laid out as Plonky3 commits them (instance-major,
/// then chunk: `m * 2^log_chunks` matrices of width 4).
pub fn commit_quotient(
    values: CudaSlice<u32>,
    m: usize,
    log_q: usize,
    log_chunks: usize,
    log_blowup: usize,
    shifts: &[Val],
) -> Result<GpuTree, String> {
    let nch = 1usize << log_chunks;
    if shifts.len() != nch {
        return Err("one shift per chunk".into());
    }
    with_device(|dev| {
        let s = dev.stream.clone();
        let rows_c = 1usize << (log_q - log_chunks);
        let (w, h) = (4 * m, rows_c << log_blowup);
        let mut phys = s.alloc_zeros::<u32>(h * w * nch).map_err(err)?;
        for (j, &sh) in shifts.iter().enumerate() {
            let chunk = if nch == 1 {
                s.clone_dtod(&values).map_err(err)?
            } else {
                s.clone_dtod(&values.slice(j * rows_c * w..(j + 1) * rows_c * w)).map_err(err)?
            };
            let lde = dev.coset_lde_on_device(chunk, rows_c, w, log_blowup, Val::GENERATOR / sh)?;
            for k in 0..m {
                copy_block(dev, &lde, w, 4 * k, &mut phys, w * nch, (k * nch + j) * 4, 4, h)?;
            }
        }
        build_tree_of(dev, DevMat { buf: phys, height: h, width: w * nch }, vec![4; m * nch])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use p3_commit::Mmcs;
    use p3_dft::{Radix2DitParallel, TwoAdicSubgroupDft};
    use p3_matrix::bitrev::BitReversibleMatrix;

    fn mat(h: usize, w: usize, seed: u64) -> RowMajorMatrix<Val> {
        RowMajorMatrix::new(
            (0..h * w).map(|i| Val::from_u64((i as u64 + seed).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33)).collect(),
            w,
        )
    }

    #[test]
    fn device_tree_equals_cpu_tree() {
        if !crate::gpu_available() {
            eprintln!("no GPU; skipped");
            return;
        }
        let cpu = GpuMmcs::default().cpu.clone();
        let gpu = GpuMmcs::default();
        for (h, ws) in [(4, vec![3]), (64, vec![17]), (256, vec![4, 4]), (2048, vec![2016]), (16, vec![1, 5])] {
            let mats: Vec<_> = ws.iter().enumerate().map(|(i, &w)| mat(h, w, i as u64 * 7)).collect();
            let (c_cpu, d_cpu) = cpu.commit(mats.clone());
            let (c_gpu, d_gpu) = gpu.commit(mats.clone());
            assert_eq!(c_cpu, c_gpu, "root h={h} widths={ws:?}");
            let idx = [0, h - 1, h / 2, 1 % h, h / 2, 3 % h];
            let (v_cpu, p_cpu) = cpu.open_multi_batch(&idx, &d_cpu);
            let (v_gpu, p_gpu) = gpu.open_multi_batch::<RowMajorMatrix<Val>>(&idx, &d_gpu);
            assert_eq!(v_cpu, v_gpu, "opened rows");
            assert_eq!(p_cpu.sibling_hashes, p_gpu.sibling_hashes, "pruned paths");
            let o_cpu = cpu.open_batch(h / 3, &d_cpu);
            let o_gpu = gpu.open_batch::<RowMajorMatrix<Val>>(h / 3, &d_gpu);
            assert_eq!(o_cpu.opened_values, o_gpu.opened_values);
            assert_eq!(o_cpu.opening_proof, o_gpu.opening_proof);
        }
        // a committed trace LDE equals the CPU LDE committed by the CPU tree
        let t = mat(1024, 300, 5);
        let lde = Radix2DitParallel::<Val>::default()
            .coset_lde_batch(t.clone(), 1, Val::GENERATOR)
            .bit_reverse_rows()
            .to_row_major_matrix();
        let (c_cpu, _) = cpu.commit(vec![lde]);
        let tree = commit_trace(&t, 1, Val::GENERATOR).unwrap();
        assert_eq!(c_cpu, tree.commitment(), "trace commitment");
        eprintln!("device trees == CPU trees");
    }
}
