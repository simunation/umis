//! Time of the 16-bit FRI query grind on the CPU (`RAYON_NUM_THREADS` threads).
use p3_challenger::{CanObserve, GrindingChallenger};
use p3_field::PrimeCharacteristicRing;
use std::time::Instant;
use umis_stark::config::FriPreset;
use umis_stark::config::koala_bear_keccak::config;

fn main() {
    use p3_uni_stark::StarkGenericConfig;
    let c = config(FriPreset::Benchmark);
    let mut total = std::time::Duration::ZERO;
    let n = 20;
    for i in 0..n {
        let mut ch = c.initialise_challenger();
        ch.observe(p3_koala_bear::KoalaBear::from_u64(i));
        let t = Instant::now();
        let _ = ch.grind(16);
        total += t.elapsed();
    }
    println!("grind(16): {:?} mean over {n}", total / n as u32);
}
