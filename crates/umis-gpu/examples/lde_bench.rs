//! GPU vs CPU coset LDE (blowup 2) of KoalaBear matrices:
//! `cargo run --release -p umis-gpu --features cuda --example lde_bench -- [w h ...]`.
use p3_dft::{Radix2DitParallel, TwoAdicSubgroupDft};
use p3_field::{Field, PrimeCharacteristicRing};
use p3_koala_bear::KoalaBear;
use p3_matrix::dense::RowMajorMatrix;
use std::time::Instant;
use umis_gpu::GpuDft;

fn main() {
    let a: Vec<usize> = std::env::args().skip(1).filter_map(|x| x.parse().ok()).collect();
    let shapes: Vec<(usize, usize)> = if a.len() >= 2 {
        a.chunks(2).map(|c| (c[0], c[1])).collect()
    } else {
        vec![(352, 256), (2016, 1024), (8064, 1024), (2016, 8192)]
    };
    let cpu = Radix2DitParallel::<KoalaBear>::default();
    let gpu = GpuDft::default();
    assert!(umis_gpu::gpu_available(), "no GPU");
    for (w, h) in shapes {
        let m = RowMajorMatrix::new((0..w * h).map(|i| KoalaBear::from_u32(i as u32 % 1_000_003)).collect(), w);
        let best = |f: &dyn Fn()| {
            (0..5)
                .map(|_| {
                    let t = Instant::now();
                    f();
                    t.elapsed()
                })
                .min()
                .unwrap()
        };
        let s = KoalaBear::GENERATOR;
        let tc = best(&|| {
            let _ = std::hint::black_box(cpu.coset_lde_batch(m.clone(), 1, s));
        });
        let tg = best(&|| {
            let _ = std::hint::black_box(gpu.coset_lde_batch(m.clone(), 1, s));
        });
        println!(
            "{w:>5} x {h:>5} ({:6.1} MB): cpu {tc:>9.2?}  gpu {tg:>9.2?}  ({:.2}x)",
            (w * h * 4) as f64 / 1e6,
            tc.as_secs_f64() / tg.as_secs_f64()
        );
        let _ = s.inverse();
    }
}
