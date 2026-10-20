//! Toy AIR benchmark: prove / verify time and proof size, Goldilocks vs KoalaBear.
//!
//! ```text
//! cargo run -p umis-stark --release --example bench_toy -- [log_rows ...]   # default: 14 16
//! ```
//!
//! Shape: N = 256 cells, T = 2^log_rows / 256 steps, 50 main columns (8 + 42 dummies),
//! FRI benchmark preset (blowup 2, 100 queries, 16 PoW bits). Each row is measured with the
//! default lookup packing and with same-bus folding up to degree 3.

use std::time::Instant;

use p3_field::PrimeField64;
use p3_matrix::Matrix;
use umis_stark::config::{FriPreset, goldilocks, koala_bear};
use umis_stark::toy::{ToyParams, honest_instance, proof_size, prove, setup, setup_with_lookup_budget, verify};

const N_CELLS: usize = 256;
const EXTRA_COLS: usize = 42;
const REPS: usize = 3;

macro_rules! bench_field {
    ($name:literal, $field:ident, $log_rows:expr) => {{
        let config = $field::config(FriPreset::Benchmark);
        let params = ToyParams::new(N_CELLS, (1 << $log_rows) / N_CELLS, EXTRA_COLS);
        let t = Instant::now();
        let (air, trace) = honest_instance::<$field::Val>(params, 42);
        let witness_ms = ms(t);
        for (label, packed) in [("default", false), ("fold<=3", true)] {
            let t = Instant::now();
            let pd = if packed { setup_with_lookup_budget(&config, &air, 3, 1) } else { setup(&config, &air) };
            let setup_ms = ms(t);
            let n_lookups = pd.common.lookups[0].len();
            // Warm-up, then best of REPS.
            let mut proof = prove(&config, &air, &trace, &pd).expect("prove");
            let mut prove_ms = f64::MAX;
            for _ in 0..REPS {
                let t = Instant::now();
                proof = prove(&config, &air, &trace, &pd).expect("prove");
                prove_ms = prove_ms.min(ms(t));
            }
            let mut verify_ms = f64::MAX;
            for _ in 0..REPS {
                let t = Instant::now();
                verify(&config, &air, &proof, &pd).expect("verify");
                verify_ms = verify_ms.min(ms(t));
            }
            println!(
                "{:<10} 2^{:<2} {:>3} cols {:<8} perm cols {:>2} EF  | witness {:>6.1} ms  setup {:>6.1} ms  \
                 prove {:>8.1} ms  verify {:>6.1} ms  proof {:>7} B",
                $name,
                $log_rows,
                trace.width(),
                label,
                n_lookups + 1,
                witness_ms,
                setup_ms,
                prove_ms,
                verify_ms,
                proof_size(&proof),
            );
        }
    }};
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).map(|a| a.parse().expect("log_rows must be an integer")).collect();
    let logs = if args.is_empty() { vec![14, 16] } else { args };
    println!(
        "threads: {}, Goldilocks p = {}, KoalaBear p = {}",
        std::thread::available_parallelism().map_or(1, |n| n.get()),
        goldilocks::Val::ORDER_U64,
        koala_bear::Val::ORDER_U64
    );
    for log_rows in logs {
        assert!(log_rows >= 8, "need at least 2^8 rows");
        bench_field!("goldilocks", goldilocks, log_rows);
        bench_field!("koalabear", koala_bear, log_rows);
    }
}
