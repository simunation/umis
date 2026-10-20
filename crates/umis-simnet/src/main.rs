//! `umis-simnet --seeds 100 --scenario churn|partition|adversary|faults|zone-create|...|all`
//!
//! Runs every (scenario, seed) pair on all cores and checks the invariants.

use clap::Parser;
use std::sync::Mutex;
use umis_simnet::scenarios::{self, Scenario};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = 1)]
    seeds: u64,
    #[arg(long, default_value_t = 0)]
    first_seed: u64,
    /// Comma-separated scenario names, or `all`.
    #[arg(long, default_value = "basic")]
    scenario: String,
    #[arg(long, default_value_t = 200)]
    slots: u64,
    /// Worker threads (default: all cores).
    #[arg(long)]
    threads: Option<usize>,
    /// Also rerun each pair and require an identical trace hash.
    #[arg(long)]
    determinism: bool,
}

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let a = Args::parse();
    let names: Vec<String> = if a.scenario == "all" {
        scenarios::ALL.iter().map(|s| s.to_string()).collect()
    } else {
        a.scenario.split(',').map(|s| s.to_string()).collect()
    };
    let mut jobs = Vec::new();
    for n in &names {
        let s: Scenario = n.parse().expect("scenario");
        for seed in a.first_seed..a.first_seed + a.seeds {
            jobs.push((n.clone(), s, seed));
        }
    }
    let total = jobs.len();
    let queue = Mutex::new(jobs);
    let results = Mutex::new(Vec::new());
    let threads = a.threads.unwrap_or_else(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4));
    let t0 = std::time::Instant::now();
    std::thread::scope(|sc| {
        for _ in 0..threads.min(total.max(1)) {
            sc.spawn(|| {
                loop {
                    let Some((name, s, seed)) = queue.lock().expect("queue").pop() else { break };
                    let t = std::time::Instant::now();
                    let o = scenarios::run(s, seed, a.slots);
                    let mut violations = o.report.violations.clone();
                    if a.determinism {
                        let o2 = scenarios::run(s, seed, a.slots);
                        if o2.trace != o.trace {
                            violations.push("same seed gave a different trace".into());
                        }
                    }
                    let r = &o.report;
                    let line = format!(
                        "{name:12} seed {seed:3}: head {} fin {} zones {} commits {:?} rollbacks {} slashes {} gaps {:?} stale {:.0}ms reverts {} {:.1?} {}",
                        r.head_slot,
                        r.finalized_slot,
                        r.zones,
                        r.commits.values().collect::<Vec<_>>(),
                        r.rollbacks,
                        r.slashes,
                        r.max_commit_gap.values().collect::<Vec<_>>(),
                        r.avg_staleness_ms,
                        o.sim.net.nodes.iter().map(|n| n.node.chain.finality_reverts).sum::<u64>(),
                        t.elapsed(),
                        if violations.is_empty() { "OK" } else { "FAIL" }
                    );
                    println!("{line}");
                    for v in &violations {
                        println!("    {v}");
                    }
                    results.lock().expect("results").push((name, seed, violations.is_empty()));
                }
            });
        }
    });
    let results = results.into_inner().expect("results");
    let failed: Vec<_> = results.iter().filter(|r| !r.2).collect();
    println!("{} runs, {} failed, {:.1?}", results.len(), failed.len(), t0.elapsed());
    for f in &failed {
        println!("  FAILED {} seed {}", f.0, f.1);
    }
    if !failed.is_empty() {
        std::process::exit(1);
    }
}
