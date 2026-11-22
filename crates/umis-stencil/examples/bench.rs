//! `cargo run --release -p umis-stencil --example bench -- n,k,rpr ...` (default 64,1,2 256,1,2):
//! trace generation, P100 prove and verify of one slot on the stencil framework.
//! `RULE=heat` (16-bit fixed-point diffusion), `lbm`, `gs` (Gray–Scott; batch only) or `life` (default); `BATCH=1`: the batch leaf with
//! lookups (range table, separate hash table; `heat` then uses lookup range checks). `TRACE=1`:
//! Plonky3's prover span timings.
use p3_uni_stark::{prove, verify};
use std::time::Instant;
use umis_stark::config::{FriPreset, koala_bear};
use umis_stencil::Grid;
use umis_stencil::Rule;
use umis_stencil::air::{StencilAir, public_values};
use umis_stencil::batch::StencilBatch;
use umis_stencil::heat::Heat;
use umis_stencil::lbm::{Lbm, U};
use umis_stencil::life::Life;

fn main() {
    let args: Vec<Vec<usize>> =
        std::env::args().skip(1).map(|a| a.split(',').map(|x| x.parse().unwrap()).collect()).collect();
    let layouts = if args.is_empty() { vec![vec![64, 1, 2], vec![256, 1, 2]] } else { args };
    let cfg = koala_bear::config(FriPreset::P100);
    if std::env::var("TRACE").is_ok_and(|v| v == "1") {
        // Plonky3's span timings (prover phases).
        tracing_subscriber::fmt()
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
            .init();
    }
    for l in layouts {
        let batch = std::env::var("BATCH").is_ok_and(|v| v == "1");
        let heat = std::env::var("RULE").as_deref() == Ok("heat");
        if std::env::var("RULE").as_deref() == Ok("lbm") {
            // rest fluid with a density bump in the middle
            let we = l[0] + 2 * l[1];
            let fill = move |i: usize| {
                let (cell, j) = (i / 9, i % 9);
                let (x, y) = ((cell % we) as i64 - we as i64 / 2, (cell / we) as i64 - we as i64 / 2);
                let rho = if x * x + y * y <= 9 { U + 150 } else { U };
                Lbm::equilibrium(rho, (0, 0))[j]
            };
            run_batch(Lbm::default(), &l, fill);
            continue;
        }
        if std::env::var("RULE").as_deref() == Ok("gs") {
            // substrate with a catalyst square in the middle
            let we = l[0] + 2 * l[1];
            let fill = move |i: usize| {
                let (cell, j) = (i / 2, i % 2);
                let (x, y) = ((cell % we) as i64 - we as i64 / 2, (cell / we) as i64 - we as i64 / 2);
                let seed = x.abs() <= 3 && y.abs() <= 3;
                let u = umis_stencil::grayscott::U as u32;
                [if seed { u / 2 } else { u }, if seed { u / 4 } else { 0 }][j]
            };
            run_batch(umis_stencil::grayscott::GrayScott::default(), &l, fill);
            continue;
        }
        if batch {
            if heat {
                run_batch(Heat::new(16, 3, 4, 1000).with_lookups(), &l, |i| {
                    ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 48) as u32
                });
            } else {
                run_batch(Life, &l, |i| ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 5) as u32);
            }
            continue;
        }
        if heat {
            run(&cfg, Heat::new(16, 3, 4, 1000), &l, |i| ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 48) as u32);
        } else {
            run(&cfg, Life, &l, |i| ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 5) as u32);
        }
    }
}

fn run<R: Rule + Clone>(cfg: &koala_bear::Config, rule: R, l: &[usize], fill: impl Fn(usize) -> u32) {
    {
        let air = StencilAir::new(rule, l[0], l[1], l[2]).unwrap();
        let s = air.shape;
        let mut ext = Grid::filled(s.we, s.we, &[0]);
        for (i, c) in ext.data.iter_mut().enumerate() {
            *c = fill(i);
        }
        let t = Instant::now();
        let (trace, next) = air.generate_trace(&ext);
        let public = public_values(&s, &ext, &next);
        let t_gen = t.elapsed();
        let t = Instant::now();
        let proof = prove(cfg, &air, trace, &public).unwrap();
        let t_prove = t.elapsed();
        let t = Instant::now();
        verify(cfg, &air, &proof, &public).unwrap();
        let t_verify = t.elapsed();
        let kb = postcard::to_allocvec(&proof).unwrap().len() / 1024;
        println!(
            "{l:?}: width {} height {} | trace {:.1} ms prove {:.1} ms verify {:.1} ms | {kb} KB",
            s.width(),
            s.height,
            t_gen.as_secs_f64() * 1e3,
            t_prove.as_secs_f64() * 1e3,
            t_verify.as_secs_f64() * 1e3
        );
    }
}

fn run_batch<R: Rule + Clone>(rule: R, l: &[usize], fill: impl Fn(usize) -> u32) {
    // `M=m`: m slots per proof (shared hash and range tables); times are per proof.
    let m = std::env::var("M").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let b = StencilBatch::new_multi(rule, l[0], l[1], l[2], FriPreset::P100, m).unwrap();
    let s = b.shape;
    let mut ext = Grid::filled(s.we, s.we, &vec![0; s.spec.values]);
    for (i, c) in ext.data.iter_mut().enumerate() {
        *c = fill(i);
    }
    let exts = vec![ext; m];
    let t = Instant::now();
    let (traces, public, _) = b.generate_multi(&exts);
    let t_gen = t.elapsed();
    let t = Instant::now();
    let proof = b.prove(&traces, &public).unwrap();
    let t_prove = t.elapsed();
    let t = Instant::now();
    b.verify(&proof, &public).unwrap();
    let t_verify = t.elapsed();
    let kb = postcard::to_allocvec(&proof).unwrap().len() / 1024;
    let per = |d: std::time::Duration| d.as_secs_f64() * 1e3 / m as f64;
    println!(
        "{l:?} batch m={m}: widths {:?} | per slot: trace {:.1} ms prove {:.1} ms verify {:.1} ms {} KB",
        b.widths(),
        per(t_gen),
        per(t_prove),
        per(t_verify),
        kb / m
    );
}
