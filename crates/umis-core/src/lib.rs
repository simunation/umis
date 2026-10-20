//! Core of simunation: ids, keccak256, the bmt-js port, the segment-aligned
//! `seg` codec for committed records, tick/slot arithmetic, genesis, and
//! ed25519 signing.

pub mod bmt;
pub mod crypto;
pub mod genesis;
pub mod hash;
pub mod records;
pub mod seg;
pub mod time;

pub use crypto::{NodeId, SecretKey, Signature};
pub use hash::{Hash, keccak256, keccak256_concat};
pub use seg::{SegRecord, ShortStr};

/// Zone id: genesis zones first, then each created zone gets the next id.
pub type ZoneId = u32;
