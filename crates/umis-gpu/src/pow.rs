//! Proof-of-work grinding on the GPU: Poseidon2 (KoalaBear, width 16, as
//! `default_koalabear_poseidon2_16`) over many candidates at once, for
//! `HybridChallenger::grind` (`umis_stark::config::koala_bear_keccak::set_grinder`).

use crate::lde::{device, raw};
use crate::resident::Val;
use cudarc::driver::{LaunchConfig, PushKernelArg};
use p3_field::{Field, PrimeCharacteristicRing};
use p3_koala_bear::{
    KOALABEAR_POSEIDON2_RC_16_EXTERNAL_FINAL, KOALABEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL,
    KOALABEAR_POSEIDON2_RC_16_INTERNAL,
};

/// The permutation's constants as `p2_permute` (resident.cu) reads them.
fn constants() -> Vec<u32> {
    let mut v = Vec::with_capacity(164);
    v.extend(KOALABEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL.iter().flatten().map(|&x| raw(x)));
    v.extend(KOALABEAR_POSEIDON2_RC_16_INTERNAL.iter().map(|&x| raw(x)));
    v.extend(KOALABEAR_POSEIDON2_RC_16_EXTERNAL_FINAL.iter().flatten().map(|&x| raw(x)));
    // Internal diagonal V (`KoalaBearInternalLayerParameters`): -2, 1, 2, 1/2, 3, 4, -1/2,
    // -3, -4, 1/2^8, 1/8, 1/2^24, -1/2^8, -1/8, -1/16, -1/2^24 (V[0] is not used).
    let inv2 = |k: u64| Val::from_u64(1u64 << k).inverse();
    let d = [
        -Val::TWO,
        Val::ONE,
        Val::TWO,
        inv2(1),
        Val::from_u64(3),
        Val::from_u64(4),
        -inv2(1),
        -Val::from_u64(3),
        -Val::from_u64(4),
        inv2(8),
        inv2(3),
        inv2(24),
        -inv2(8),
        -inv2(3),
        -inv2(4),
        -inv2(24),
    ];
    v.extend(d.iter().map(|&x| raw(x)));
    v
}

/// Permutes states with the GPU Poseidon2 (tests).
pub fn permute_states(states: &mut [[Val; 16]]) -> Result<(), String> {
    let dev = device().ok_or("no GPU")?;
    let d = dev.lock().map_err(|e| e.to_string())?;
    let s = d.stream.clone();
    let e = |x: cudarc::driver::DriverError| x.to_string();
    let flat: Vec<u32> = states.iter().flatten().map(|&x| raw(x)).collect();
    let mut buf = s.clone_htod(&flat).map_err(e)?;
    let rc = s.clone_htod(&constants()).map_err(e)?;
    let n = states.len() as u32;
    // SAFETY: `buf` holds `n` states of 16 elements.
    unsafe {
        s.launch_builder(&d.p2_permute_states)
            .arg(&mut buf)
            .arg(&rc)
            .arg(&n)
            .launch(LaunchConfig { grid_dim: (n.div_ceil(64), 1, 1), block_dim: (64, 1, 1), shared_mem_bytes: 0 })
            .map_err(e)?;
    }
    let out = crate::lde::from_raw_vec(s.clone_dtoh(&buf).map_err(e)?);
    for (st, c) in states.iter_mut().zip(out.chunks_exact(16)) {
        st.copy_from_slice(c);
    }
    Ok(())
}

/// The smallest candidate below `2^32` (canonical value) passing the search of
/// `DuplexChallenger::grind` from `base` with the witness in rate slot `widx`, or `None`.
pub fn grind(base: &[Val; 16], widx: usize, bits: usize) -> Option<Val> {
    if widx >= 8 || bits == 0 || bits >= 32 {
        return None;
    }
    let dev = device()?;
    let d = dev.lock().ok()?;
    let s = d.stream.clone();
    let rc = s.clone_htod(&constants()).ok()?;
    let b: Vec<u32> = base.iter().map(|&x| raw(x)).collect();
    let d_base = s.clone_htod(&b).ok()?;
    let mask = ((1u64 << bits) - 1) as u32;
    let r2 = raw(Val::from_u64((1u64 << 32) % 0x7f00_0001));
    let wi = widx as u32;
    // 4 * 2^bits candidates per launch: one launch suffices with probability 1 - e^-4.
    let chunk: u32 = 1u32 << (bits + 2).clamp(10, 26);
    let t0 = std::time::Instant::now();
    let mut start = 0u32;
    while (start as u64) < 0x7f00_0001 {
        let mut found = s.clone_htod(&[u32::MAX]).ok()?;
        // SAFETY: `base` has 16 elements, `rc` the 164 constants, `found` one word.
        unsafe {
            s.launch_builder(&d.p2_grind)
                .arg(&d_base)
                .arg(&wi)
                .arg(&rc)
                .arg(&mask)
                .arg(&start)
                .arg(&r2)
                .arg(&mut found)
                .launch(LaunchConfig {
                    grid_dim: (chunk.div_ceil(256), 1, 1),
                    block_dim: (256, 1, 1),
                    shared_mem_bytes: 0,
                })
                .ok()?;
        }
        let f: Vec<u32> = s.clone_dtoh(&found).ok()?;
        if f[0] != u32::MAX {
            if std::env::var_os("UMIS_GPU_PROFILE").is_some() {
                eprintln!("    gpu grind {bits} bits: {:?}", t0.elapsed());
            }
            return Some(Val::from_u32(f[0]));
        }
        start = start.checked_add(chunk)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use p3_challenger::{CanObserve, GrindingChallenger};
    use p3_symmetric::Permutation;
    use p3_uni_stark::StarkGenericConfig;

    #[test]
    fn gpu_poseidon2_equals_cpu_and_grinds() {
        if !crate::gpu_available() {
            eprintln!("no GPU; skipped");
            return;
        }
        let perm = p3_koala_bear::default_koalabear_poseidon2_16();
        let mut states: Vec<[Val; 16]> = (0..100u64)
            .map(|t| core::array::from_fn(|i| Val::from_u64(t * 1_000_003 + i as u64 * 7919 + t * t)))
            .collect();
        let want: Vec<[Val; 16]> = states.iter().map(|s| perm.permute(*s)).collect();
        permute_states(&mut states).unwrap();
        assert_eq!(states, want, "GPU Poseidon2 differs from the CPU");
        // the hooked grind gives witnesses the CPU check accepts, for every buffer fill
        umis_stark::config::koala_bear_keccak::set_grinder(grind);
        let cfg = umis_stark::config::koala_bear_keccak::config(umis_stark::config::FriPreset::Benchmark);
        for fill in 0..8u64 {
            let mut ch = cfg.initialise_challenger();
            for i in 0..8 + fill {
                ch.observe(Val::from_u64(i * 31 + 5));
            }
            let mut cpu = ch.clone();
            // the GPU search itself finds a witness the CPU accepts (no fallback)
            let (base, widx) = ch.grind_base();
            let gw = grind(&base, widx, 16).expect("GPU witness");
            assert!(ch.clone().check_witness(16, gw), "fill {fill}: GPU witness rejected");
            let w = ch.grind(16);
            assert_eq!(w, gw);
            assert!(cpu.check_witness(16, w), "fill {fill}: witness rejected");
            // and the transcripts agree afterwards
            assert_eq!(format!("{:?}", ch.0.sponge_state), format!("{:?}", cpu.0.sponge_state));
        }
    }
}
