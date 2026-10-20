//! Shared Plonky3 configuration and LogUp helpers.
//!
//! - [`config`]: Goldilocks / KoalaBear STARK configs (Poseidon2 MMCS, two-adic FRI).
//! - [`toy`]: M7a spike, a 1D stencil AIR exercising batch-STARK, a LogUp memory bus, range-check
//!   lookups and public inputs.

#![no_std]

extern crate alloc;

pub mod config;
pub mod toy;
