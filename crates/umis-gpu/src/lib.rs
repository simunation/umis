//! CUDA acceleration for the KoalaBear STARK provers (feature `cuda`).
//!
//! [`GpuDft`] implements Plonky3's `TwoAdicSubgroupDft` for KoalaBear: the coset
//! LDE of trace and quotient matrices (the PCS's `commit`) runs on the GPU,
//! everything else on the CPU (`Radix2DitParallel`). Field arithmetic matches
//! Plonky3 bit for bit (canonical Montgomery form), so proofs are identical in
//! format and verified by the CPU configuration.
//!
//! The device is the first GPU whose name contains `UMIS_GPU` (default `A6000`),
//! else device 0. Matrices below `UMIS_GPU_MIN` elements (default 2^18) stay on
//! the CPU, where transfers would cost more than they save.

#[cfg(feature = "cuda")]
mod lde;
#[cfg(feature = "cuda")]
pub use lde::{GpuDft, gpu_available};
#[cfg(feature = "cuda")]
pub mod jit;
#[cfg(feature = "cuda")]
pub mod pow;
#[cfg(feature = "cuda")]
pub mod prover;
#[cfg(feature = "cuda")]
pub mod resident;
