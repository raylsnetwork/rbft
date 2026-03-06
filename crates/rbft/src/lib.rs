// SPDX-License-Identifier: Apache-2.0
//! QBFT Consensus Protocol Implementation
//!
//! This crate provides a complete implementation of the QBFT (QBFT Byzantine Fault Tolerant)
//! consensus algorithm as specified in the formal Dafny specification.
//!
//! The implementation includes:
//! - Core QBFT node implementation
//! - Message types and handling
//! - Cryptographic signatures compatible with Ethereum
//! - State management and transitions
//! - Block and blockchain abstractions

pub mod node;
pub mod node_auxilliary_functions;
pub mod types;

#[cfg(test)]
mod tests;

// Re-export commonly used types for convenience
pub use types::{
    Address, Block, Blockchain, Commit, NodeState, Prepare, Proposal, ProposalJustification,
    QbftMessage, RoundChange, Signature,
};
