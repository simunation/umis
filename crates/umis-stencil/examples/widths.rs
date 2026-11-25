//! Column budget of a batch leaf: `RULE=lbm|heat|life cargo run --release -p umis-stencil --example widths -- n,k,rpr`.
use umis_stark::config::FriPreset;
use umis_stencil::batch::StencilBatch;
use umis_stencil::lbm::{Lbm, U};
use umis_stencil::{Grid, Rule};

fn run<R: Rule + Clone>(rule: R, l: &[usize]) {
    let b = StencilBatch::new(rule.clone(), l[0], l[1], l[2], FriPreset::Test).expect("shape");
    let mut ext = Grid::filled(b.shape.we, b.shape.we, &rule.boundary());
    for y in 0..b.shape.we {
        for x in 0..b.shape.we {
            let h = ((y * b.shape.we + x) as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40;
            let c = ext.cell_mut(x, y);
            if c.len() == 9 {
                let u = (((h % 41) as i64) - 20, ((h / 41 % 41) as i64) - 20);
                c.copy_from_slice(&Lbm::equilibrium(U + (h % 64) as i64 - 32, u));
            }
        }
    }
    let (t, p, _) = b.generate(&ext);
    let proof = b.prove(&t, &p).expect("prove");
    println!("cells width {} (rule part {}), widths {:?}", b.shape.batch_width(), b.shape.cells_width(), b.widths());
    for (i, o) in proof.opened_values.instances.iter().enumerate() {
        let v = &o.base_opened_values;
        println!(
            "instance {i}: height 2^{} main {} perm(EF) {} quotient(EF) {} preprocessed {}",
            proof.degree_bits[i],
            v.trace_local.len(),
            o.permutation_local.len(),
            v.quotient_chunks.iter().map(|c| c.len()).sum::<usize>(),
            v.preprocessed_local().map_or(0, |p| p.len()),
        );
    }
}

fn main() {
    let l: Vec<usize> =
        std::env::args().nth(1).unwrap_or("32,1,2".into()).split(',').map(|x| x.parse().unwrap()).collect();
    match std::env::var("RULE").as_deref() {
        Ok("life") => run(umis_stencil::life::Life, &l),
        _ => run(Lbm::default(), &l),
    }
}
