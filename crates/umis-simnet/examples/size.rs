use umis_simnet::{Sim, SimConfig};
fn main() {
    let mut sim = Sim::new(SimConfig::new(0));
    sim.run_to_slot(15);
    let st = sim.node(3).chain.head_state().clone();
    let total = borsh::to_vec(&*st).unwrap().len();
    let commits = borsh::to_vec(&st.commits).unwrap().len();
    let zones = borsh::to_vec(&st.zones).unwrap().len();
    let nodes = borsh::to_vec(&st.nodes).unwrap().len();
    println!("total {total} commits {commits} zones {zones} nodes {nodes}");
    for (z, l) in &st.commits {
        println!(
            "zone {z}: {} commits {:?}",
            l.len(),
            l.iter().map(|c| (c.e, format!("{:?}", c.status))).collect::<Vec<_>>()
        );
    }
    let t = std::time::Instant::now();
    for _ in 0..100 {
        let _ = st.hash();
    }
    println!("hash {:?}", t.elapsed() / 100);
    let t = std::time::Instant::now();
    for _ in 0..100 {
        let _ = (*st).clone();
    }
    println!("clone {:?}", t.elapsed() / 100);
}
