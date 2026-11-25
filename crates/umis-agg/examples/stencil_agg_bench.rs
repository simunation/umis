//! Aggregation cost of stencil batch leaves over KoalaBear⁵: two consecutive leaves → two
//! layer-1 proofs (one owner) → one 2-to-1 aggregate.
//! `RULE=heat|lbm|life PARAMS=j101|u101|test cargo run --release -p umis-agg --example stencil_agg_bench -- n,k,rpr ...`
//! `M=m`: instead, one `m`-slot leaf → one layer-1 proof (the cost per slot of aggregating
//! multi-slot leaves, NEXT step 3).
use std::time::Instant;
use umis_agg::native::{NativeLayer1, NativeRelation};
use umis_agg::{AggOwner, AggParams, proof_bytes, recursive_shapes, stencil_chain};
use umis_stark::config::FriPreset;
use umis_stark::security::worst_bits;
use umis_stencil::batch::{StencilBatch, lookup_budget_for, security_shapes_of};
use umis_stencil::heat::Heat;
use umis_stencil::lbm::{Lbm, U};
use umis_stencil::life::Life;
use umis_stencil::{Grid, Rule};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn board<R: Rule>(rule: &R, name: &str, w: usize) -> Grid {
    let mut g = Grid::filled(w, w, &rule.boundary());
    for y in 0..w {
        for x in 0..w {
            let h = ((y * w + x) as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40;
            match name {
                "life" => g.cell_mut(x, y)[0] = (h % 3 == 0) as u32,
                "heat" => g.cell_mut(x, y)[0] = 1000 + (h % 50_000) as u32,
                _ => {
                    let u = (((h % 41) as i64) - 20, ((h / 41 % 41) as i64) - 20);
                    g.cell_mut(x, y).copy_from_slice(&Lbm::equilibrium(U + (h % 64) as i64 - 32, u));
                }
            }
        }
    }
    g
}

fn run<R: Rule + Clone>(p: &AggParams, rule: R, name: &str, l: &[usize]) {
    let b = StencilBatch::new(rule.clone(), l[0], l[1], l[2], FriPreset::Test).expect("shape");
    let t = Instant::now();
    let rel = NativeRelation::new(p, b.airs().to_vec(), b.degrees().to_vec(), lookup_budget_for(p.fri.log_blowup))
        .expect("setup");
    let t_setup = ms(t);
    let ext = board(&rule, name, b.shape.we);
    let (t1, p1, next) = b.generate(&ext);
    let mut ext2 = ext.clone();
    let r0 = b.shape.spec.radius * b.shape.k;
    for y in 0..b.shape.n {
        for x in 0..b.shape.n {
            ext2.cell_mut(x + r0, y + r0).copy_from_slice(next.cell(x, y));
        }
    }
    let (t2, p2, _) = b.generate(&ext2);
    let t = Instant::now();
    let a = rel.prove(p, &t1, b.publics(&p1)).expect("leaf");
    let t_leaf = ms(t);
    let c = rel.prove(p, &t2, b.publics(&p2)).expect("leaf 2");
    let leaf_bits = worst_bits(&security_shapes_of(&a.proof, 5), |s| p.bits(s));
    let t = Instant::now();
    let owner = NativeLayer1::new(p, &rel, &a).expect("owner");
    let t_owner = ms(t);
    let t = Instant::now();
    let ra = owner.prove(&a).expect("layer 1");
    let t_l1 = ms(t);
    let rc = owner.prove(&c).expect("layer 1 (2)");
    ra.verify().expect("verify");
    let t = Instant::now();
    let agg = AggOwner::new(p, &ra, &rc).expect("agg owner");
    let t_agg_owner = ms(t);
    let t = Instant::now();
    let two = agg.prove(&ra, &rc).expect("aggregate");
    let t_agg = ms(t);
    let t = Instant::now();
    two.verify().expect("aggregate verifies");
    let t_ver = ms(t);
    let _ = stencil_chain(&two.statement).expect("chain");
    println!(
        "{name} n={} k={} rpr={}: setup {t_setup:.0} ms, leaf {t_leaf:.0} ms {:.2} MB {leaf_bits:.1} bits; \
         layer-1 owner {t_owner:.0} ms, layer 1 {t_l1:.0} ms {:.2} MB {:.1} bits; agg owner {t_agg_owner:.0} ms, \
         aggregate {t_agg:.0} ms {:.2} MB {:.1} bits, verify {t_ver:.0} ms",
        l[0],
        l[1],
        l[2],
        a.bytes() as f64 / 1e6,
        proof_bytes(&ra.out.0) as f64 / 1e6,
        umis_stark::security::worst_bits(&recursive_shapes(&ra.out.0), |s| p.bits(s)),
        proof_bytes(&two.out.0) as f64 / 1e6,
        umis_stark::security::worst_bits(&recursive_shapes(&two.out.0), |s| p.bits(s)),
    );
}

/// `m` consecutive slots in one leaf and one layer-1 proof over it.
fn run_multi<R: Rule + Clone>(p: &AggParams, rule: R, name: &str, l: &[usize], m: usize) {
    let b = StencilBatch::new_multi(rule.clone(), l[0], l[1], l[2], FriPreset::Test, m).expect("shape");
    let rel = NativeRelation::new(p, b.airs().to_vec(), b.degrees().to_vec(), lookup_budget_for(p.fri.log_blowup))
        .expect("setup");
    let h = b.shape.spec.radius * b.shape.k;
    let mut exts = vec![board(&rule, name, b.shape.we)];
    for _ in 1..m {
        let next = exts.last().unwrap().evolve(&rule, b.shape.k);
        let mut e = exts.last().unwrap().clone();
        for y in 0..b.shape.n {
            for x in 0..b.shape.n {
                e.cell_mut(x + h, y + h).copy_from_slice(next.cell(x, y));
            }
        }
        exts.push(e);
    }
    let (traces, public, _) = b.generate_multi(&exts);
    let t = Instant::now();
    let a = rel.prove(p, &traces, b.publics(&public)).expect("leaf");
    let t_leaf = ms(t);
    let leaf_bits = worst_bits(&security_shapes_of(&a.proof, 5), |s| p.bits(s));
    let t = Instant::now();
    let owner = NativeLayer1::new(p, &rel, &a).expect("owner");
    let t_owner = ms(t);
    let t = Instant::now();
    let ra = owner.prove(&a).expect("layer 1");
    let t_l1 = ms(t);
    let t = Instant::now();
    ra.verify().expect("verify");
    let t_ver = ms(t);
    assert_eq!(ra.statement, public);
    println!(
        "{name} n={} k={} rpr={} m={m}: leaf {t_leaf:.0} ms ({:.0}/slot) {:.2} MB {leaf_bits:.1} bits; layer-1 owner \
         {t_owner:.0} ms, layer 1 {t_l1:.0} ms ({:.0}/slot) {:.2} MB {:.1} bits, verify {t_ver:.0} ms",
        l[0],
        l[1],
        l[2],
        t_leaf / m as f64,
        a.bytes() as f64 / 1e6,
        t_l1 / m as f64,
        proof_bytes(&ra.out.0) as f64 / 1e6,
        umis_stark::security::worst_bits(&recursive_shapes(&ra.out.0), |s| p.bits(s)),
    );
}

fn main() {
    let mut p = match std::env::var("PARAMS").as_deref() {
        Ok("u101") => AggParams::U101,
        Ok("test") => AggParams::TEST,
        _ => AggParams::J101,
    };
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok());
    if let Some(q) = env("QUERIES") {
        p.fri.num_queries = q;
    }
    if let Some(b) = env("COMMIT_POW") {
        p.fri.commit_pow_bits = b;
    }
    if std::env::var("BITS_ONLY").is_ok() {
        let b = StencilBatch::new(Lbm::default(), 32, 1, 2, FriPreset::Test).unwrap();
        let rel = NativeRelation::new(&p, b.airs().to_vec(), b.degrees().to_vec(), lookup_budget_for(p.fri.log_blowup))
            .unwrap();
        let ext = board(&Lbm::default(), "lbm", b.shape.we);
        let (t1, p1, _) = b.generate(&ext);
        let a = rel.prove(&p, &t1, b.publics(&p1)).unwrap();
        let shapes = security_shapes_of(&a.proof, 5);
        println!("{shapes:?}");
        for (q, c) in [(58, 16), (60, 16), (64, 16), (58, 18), (58, 20)] {
            let mut pp = p;
            pp.fri.num_queries = q;
            pp.fri.commit_pow_bits = c;
            println!("q={q} commit_pow={c}: {:.2}", worst_bits(&shapes, |s| pp.bits(s)));
        }
        return;
    }
    let rule = std::env::var("RULE").unwrap_or_else(|_| "heat".into());
    let layouts: Vec<Vec<usize>> = {
        let a: Vec<Vec<usize>> =
            std::env::args().skip(1).map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).collect();
        if a.is_empty() { vec![vec![32, 1, 2]] } else { a }
    };
    if let Some(m) = env("M") {
        for l in layouts {
            match rule.as_str() {
                "life" => run_multi(&p, Life, "life", &l, m),
                "lbm" => run_multi(&p, Lbm::default(), "lbm", &l, m),
                _ => run_multi(&p, Heat::new(16, 3, 4, 1000).with_lookups(), "heat", &l, m),
            }
        }
        return;
    }
    for l in layouts {
        match rule.as_str() {
            "life" => run(&p, Life, "life", &l),
            "lbm" => run(&p, Lbm::default(), "lbm", &l),
            _ => run(&p, Heat::new(16, 3, 4, 1000).with_lookups(), "heat", &l),
        }
    }
}
