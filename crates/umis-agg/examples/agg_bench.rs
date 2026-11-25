//! Aggregation cost over KoalaBear⁵: life2d leaf → layer-1 recursive proof → 2-to-1 aggregate.
//! `cargo run --release -p umis-agg --example agg_bench -- n,k,strips,rpr ...` (default 64,1,4,2)
//! `PARAMS=u101` (unique decoding), `j101` (Johnson, default) or `test`. `DIGEST=1`: the
//! claim-independent digest leaf with owners built once and reused (steady state).
use std::time::Instant;
use task_life2d::Life2dParams;
use task_life2d::board::Board;
use umis_agg::{
    AggOwner, AggParams, Layer1Owner, aggregate, config, digest_leaf, layer1, leaf, proof_bytes, recursive_shapes,
};

fn main() {
    let p = match std::env::var("PARAMS").as_deref() {
        Ok("u101") => AggParams::U101,
        Ok("test") => AggParams::TEST,
        _ => AggParams::J101,
    };
    let cfg = config(&p);
    let layouts: Vec<Vec<u32>> = {
        let a: Vec<Vec<u32>> =
            std::env::args().skip(1).map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).collect();
        if a.is_empty() { vec![vec![64, 1, 4, 2]] } else { a }
    };
    for l in layouts {
        let mut lp = Life2dParams::default_for([1, 1, 0], 4);
        (lp.n, lp.gens, lp.strips, lp.rows_per_trace_row) = (l[0], l[1], l[2], l[3]);
        let e = (lp.n + 2 * lp.gens) as usize;
        let mut ext = Board::dead(e, e);
        for (i, c) in ext.cells.iter_mut().enumerate() {
            *c = ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 5) as u8;
        }
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
        if std::env::var("DIGEST").is_ok_and(|v| v == "1") {
            let mut ext2 = ext.clone();
            ext2.cells.rotate_left(7);
            let t = Instant::now();
            let a = digest_leaf(&cfg, &lp, &ext, &[false; 8]);
            let t_leaf = ms(t);
            let b = digest_leaf(&cfg, &lp, &ext2, &[false; 8]);
            let l1 = Layer1Owner::new(&p, &a).expect("layer-1 owner");
            let (ra, rb) = (l1.prove(&a).expect("layer 1"), l1.prove(&b).expect("layer 1"));
            ra.verify().expect("verifies");
            rb.verify().expect("verifies");
            let agg = AggOwner::new(&p, &ra, &rb).expect("2-to-1 owner");
            let ab = agg.prove(&ra, &rb).expect("2-to-1");
            let ba = agg.prove(&rb, &ra).expect("2-to-1");
            ab.verify().expect("verifies");
            ba.verify().expect("verifies");
            let ms_d = |d: std::time::Duration| d.as_secs_f64() * 1e3;
            println!(
                "{l:?} digest {:?}/{}q lb{}: leaf {t_leaf:.1} ms {} KB width {} bits {:.1} | layer1 build {:.0} prove {:.0}/{:.0} ms {} KB bits {:.1} | 2-to-1 build {:.0} prove {:.0}/{:.0} ms {} KB bits {:.1}",
                p.regime,
                p.fri.num_queries,
                p.fri.log_blowup,
                postcard::to_allocvec(&a.proof).unwrap().len() / 1024,
                a.air.shape.width(),
                p.bits(&a.shape()),
                ms_d(l1.build),
                ms_d(ra.timing.prove),
                ms_d(rb.timing.prove),
                proof_bytes(&ra.out.0) / 1024,
                umis_stark::security::worst_bits(&recursive_shapes(&ra.out.0), |s| p.bits(s)),
                ms_d(agg.build),
                ms_d(ab.timing.prove),
                ms_d(ba.timing.prove),
                proof_bytes(&ab.out.0) / 1024,
                umis_stark::security::worst_bits(&recursive_shapes(&ab.out.0), |s| p.bits(s)),
            );
            continue;
        }
        let t = Instant::now();
        let a = leaf(&cfg, &lp, 1, &ext, &[false; 8]);
        let t_leaf = ms(t);
        let b = leaf(&cfg, &lp, 2, &ext, &[false; 8]);
        let leaf_kb = postcard::to_allocvec(&a.proof).unwrap().len() / 1024;
        let t = Instant::now();
        let ra = layer1(&p, &a).expect("layer 1");
        let t_l1 = ms(t);
        let rb = layer1(&p, &b).expect("layer 1");
        let t = Instant::now();
        ra.verify().expect("layer-1 proof verifies");
        let t_v1 = ms(t);
        let t = Instant::now();
        let agg = aggregate(&p, &ra, &rb).expect("aggregate");
        let t_agg = ms(t);
        let t = Instant::now();
        agg.verify().expect("aggregate verifies");
        let t_va = ms(t);
        eprintln!("  leaf: {:.1} bits", p.bits(&a.shape()));
        for (name, r) in [("layer1", &ra), ("2-to-1", &agg)] {
            let shapes = recursive_shapes(&r.out.0);
            let sh = shapes[0];
            eprintln!(
                "  {name}: build {:.0} ms prove {:.0} ms | tables {} log heights {:?} openings {} max degree {} bits {:.1}",
                r.timing.build.as_secs_f64() * 1e3,
                r.timing.prove.as_secs_f64() * 1e3,
                r.out.0.proof.degree_bits.len(),
                r.out.0.proof.degree_bits,
                sh.batched_openings,
                sh.max_degree,
                umis_stark::security::worst_bits(&shapes, |s| p.bits(s))
            );
        }
        println!(
            "{l:?} {:?}/{}q lb{}: leaf {t_leaf:.1} ms {leaf_kb} KB | layer1 {t_l1:.0} ms {} KB verify {t_v1:.1} ms | 2-to-1 {t_agg:.0} ms {} KB verify {t_va:.1} ms",
            p.regime,
            p.fri.num_queries,
            p.fri.log_blowup,
            proof_bytes(&ra.out.0) / 1024,
            proof_bytes(&agg.out.0) / 1024
        );
    }
}
