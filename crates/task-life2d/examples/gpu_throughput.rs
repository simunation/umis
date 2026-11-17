//! GPU prover throughput: `T` threads proving advancing life2d slots concurrently.
//! `UMIS_GPU=3090 cargo run --release -p task-life2d --features cuda --example gpu_throughput -- n,k,strips,rpr threads proofs [batch]`
//! With `batch` > 1 every proof is one batch STARK over `batch` slots; times are per slot.
use std::time::Instant;
use task_life2d::Life2dParams;
use task_life2d::air::Shape;
use task_life2d::air::gpu::GpuProver;
use task_life2d::board::Board;
use umis_stark::config::FriPreset;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let l: Vec<u32> =
        a.first().map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or(vec![256, 8, 8, 3]);
    let threads: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(4);
    let per: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let batch: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut p = Life2dParams::default_for([1, 1, 0], 4);
    (p.n, p.gens, p.strips, p.rows_per_trace_row) = (l[0], l[1], l[2], l[3]);
    let s = Shape::of(&p);
    let e = (p.n + 2 * p.gens) as usize;
    let mut ext = Board::dead(e, e);
    for (i, c) in ext.cells.iter_mut().enumerate() {
        *c = ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 5) as u8;
    }
    let g = GpuProver::new(FriPreset::Benchmark);
    let _ = g.prove_ext(&s, &Default::default(), 1, &ext, &[false; 8]).unwrap(); // warm up
    let t = Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..threads {
            sc.spawn(|| {
                for j in 0..per {
                    if batch > 1 {
                        let claims: Vec<_> = (0..batch)
                            .map(|b| (Default::default(), (j * batch + b) as u64, &ext, [false; 8]))
                            .collect();
                        let _ = g.prove_ext_batch(&s, &claims).unwrap();
                    } else {
                        let _ = g.prove_ext(&s, &Default::default(), j as u64, &ext, &[false; 8]).unwrap();
                    }
                }
            });
        }
    });
    let n = threads * per * batch;
    let el = t.elapsed();
    println!(
        "{:?} batch {batch}: {threads} threads x {per} proofs: {:?} total, {:.2} ms per slot",
        l,
        el,
        el.as_secs_f64() * 1e3 / n as f64
    );
}
