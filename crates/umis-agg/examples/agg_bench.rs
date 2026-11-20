//! Aggregation cost: life2d leaf (R100) → layer-1 recursive proof → 2-to-1 aggregate.
//! `cargo run --release -p umis-agg --example agg_bench -- n,k,strips,rpr ...` (default 64,1,4,2)
//! `PRESET=test` uses the 4-query test preset (fast, insecure) for the leaf and the layers.
use std::time::Instant;
use task_life2d::Life2dParams;
use task_life2d::board::Board;
use umis_agg::{AggConfig, aggregate, layer1, leaf, proof_bytes, verify_recursive};
use umis_stark::config::FriPreset;

fn main() {
    let preset = if std::env::var("PRESET").as_deref() == Ok("test") { FriPreset::Test } else { FriPreset::R100 };
    let cfg = AggConfig::new(preset);
    let layouts: Vec<Vec<u32>> = {
        let a: Vec<Vec<u32>> =
            std::env::args().skip(1).map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).collect();
        if a.is_empty() { vec![vec![64, 1, 4, 2]] } else { a }
    };
    for l in layouts {
        let mut p = Life2dParams::default_for([1, 1, 0], 4);
        (p.n, p.gens, p.strips, p.rows_per_trace_row) = (l[0], l[1], l[2], l[3]);
        let e = (p.n + 2 * p.gens) as usize;
        let mut ext = Board::dead(e, e);
        for (i, c) in ext.cells.iter_mut().enumerate() {
            *c = ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 5) as u8;
        }
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let a = leaf(&cfg, &p, 1, &ext, &[false; 8]);
        let t_leaf = ms(t);
        let b = leaf(&cfg, &p, 2, &ext, &[false; 8]);
        let leaf_kb = postcard::to_allocvec(&a.proof).unwrap().len() / 1024;
        let t = Instant::now();
        let ra = layer1(&cfg, &a).expect("layer 1");
        let t_l1 = ms(t);
        let rb = layer1(&cfg, &b).expect("layer 1");
        let t = Instant::now();
        verify_recursive(&cfg, &ra).expect("layer-1 proof verifies");
        let t_v1 = ms(t);
        let t = Instant::now();
        let agg = aggregate(&cfg, &ra, &rb).expect("aggregate");
        let t_agg = ms(t);
        let t = Instant::now();
        verify_recursive(&cfg, &agg).expect("aggregate verifies");
        let t_va = ms(t);
        for (name, out) in [("layer1", &ra), ("2-to-1", &agg)] {
            let sh = umis_agg::recursive_shape(&out.0);
            eprintln!(
                "  {name}: tables {} height 2^{} openings {} UDR bits (R100) {:.1}",
                out.0.proof.degree_bits.len(),
                sh.log_height,
                sh.batched_openings,
                umis_stark::security::proven_udr_bits(preset, &sh)
            );
        }
        println!(
            "{l:?} {preset:?}: leaf {t_leaf:.1} ms {leaf_kb} KB | layer1 {t_l1:.0} ms {} KB verify {t_v1:.1} ms | 2-to-1 {t_agg:.0} ms {} KB verify {t_va:.1} ms",
            proof_bytes(&ra.0) / 1024,
            proof_bytes(&agg.0) / 1024
        );
    }
}
