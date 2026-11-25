//! Shared Plonky3 configuration and LogUp helpers.
//!
//! - [`config`]: Goldilocks / KoalaBear STARK configs (Poseidon2 MMCS, two-adic FRI).
//! - [`security`]: proven security of a proof shape under a preset (p3-security), INV-PARAM-3.
//! - [`toy`]: M7a spike, a 1D stencil AIR exercising batch-STARK, a LogUp memory bus, range-check
//!   lookups and public inputs.

#![no_std]

extern crate alloc;

pub mod config;
/// GPU-ready Merkle MMCS (native only: it parallelises with rayon).
#[cfg(not(target_arch = "wasm32"))]
pub mod fast_mmcs;
pub mod security;
pub mod toy;
