//! The beacon chain: state transition and block tree.

pub mod chain;
pub mod state;

pub use chain::{BeaconChain, BeaconEntry, BranchView, DepthK, FinalityRule, InsertOutcome};
pub use state::{
    BeaconState, ChainView, CommitInfo, CommitStatus, NodeKind, NodeStatus, Proposal, ProposalStatus, Reject,
    RollbackRecord, VResult, ZoneEntry,
};
