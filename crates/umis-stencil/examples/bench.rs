//! `cargo run --release -p umis-stencil --example bench -- n,k,rpr ...` (default 64,1,2 256,1,2):
//! trace generation, P100 prove and verify of one life slot on the stencil framework.
use p3_uni_stark::{prove, verify};
use std::time::Instant;
use umis_stark::config::{FriPreset, koala_bear};
use umis_stencil::Grid;
use umis_stencil::air::{StencilAir, public_values};
use umis_stencil::life::Life;

fn main() {
    let args: Vec<Vec<usize>> =
        std::env::args().skip(1).map(|a| a.split(',').map(|x| x.parse().unwrap()).collect()).collect();
    let layouts = if args.is_empty() { vec![vec![64, 1, 2], vec![256, 1, 2]] } else { args };
    let cfg = koala_bear::config(FriPreset::P100);
    for l in layouts {
        let air = StencilAir::new(Life, l[0], l[1], l[2]).unwrap();
        let s = air.shape;
        let mut ext = Grid::filled(s.we, s.we, &[0]);
        for (i, c) in ext.data.iter_mut().enumerate() {
            *c = ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 60 < 5) as u32;
        }
        let t = Instant::now();
        let (trace, next) = air.generate_trace(&ext);
        let public = public_values(&s, &ext, &next);
        let t_gen = t.elapsed();
        let t = Instant::now();
        let proof = prove(&cfg, &air, trace, &public).unwrap();
        let t_prove = t.elapsed();
        let t = Instant::now();
        verify(&cfg, &air, &proof, &public).unwrap();
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
