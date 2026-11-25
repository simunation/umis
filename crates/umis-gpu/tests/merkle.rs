//! The GPU Merkle backend equals the CPU reference layer for layer, and through `FastMmcs`
//! the stock MMCS (commitment, pruned multi-proofs). Skipped without a usable GPU.
#![cfg(feature = "cuda")]

use p3_commit::Mmcs;
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;
use umis_gpu::merkle::GpuTree;
use umis_stark::config::koala_bear::Val;
use umis_stark::fast_mmcs::{CpuTree, FastMmcs, FastTree, TreeBackend};

fn mat(seed: u32, h: usize, w: usize) -> RowMajorMatrix<Val> {
    let v = (0..h * w).map(|i| Val::from_u32((i as u32).wrapping_mul(2654435761) ^ seed)).collect();
    RowMajorMatrix::new(v, w)
}

#[test]
fn gpu_trees_equal_the_cpu_reference_and_the_stock_mmcs() {
    if !umis_gpu::gpu_available() {
        eprintln!("no GPU: skipped");
        return;
    }
    let shapes: Vec<Vec<(usize, usize)>> = vec![
        vec![(8, 3)],
        vec![(16, 5), (16, 2)],
        vec![(64, 1), (8, 7), (16, 3), (8, 2), (2, 9)],
        vec![(4, 8), (4, 16), (1 << 12, 1), (32, 33)],
        vec![(256, 5548), (256, 5548), (1 << 16, 20), (1 << 17, 1)],
    ];
    let gpu = FastMmcs::new(GpuTree::with_min_cells(0));
    let cpu = FastMmcs::new(CpuTree::new(0));
    for (s, shape) in shapes.iter().enumerate() {
        let ms: Vec<_> = shape.iter().enumerate().map(|(i, &(h, w))| mat(97 * s as u32 + i as u32, h, w)).collect();
        let (c1, t1) = gpu.commit(ms.clone());
        let (c2, t2) = cpu.commit(ms.clone());
        let (c3, _) = gpu.stock.commit(ms.clone());
        assert_eq!(c1, c2, "shape {s}: gpu vs cpu reference");
        assert_eq!(c1, c3, "shape {s}: gpu vs stock");
        if let (FastTree::Built { layers: a, .. }, FastTree::Built { layers: b, .. }) = (&t1, &t2) {
            assert_eq!(a, b, "shape {s}: every layer");
        } else {
            panic!("shape {s}: both built by a backend");
        }
        let max = shape.iter().map(|x| x.0).max().unwrap();
        let qs: Vec<usize> = (0..max).step_by((max / 7).max(1)).collect();
        assert_eq!(gpu.open_multi_batch(&qs, &t1).1.sibling_hashes, cpu.open_multi_batch(&qs, &t2).1.sibling_hashes);
    }
    assert!(GpuTree::default().wants(1 << 16) && !GpuTree::default().wants(1 << 10));
}
