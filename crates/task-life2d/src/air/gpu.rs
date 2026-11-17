//! GPU-resident prover for advancing life2d slots (feature `cuda`): the generic
//! [`GpuStarkProver`] plus life2d's trace kernels. The result is the CPU Keccak
//! configuration's [`Proof`]: the CPU verifier checks it unchanged.

use super::{LifeAir, Shape, Val};
use p3_field::Field;
use p3_matrix::Matrix;
use p3_matrix::dense::RowMajorMatrix;
use p3_uni_stark::Proof;
use umis_gpu::prover::{GpuInstance, GpuStarkProver};
use umis_gpu::resident;
use umis_stark::config::FriPreset;
use umis_stark::config::koala_bear_keccak::Config;

/// Proves life2d claims on the GPU ([`GpuStarkProver`]), producing
/// `koala_bear_keccak::Config` proofs. Life adds only its trace kernels (the trace is
/// generated on the device from the extended board); the quotient kernel is compiled
/// from `LifeAir`'s symbolic constraints.
pub struct GpuProver {
    inner: GpuStarkProver,
}

/// Cache key of a shape's constraint program.
fn shape_key(s: &Shape) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    ("life2d", s).hash(&mut h);
    h.finish()
}

impl GpuProver {
    pub fn new(preset: FriPreset) -> Self {
        Self { inner: GpuStarkProver::new(preset) }
    }

    /// Proves a host trace (tests; the node path is [`Self::prove_ext`]).
    pub fn prove(&self, air: &LifeAir, trace: RowMajorMatrix<Val>, public: &[Val]) -> Result<Proof<Config>, String> {
        let tree = self.inner.commit_trace(&trace)?;
        self.prove_committed(air, tree, trace.height(), public)
    }

    /// Proves an advancing slot from its extended board: the trace is generated,
    /// extended and committed on the GPU; only generation `k` comes back for the claim.
    pub fn prove_ext(
        &self,
        shape: &Shape,
        params_hash: &umis_core::Hash,
        slot: u64,
        ext: &crate::board::Board,
        absent: &[bool; 8],
    ) -> Result<Proof<Config>, String> {
        let (air, lde, public) = self.claim_on_gpu(shape, params_hash, slot, ext, absent)?;
        let tree = resident::commit_mats(vec![lde])?;
        self.prove_committed(&air, tree, shape.height, &public)
    }

    /// One batch STARK (`p3_batch_stark`, INV-LIFE-BATCH-1) over several advancing
    /// slots of one shape, `(params hash, slot, extended board, absent bands)` each.
    pub fn prove_ext_batch(
        &self,
        shape: &Shape,
        claims: &[(umis_core::Hash, u64, &crate::board::Board, [bool; 8])],
    ) -> Result<p3_batch_stark::BatchProof<Config>, String> {
        let t0 = std::time::Instant::now();
        use p3_maybe_rayon::prelude::*;
        // Claims in parallel: their CPU parts overlap, the GPU parts take turns.
        let parts = claims
            .par_iter()
            .map(|(params_hash, slot, ext, absent)| self.claim_on_gpu(shape, params_hash, *slot, ext, absent))
            .collect::<Result<Vec<_>, _>>()?;
        if std::env::var_os("UMIS_GPU_PROFILE").is_some() {
            eprintln!("  life claims ({}): {:?}", claims.len(), t0.elapsed());
        }
        let key = shape_key(shape);
        let mut airs = Vec::with_capacity(parts.len());
        let mut rest = Vec::with_capacity(parts.len());
        for (air, lde, public) in parts {
            airs.push(air);
            rest.push((lde, public));
        }
        let instances =
            airs.iter().zip(rest).map(|(air, (lde, public))| GpuInstance { air, key, lde, public }).collect();
        self.inner.prove_batch(instances, shape.height)
    }

    /// The claim of an advancing slot with its trace LDE on the device.
    fn claim_on_gpu(
        &self,
        shape: &Shape,
        params_hash: &umis_core::Hash,
        slot: u64,
        ext: &crate::board::Board,
        absent: &[bool; 8],
    ) -> Result<(LifeAir, resident::DevMat, Vec<Val>), String> {
        let s = shape;
        let mut columns = vec![(0u8, 0u8, 0u8, 0u16); s.width()];
        for i in 0..s.rpr {
            for x in 0..s.ws {
                columns[s.col0(i, x)] = (0, 0, i as u8, x as u16);
            }
        }
        for t in 1..=s.k {
            for i in 0..s.rpr {
                for x in t..s.ws - t {
                    let c = s.col(t, i, x);
                    columns[c] = (1, t as u8, i as u8, x as u16);
                    columns[c + 1] = (2, t as u8, i as u8, x as u16);
                }
            }
        }
        let band_absent: [bool; 9] = std::array::from_fn(|b| super::band_absent(absent, b / 3, b % 3));
        let ts = resident::LifeTraceShape {
            n: s.n as u32,
            k: s.k as u32,
            strips: s.strips as u32,
            rpr: s.rpr as u32,
            w: s.w as u32,
            ws: s.ws as u32,
            y: s.y as u32,
            height: s.height as u32,
            columns,
            band_absent,
        };
        let t0 = std::time::Instant::now();
        let (lde, gk) = resident::life_trace_lde(&ts, &ext.cells, ext.w, self.inner.log_blowup(), Val::GENERATOR)?;
        let t1 = t0.elapsed();
        let mut out = crate::board::Board::dead(s.n, s.n);
        for st in 0..s.strips {
            for yy in s.k..s.k + s.n {
                for x in 0..s.w {
                    out.cells[(yy - s.k) * s.n + st * s.w + x] = gk[(st * s.y + yy) * s.ws + s.k + x];
                }
            }
        }
        let (per, public) = super::claim_data(s, params_hash, slot, ext, absent, &out);
        if std::env::var_os("UMIS_GPU_PROFILE").is_some() {
            eprintln!("  life claim: trace {t1:?}, claim data {:?}", t0.elapsed() - t1);
        }
        Ok((LifeAir { shape: *s, periodic: per }, lde, public))
    }

    fn prove_committed(
        &self,
        air: &LifeAir,
        tree: resident::GpuTree,
        degree: usize,
        public: &[Val],
    ) -> Result<Proof<Config>, String> {
        self.inner.prove_committed(air, shape_key(&air.shape), tree, degree, public)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{claim_data, generate_trace};
    use super::*;
    use crate::Life2dParams;
    use crate::board::Board;
    use p3_field::PrimeCharacteristicRing;
    use rand_chacha::ChaCha8Rng;
    use rand_chacha::rand_core::{Rng, SeedableRng};
    use umis_stark::config::koala_bear_keccak::config;

    fn claim_inputs(n: u32, k: u32, absent_mask: u8, seed: u64) -> (Board, [bool; 8]) {
        let e = (n + 2 * k) as usize;
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let mut ext = Board::dead(e, e);
        for c in ext.cells.iter_mut() {
            *c = (rng.next_u32() % 100 < 35) as u8;
        }
        (ext, std::array::from_fn(|i| absent_mask >> i & 1 == 1))
    }

    /// A random advancing claim (extended board, absent halo bands from `absent_mask`).
    fn random_claim(
        n: u32,
        k: u32,
        strips: u32,
        rpr: u32,
        absent_mask: u8,
        seed: u64,
    ) -> (LifeAir, RowMajorMatrix<Val>, Vec<Val>) {
        let mut p = Life2dParams::default_for([1, 1, 0], 4);
        (p.n, p.gens, p.strips, p.rows_per_trace_row) = (n, k, strips, rpr);
        let s = Shape::of(&p);
        let (ext, absent) = claim_inputs(n, k, absent_mask, seed);
        let (trace, out) = generate_trace(&s, &ext, &absent);
        let (per, public) = claim_data(&s, &umis_core::Hash::default(), 3, &ext, &absent, &out);
        (LifeAir { shape: s, periodic: per }, trace, public)
    }

    #[test]
    fn gpu_proofs_verify_with_the_cpu_verifier() {
        if !umis_gpu::gpu_available() {
            eprintln!("no GPU; skipped");
            return;
        }
        let g = GpuProver::new(FriPreset::Benchmark);
        let cpu = config(FriPreset::Benchmark);
        for (n, k, strips, rpr, absent_mask) in
            [(16, 2, 2, 2, 0u8), (64, 4, 4, 2, 0b1010_0101), (128, 8, 4, 2, 0), (256, 8, 8, 3, 0)]
        {
            let (air, trace, public) = random_claim(n, k, strips, rpr, absent_mask, 11);
            let proof = g.prove(&air, trace.clone(), &public).unwrap();
            p3_uni_stark::verify(&cpu, &air, &proof, &public).unwrap_or_else(|e| panic!("n={n}: {e:?}"));
            // the trace generated on the GPU from the extended board gives the same proof claim
            let (ext, absent) = claim_inputs(n, k, absent_mask, 11);
            let p2 = g.prove_ext(&air.shape, &umis_core::Hash::default(), 3, &ext, &absent).unwrap();
            p3_uni_stark::verify(&cpu, &air, &p2, &public).unwrap_or_else(|e| panic!("n={n} (GPU trace): {e:?}"));
            // postcard round trip as the CPU proof type
            let bytes = postcard::to_allocvec(&proof).unwrap();
            let back: Proof<Config> = postcard::from_bytes(&bytes).unwrap();
            p3_uni_stark::verify(&cpu, &air, &back, &public).unwrap();
            // a tampered trace is rejected (the GPU prover does not check constraints)
            let mut bad = trace;
            let w = bad.width();
            bad.values[3 * w + 7] += Val::ONE;
            let forged = g.prove(&air, bad, &public).unwrap();
            assert!(p3_uni_stark::verify(&cpu, &air, &forged, &public).is_err(), "n={n}: tampered trace verified");
            eprintln!("n={n} k={k}: GPU proof verifies ({} KB), tampered trace rejected", bytes.len() / 1024);
        }
        // concurrent provers share the device safely
        let (air, _, public) = random_claim(64, 4, 4, 2, 0b0110_0000, 5);
        let (ext, absent) = claim_inputs(64, 4, 0b0110_0000, 5);
        std::thread::scope(|sc| {
            for _ in 0..4 {
                sc.spawn(|| {
                    for _ in 0..3 {
                        let p = g.prove_ext(&air.shape, &umis_core::Hash::default(), 3, &ext, &absent).unwrap();
                        p3_uni_stark::verify(&cpu, &air, &p, &public).expect("concurrent GPU proof");
                    }
                });
            }
        });
        eprintln!("12 concurrent GPU proofs verify");
    }

    #[test]
    fn gpu_batch_proofs_verify_with_the_cpu_batch_verifier() {
        use p3_batch_stark::{ProverData, verify_batch};
        if !umis_gpu::gpu_available() {
            eprintln!("no GPU; skipped");
            return;
        }
        let g = GpuProver::new(FriPreset::Benchmark);
        let cpu = config(FriPreset::Benchmark);
        for (n, k, strips, rpr, m) in [(16, 1, 2, 2, 2usize), (64, 1, 4, 2, 5), (64, 4, 4, 2, 3), (256, 1, 8, 3, 8)] {
            let masks = [0u8, 0b1010_0101, 0b0110_0000, 0xff, 0b0000_0001];
            let claims: Vec<(Board, [bool; 8], u64)> = (0..m)
                .map(|j| {
                    let (ext, absent) = claim_inputs(n, k, masks[j % masks.len()], 100 + j as u64);
                    (ext, absent, 7 + j as u64)
                })
                .collect();
            let (air0, _, _) = random_claim(n, k, strips, rpr, 0, 1);
            let shape = air0.shape;
            let input: Vec<(umis_core::Hash, u64, &Board, [bool; 8])> =
                claims.iter().map(|(e, a, sl)| (umis_core::Hash::default(), *sl, e, *a)).collect();
            let proof = g.prove_ext_batch(&shape, &input).unwrap();
            // the verifier's claims, derived on the CPU
            let (mut airs, mut publics) = (Vec::new(), Vec::new());
            for (ext, absent, sl) in &claims {
                let (_, out) = generate_trace(&shape, ext, absent);
                let (per, public) = claim_data(&shape, &umis_core::Hash::default(), *sl, ext, absent, &out);
                airs.push(LifeAir { shape, periodic: per });
                publics.push(public);
            }
            let degrees = vec![shape.height.trailing_zeros() as usize; m];
            let pd = ProverData::from_airs_and_degrees(&cpu, &airs, &degrees).unwrap();
            let bytes = postcard::to_allocvec(&proof).unwrap();
            let back = postcard::from_bytes(&bytes).unwrap();
            verify_batch(&cpu, &airs, &back, &publics, &pd.common).unwrap_or_else(|e| panic!("n={n} m={m}: {e:?}"));
            // one member's claim changed: rejected
            let mut bad = publics.clone();
            bad[m - 1][0] += Val::ONE;
            assert!(verify_batch(&cpu, &airs, &back, &bad, &pd.common).is_err(), "n={n}: altered member verified");
            // members swapped: rejected
            let mut sw = publics.clone();
            sw.swap(0, 1);
            assert!(verify_batch(&cpu, &airs, &back, &sw, &pd.common).is_err(), "n={n}: swapped members verified");
            eprintln!(
                "n={n} k={k} m={m}: GPU batch proof verifies ({} KB), altered claims rejected",
                bytes.len() / 1024
            );
            if n == 64 && k == 1 {
                // concurrent batch provers share the device and the CPU pool without deadlock
                std::thread::scope(|sc| {
                    for _ in 0..3 {
                        sc.spawn(|| {
                            for _ in 0..2 {
                                let p = g.prove_ext_batch(&shape, &input).unwrap();
                                verify_batch(&cpu, &airs, &p, &publics, &pd.common).expect("concurrent batch");
                            }
                        });
                    }
                });
            }
        }
    }
}
