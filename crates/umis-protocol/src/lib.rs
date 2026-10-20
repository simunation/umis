//! The sans-IO simunation node.
//!
//! `Node::handle(now_ms, Event) -> Vec<Action>`: libp2p, wasm, the clock and
//! compute workers are thin adapters around it, and the same core runs in
//! the deterministic simulated network (`umis-simnet`).

pub mod beacon;
pub mod ctx;
pub mod exec;
pub mod node;
pub mod ranks;
pub mod types;
pub mod zone;

pub use ctx::Ctx;
pub use node::{Action, Adversary, Command, Event, Node, NodeConfig, UiEvent, Validation, VotePolicy};
pub use types::Topic;
