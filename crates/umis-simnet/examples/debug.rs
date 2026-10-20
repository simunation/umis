use umis_protocol::Command;
use umis_simnet::{Sim, SimConfig};
fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let mut sim = Sim::new(SimConfig::new(0));
    let mut late = vec![];
    for e in 1..40u64 {
        let t0 = std::time::Instant::now();
        let ev0 = sim.net.events;
        if e == 12 {
            sim.net.set_online(10, false);
            sim.net.set_online(11, false);
        }
        if e == 15 {
            for k in 0..3 {
                late.push(sim.add_node(&format!("late{k}")));
            }
            for &l in &late {
                let to = sim.node(l).me;
                sim.command(0, Command::Grant { to, amount: 500 });
            }
        }
        if e == 18 {
            for &l in &late {
                sim.command(l, Command::Join { stake: 200, full_node: false });
            }
        }
        if e == 21 {
            sim.net.set_online(10, true);
            sim.net.set_online(11, true);
        }
        sim.run_to_slot(e);
        let a = sim.node(0);
        let st = a.chain.head_state();
        let lj: Vec<_> = late
            .iter()
            .map(|l| {
                let n = sim.node(*l);
                (
                    n.chain.head().slot,
                    n.my_zone(),
                    n.engines.values().map(|e| e.has_root()).collect::<Vec<_>>(),
                    n.stats.zone_blocks_accepted,
                )
            })
            .collect();
        println!(
            "slot {e}: {:?} events {} head {:?} rollbacks {} late {:?} back {:?}",
            t0.elapsed(),
            sim.net.events - ev0,
            a.chain.head().slot,
            st.rollback_log.len(),
            lj,
            sim.node(10).chain.head().slot
        );
    }
}
