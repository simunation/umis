//! Browser (wasm) simunation node.
//!
//! `SimNode` runs a full protocol participant in the page: its own keypair
//! and libp2p Swarm (WebRTC-direct to native nodes, relay circuits to other
//! browsers), the sans-IO protocol core and the shared runtime loop. Several
//! can run in one page; all Swarms live on the main thread.
//!
//! STARK proving jobs go to Web Workers (`setProver` + `proveJob`).
//!
//! `SpikeNode` is the M1 connectivity spike, kept as a network diagnostic.

#[cfg(target_arch = "wasm32")]
mod prover;
#[cfg(target_arch = "wasm32")]
mod simnode;
#[cfg(target_arch = "wasm32")]
mod spike;
