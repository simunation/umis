//! Merkle commitment throughput of KoalaBear matrices: Poseidon2 (the current
//! config) vs Keccak-f (AVX2 x4 lanes). `cargo run --release -p umis-stark
//! --example mmcs_bench -- [width height]`; threads: `RAYON_NUM_THREADS`.
use p3_commit::Mmcs;
use p3_field::PrimeCharacteristicRing;
use p3_keccak::{KeccakF, VECTOR_LEN};
use p3_koala_bear::KoalaBear;
use p3_matrix::dense::RowMajorMatrix;
use p3_merkle_tree::MerkleTreeMmcs;
use p3_symmetric::{CompressionFunctionFromHasher, PaddingFreeSponge, SerializingHasher};
use std::time::Instant;
use umis_stark::config::koala_bear::ValMmcs;

fn main() {
    let a: Vec<usize> = std::env::args().skip(1).filter_map(|x| x.parse().ok()).collect();
    let (w, h) = if a.len() == 2 { (a[0], a[1]) } else { (2016, 2048) };
    let m = RowMajorMatrix::new(
        (0..w * h).map(|i| KoalaBear::from_u32((i as u32).wrapping_mul(2654435761) >> 1)).collect(),
        w,
    );
    let reps = 3;
    let best = |f: &dyn Fn()| {
        (0..reps)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed()
            })
            .min()
            .unwrap()
    };
    // Poseidon2 (as in umis_stark::config::koala_bear)
    let cfg = umis_stark::config::koala_bear::config(umis_stark::config::FriPreset::Benchmark);
    let _ = cfg;
    let perm = p3_koala_bear::default_koalabear_poseidon2_16();
    let p2: ValMmcs = ValMmcs::new(
        umis_stark::config::koala_bear::Hash::new(perm.clone()),
        umis_stark::config::koala_bear::Compress::new(perm),
        0,
    );
    let t_p2 = best(&|| {
        let _ = p2.commit(vec![m.clone()]);
    });
    // Keccak-f[1600], 4 AVX2 lanes, field elements serialized into u64 words
    let sponge = PaddingFreeSponge::<KeccakF, 25, 17, 4>::new(KeccakF);
    let kc = MerkleTreeMmcs::<[KoalaBear; VECTOR_LEN], [u64; VECTOR_LEN], _, _, 2, 4>::new(
        SerializingHasher::new(sponge),
        CompressionFunctionFromHasher::new(sponge),
        0,
    );
    let _ = [KoalaBear::ZERO; VECTOR_LEN];
    let t_kc = best(&|| {
        let _ = kc.commit(vec![m.clone()]);
    });
    let t_clone = best(&|| {
        let _ = std::hint::black_box(m.clone());
    });
    let mb = (w * h * 4) as f64 / 1e6;
    println!(
        "{w}x{h} ({mb:.1} MB): poseidon2 {:?} ({:.0} MB/s)  keccak {:?} ({:.0} MB/s)  [clone {:?}]",
        t_p2,
        mb / t_p2.as_secs_f64(),
        t_kc,
        mb / t_kc.as_secs_f64(),
        t_clone
    );
}
