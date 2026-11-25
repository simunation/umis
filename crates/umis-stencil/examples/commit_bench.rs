//! Where the batch leaf's trace commitment goes (CPU): coset LDE alone vs the full PCS commit
//! (LDE + Poseidon2 Merkle tree), for one cells trace per slot.
//! `RULE=lbm|heat M=8 cargo run --release -p umis-stencil --example commit_bench -- 32,1,2`
use p3_commit::Pcs;
use p3_dft::TwoAdicSubgroupDft;
use p3_field::Field;
use std::time::Instant;
use umis_stark::config::{FriPreset, koala_bear};
use umis_stencil::batch::StencilBatch;
use umis_stencil::heat::Heat;
use umis_stencil::lbm::Lbm;
use umis_stencil::{Grid, Rule};

fn run<R: Rule + Clone>(rule: R, l: &[usize], m: usize) {
    let b = StencilBatch::new_multi(rule.clone(), l[0], l[1], l[2], FriPreset::P100, m).unwrap();
    let ext = Grid::filled(b.shape.we, b.shape.we, &rule.boundary());
    let (traces, _, _) = b.generate_multi(&vec![ext; m]);
    let cfg = koala_bear::config(FriPreset::P100);
    let pcs = p3_uni_stark::StarkGenericConfig::pcs(&cfg);
    let lb = FriPreset::P100.log_blowup();
    let dft = koala_bear::Dft::default();
    for round in 0..3 {
        let t = Instant::now();
        for tr in &traces {
            let _ = dft.coset_lde_batch(tr.clone(), lb, koala_bear::Val::GENERATOR);
        }
        let lde = t.elapsed();
        let t = Instant::now();
        let items: Vec<_> = traces
            .iter()
            .map(|tr| {
                (
                    <_ as Pcs<koala_bear::Challenge, koala_bear::Challenger>>::natural_domain_for_degree(
                        pcs,
                        tr.height(),
                    ),
                    tr.clone(),
                )
            })
            .collect();
        let _ = <_ as Pcs<koala_bear::Challenge, koala_bear::Challenger>>::commit(pcs, items);
        let full = t.elapsed();
        let cells: usize = traces.iter().map(|t| t.width() * t.height()).sum();
        println!(
            "round {round}: {} traces, {cells} cells: LDE {:.1} ms, full commit {:.1} ms (Merkle + rest {:.1} ms)",
            traces.len(),
            lde.as_secs_f64() * 1e3,
            full.as_secs_f64() * 1e3,
            (full.as_secs_f64() - lde.as_secs_f64()) * 1e3
        );
    }
}

use p3_matrix::Matrix;

fn main() {
    let l: Vec<usize> =
        std::env::args().nth(1).unwrap_or("32,1,2".into()).split(',').map(|x| x.parse().unwrap()).collect();
    let m = std::env::var("M").ok().and_then(|x| x.parse().ok()).unwrap_or(8);
    match std::env::var("RULE").as_deref() {
        Ok("heat") => run(Heat::new(16, 3, 4, 1000).with_lookups(), &l, m),
        _ => run(Lbm::default(), &l, m),
    }
}
