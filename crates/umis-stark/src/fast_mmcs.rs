//! A KoalaBear Poseidon2 Merkle MMCS whose trees are built by a pluggable [`TreeBackend`]
//! (the GPU, `umis-gpu`), with exactly the commitments, openings and pruned multi-proofs of
//! the stock [`ValMmcs`](crate::config::koala_bear::ValMmcs) (arity 2, cap height 0). Proofs
//! made with it verify with the stock configuration; verification delegates to it.
//!
//! The tree, for matrices whose heights are powers of two (other shapes and small trees go to
//! the stock MMCS):
//!
//! - layer 0: `H(row i of every tallest matrix, concatenated in insertion order)`;
//! - layer `k + 1`: `C(layer_k[2i], layer_k[2i + 1])`, then, if some matrices have the new
//!   layer's size as height, `C(that, H(their rows i, concatenated))`;
//! - the root (last layer) is the commitment; a proof is one sibling per layer below it.
//!
//! `tests` check every output against `MerkleTreeMmcs` on random shapes.

use alloc::vec;
use alloc::vec::Vec;
use p3_commit::{BatchOpening, BatchOpeningRef, Mmcs};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::{Dimensions, Matrix};
use p3_maybe_rayon::prelude::*;
use p3_merkle_tree::{MerkleCap, MerkleTree, MerkleTreeError, PrunedMerklePaths};
use p3_symmetric::{CryptographicHasher, PseudoCompressionFunction};

use crate::config::koala_bear::{Compress, Hash, Val, ValMmcs};

pub const DIGEST: usize = 8;
pub type Digest = [Val; DIGEST];

/// Builds the digest layers of the tree (module docs) for a backend.
pub trait TreeBackend: Send + Sync + Clone + core::fmt::Debug {
    /// Whether to build a tree of `cells` committed values here (else the stock MMCS does).
    fn wants(&self, cells: usize) -> bool;

    /// Every digest layer, leaves first; `groups` tallest first.
    fn layers(&self, groups: &[Group<'_>]) -> Vec<Vec<Digest>>;
}

/// The matrices of one height, in insertion order, each `(width, values)` row-major: a leaf
/// row is their rows `i` concatenated.
#[derive(Debug)]
pub struct Group<'a> {
    pub height: usize,
    pub mats: Vec<(usize, &'a [Val])>,
}

impl Group<'_> {
    pub fn width(&self) -> usize {
        self.mats.iter().map(|m| m.0).sum()
    }

    /// The concatenated rows, row-major (host backends).
    pub fn concat(&self) -> Vec<Val> {
        let w = self.width();
        let mut rows = Val::zero_vec(self.height * w);
        if w > 0 {
            rows.par_chunks_exact_mut(w).enumerate().for_each(|(r, out)| {
                let mut at = 0;
                for (mw, v) in &self.mats {
                    out[at..at + mw].copy_from_slice(&v[r * mw..(r + 1) * mw]);
                    at += mw;
                }
            });
        }
        rows
    }
}

/// The reference backend: the stock hash and compression on the CPU (tests, and the shape
/// the GPU backend must reproduce).
#[derive(Clone, Debug)]
pub struct CpuTree {
    pub hash: Hash,
    pub compress: Compress,
    /// Smallest tree built here (cells); 0 builds every power-of-two tree here.
    pub min_cells: usize,
}

impl CpuTree {
    pub fn new(min_cells: usize) -> Self {
        let perm = p3_koala_bear::default_koalabear_poseidon2_16();
        Self { hash: Hash::new(perm.clone()), compress: Compress::new(perm), min_cells }
    }
}

/// [`TreeBackend::layers`] from a row hash and a pair compression (shared by backends that
/// do the layers on the host).
pub fn layers_with(
    groups: &[Group<'_>],
    hash_rows: impl Fn(&Group<'_>) -> Vec<Digest>,
    compress: impl Fn(&[Digest], &[Digest]) -> Vec<Digest>,
) -> Vec<Vec<Digest>> {
    let mut layers = vec![hash_rows(&groups[0])];
    let mut next_group = 1;
    while layers.last().expect("a layer").len() > 1 {
        let cur = layers.last().expect("a layer");
        let (l, r): (Vec<Digest>, Vec<Digest>) = cur.chunks_exact(2).map(|p| (p[0], p[1])).unzip();
        let mut folded = compress(&l, &r);
        if let Some(g) = groups.get(next_group)
            && g.height == folded.len()
        {
            folded = compress(&folded, &hash_rows(g));
            next_group += 1;
        }
        layers.push(folded);
    }
    debug_assert_eq!(next_group, groups.len(), "every group injected");
    layers
}

impl TreeBackend for CpuTree {
    fn wants(&self, cells: usize) -> bool {
        cells >= self.min_cells
    }

    fn layers(&self, groups: &[Group<'_>]) -> Vec<Vec<Digest>> {
        layers_with(
            groups,
            |g| {
                let (w, rows) = (g.width(), g.concat());
                if w == 0 {
                    return vec![self.hash.hash_slice(&[]); g.height];
                }
                rows.par_chunks_exact(w).map(|r| self.hash.hash_slice(r)).collect()
            },
            |a, b| a.par_iter().zip(b.par_iter()).map(|(x, y)| self.compress.compress([*x, *y])).collect(),
        )
    }
}

/// The prover data: the stock tree, or one built by the backend.
#[derive(Debug)]
pub enum FastTree<M> {
    Stock(MerkleTree<Val, Val, M, 2, DIGEST>),
    Built { leaves: Vec<M>, layers: Vec<Vec<Digest>> },
}

/// The MMCS (module docs).
#[derive(Clone, Debug)]
pub struct FastMmcs<B> {
    pub stock: ValMmcs,
    pub backend: B,
}

impl<B: TreeBackend> FastMmcs<B> {
    /// Over the stock MMCS of `koala_bear::config` (cap height 0).
    pub fn new(backend: B) -> Self {
        let perm = p3_koala_bear::default_koalabear_poseidon2_16();
        Self { stock: ValMmcs::new(Hash::new(perm.clone()), Compress::new(perm), 0), backend }
    }

    fn row_of<M: Matrix<Val>>(m: &M, index: usize, log_max: usize) -> Vec<Val> {
        let reduced = index >> (log_max - log2(m.height()));
        m.row_slice(reduced).expect("row in range").to_vec()
    }
}

fn log2(h: usize) -> usize {
    h.trailing_zeros() as usize
}

impl<B: TreeBackend> Mmcs<Val> for FastMmcs<B> {
    type ProverData<M> = FastTree<M>;
    type Commitment = MerkleCap<Val, Digest>;
    type Proof = Vec<Digest>;
    type MultiProof = PrunedMerklePaths<Val, DIGEST>;
    type Error = MerkleTreeError;

    fn commit<M: Matrix<Val>>(&self, inputs: Vec<M>) -> (Self::Commitment, Self::ProverData<M>) {
        let pow2 = inputs.iter().all(|m| m.height().is_power_of_two());
        let cells: usize = inputs.iter().map(|m| m.height() * m.width()).sum();
        let max = inputs.iter().map(Matrix::height).max().unwrap_or(0);
        // Every height must lie on the halving ladder of the tallest (they are powers of two here).
        if !pow2 || !self.backend.wants(cells) || max < 2 {
            let (c, t) = self.stock.commit(inputs);
            return (c, FastTree::Stock(t));
        }
        let mut heights: Vec<usize> = inputs.iter().map(Matrix::height).collect();
        heights.sort_unstable_by(|a, b| b.cmp(a));
        heights.dedup();
        // Each matrix's values without a copy when it is stored densely (an LDE), else copied.
        let owned: Vec<Option<Vec<Val>>> = inputs
            .iter()
            .map(|m| {
                let dense = m.contiguous_rows(0..m.height()).is_some_and(|v| v.len() == m.height() * m.width());
                (!dense).then(|| (0..m.height()).flat_map(|r| m.row_slice(r).expect("row").to_vec()).collect())
            })
            .collect();
        let guards: Vec<_> = inputs.iter().map(|m| m.contiguous_rows(0..m.height())).collect();
        let slice = |i: usize| -> &[Val] {
            match &owned[i] {
                Some(v) => v,
                None => guards[i].as_deref().expect("dense"),
            }
        };
        let groups: Vec<Group<'_>> = heights
            .iter()
            .map(|&h| Group {
                height: h,
                mats: (0..inputs.len())
                    .filter(|&i| inputs[i].height() == h)
                    .map(|i| (inputs[i].width(), slice(i)))
                    .collect(),
            })
            .collect();
        let layers = self.backend.layers(&groups);
        drop(groups);
        drop(guards);
        let root = layers.last().expect("layers")[0];
        (MerkleCap::new(vec![root]), FastTree::Built { leaves: inputs, layers })
    }

    fn open_batch<M: Matrix<Val>>(&self, index: usize, prover_data: &FastTree<M>) -> BatchOpening<Val, Self> {
        match prover_data {
            FastTree::Stock(t) => {
                let (v, p) = self.stock.open_batch(index, t).unpack();
                BatchOpening::new(v, p)
            }
            FastTree::Built { leaves, layers } => {
                let log_max = log2(layers[0].len());
                let openings = leaves.iter().map(|m| Self::row_of(m, index, log_max)).collect();
                let proof = (0..layers.len() - 1).map(|l| layers[l][(index >> l) ^ 1]).collect();
                BatchOpening::new(openings, proof)
            }
        }
    }

    fn get_matrices<'a, M: Matrix<Val>>(&self, prover_data: &'a FastTree<M>) -> Vec<&'a M> {
        match prover_data {
            FastTree::Stock(t) => self.stock.get_matrices(t),
            FastTree::Built { leaves, .. } => leaves.iter().collect(),
        }
    }

    fn verify_batch(
        &self,
        commit: &Self::Commitment,
        dimensions: &[Dimensions],
        index: usize,
        batch_opening: BatchOpeningRef<'_, Val, Self>,
    ) -> Result<(), Self::Error> {
        let (v, p) = batch_opening.unpack();
        self.stock.verify_batch(commit, dimensions, index, BatchOpeningRef::new(v, p))
    }

    fn open_multi_batch<M: Matrix<Val>>(
        &self,
        indices: &[usize],
        prover_data: &FastTree<M>,
    ) -> (Vec<Vec<Vec<Val>>>, Self::MultiProof) {
        let (leaves, layers) = match prover_data {
            FastTree::Stock(t) => return self.stock.open_multi_batch(indices, t),
            FastTree::Built { leaves, layers } => (leaves, layers),
        };
        let log_max = log2(layers[0].len());
        let values = indices.iter().map(|&i| leaves.iter().map(|m| Self::row_of(m, i, log_max)).collect()).collect();
        // The frontier walk of `p3_merkle_tree::pruning` for arity 2: per level, over the sorted
        // unique nodes, every child of a touched pair that is not itself a node, ascending.
        let mut nodes: Vec<usize> = indices.to_vec();
        nodes.sort_unstable();
        nodes.dedup();
        let mut sibling_hashes = Vec::new();
        for layer in &layers[..layers.len() - 1] {
            let mut parents = Vec::with_capacity(nodes.len());
            let mut i = 0;
            while i < nodes.len() {
                let group = nodes[i] / 2;
                let mut member = i;
                for k in 0..2 {
                    if member < nodes.len() && nodes[member] == 2 * group + k {
                        member += 1;
                    } else {
                        sibling_hashes.push(layer[2 * group + k]);
                    }
                }
                parents.push(group);
                i = member;
            }
            nodes = parents;
        }
        (values, PrunedMerklePaths { sibling_hashes })
    }

    fn verify_multi_batch<R: AsRef<[Val]> + PartialEq>(
        &self,
        commit: &Self::Commitment,
        dimensions: &[Dimensions],
        indices: &[usize],
        opened_values: &[Vec<R>],
        proof: &Self::MultiProof,
    ) -> Result<(), Self::Error> {
        self.stock.verify_multi_batch(commit, dimensions, indices, opened_values, proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p3_matrix::dense::RowMajorMatrix;

    fn mat(seed: u32, h: usize, w: usize) -> RowMajorMatrix<Val> {
        let v = (0..h * w).map(|i| Val::from_u32((i as u32).wrapping_mul(2654435761) ^ seed)).collect();
        RowMajorMatrix::new(v, w)
    }

    /// Every output equals the stock MMCS's: commitment, single and multi openings (rows and
    /// proofs), on single and mixed heights, with several matrices per height.
    #[test]
    fn built_trees_equal_the_stock_mmcs() {
        let fast = FastMmcs::new(CpuTree::new(0));
        let stock = fast.stock.clone();
        let shapes: Vec<Vec<(usize, usize)>> = vec![
            vec![(8, 3)],
            vec![(16, 5), (16, 2)],
            vec![(64, 1), (8, 7), (16, 3), (8, 2), (2, 9)],
            vec![(4, 8), (4, 16), (1 << 10, 1), (32, 33)],
            vec![(2, 1)],
            // wide rows (`WideRowHash` chunks): one matrix past 512 values, several summing past it
            vec![(8, 1500), (4, 513)],
            vec![(16, 300), (16, 300), (64, 2)],
        ];
        for (s, shape) in shapes.iter().enumerate() {
            let ms: Vec<_> = shape.iter().enumerate().map(|(i, &(h, w))| mat(31 * s as u32 + i as u32, h, w)).collect();
            let (c1, t1) = fast.commit(ms.clone());
            assert!(matches!(t1, FastTree::Built { .. }), "shape {s} built by the backend");
            let (c2, t2) = stock.commit(ms.clone());
            assert_eq!(c1, c2, "shape {s}: commitment");
            let max = shape.iter().map(|x| x.0).max().unwrap();
            for index in [0, 1, max / 2, max - 1] {
                let a = fast.open_batch(index, &t1);
                let b = stock.open_batch(index, &t2);
                assert_eq!(a.opened_values, b.opened_values, "shape {s} index {index}: rows");
                assert_eq!(a.opening_proof, b.opening_proof, "shape {s} index {index}: proof");
                let dims: Vec<Dimensions> = ms.iter().map(Matrix::dimensions).collect();
                fast.verify_batch(&c1, &dims, index, BatchOpeningRef::new(&a.opened_values, &a.opening_proof)).unwrap();
            }
            for qs in
                [vec![0usize], vec![max - 1, 0, 3 % max, 3 % max], (0..max).step_by(3).collect(), (0..max).collect()]
            {
                let (va, pa) = fast.open_multi_batch(&qs, &t1);
                let (vb, pb) = stock.open_multi_batch(&qs, &t2);
                assert_eq!(va, vb, "shape {s}: multi rows");
                assert_eq!(pa.sibling_hashes, pb.sibling_hashes, "shape {s} {qs:?}: pruned proof");
            }
        }
    }

    #[test]
    fn small_or_irregular_trees_use_the_stock_mmcs() {
        let fast = FastMmcs::new(CpuTree::new(1 << 20));
        let (_, t) = fast.commit(vec![mat(1, 16, 4)]);
        assert!(matches!(t, FastTree::Stock(_)));
        let fast = FastMmcs::new(CpuTree::new(0));
        let (c, t) = fast.commit(vec![mat(1, 12, 4), mat(2, 6, 1)]);
        assert!(matches!(t, FastTree::Stock(_)));
        assert_eq!(c, fast.stock.commit(vec![mat(1, 12, 4), mat(2, 6, 1)]).0);
    }
}

#[cfg(test)]
mod wide {
    use crate::config::koala_bear::{ROW_CHUNK, Sponge, Val, WideRowHash};
    use alloc::vec::Vec;
    use p3_field::PrimeCharacteristicRing;
    use p3_symmetric::CryptographicHasher;

    #[test]
    fn wide_rows_hash_their_chunk_digests() {
        let perm = p3_koala_bear::default_koalabear_poseidon2_16();
        let (w, s) = (WideRowHash::new(perm.clone()), Sponge::new(perm));
        for n in [0, 1, 8, 511, 512] {
            let v: Vec<Val> = (0..n).map(|i| Val::from_u32(i as u32 * 7 + 1)).collect();
            assert_eq!(w.hash_slice(&v), s.hash_slice(&v), "{n}: the sponge");
        }
        for n in [513, 1024, 1500] {
            let v: Vec<Val> = (0..n).map(|i| Val::from_u32(i as u32 * 7 + 1)).collect();
            let d: Vec<Val> = v.chunks(ROW_CHUNK).flat_map(|c| s.hash_slice(c)).collect();
            assert_eq!(w.hash_slice(&v), s.hash_slice(&d), "{n}: chunk digests");
            assert_eq!(w.hash_iter(v.iter().copied()), w.hash_slice(&v));
            assert_ne!(w.hash_slice(&v), s.hash_slice(&v));
        }
    }
}
