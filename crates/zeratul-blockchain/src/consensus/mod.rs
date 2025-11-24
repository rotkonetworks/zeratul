//! Consensus Module
//!
//! Leader selection and block production using BLS12-381 Timelock Encryption

pub mod entropy;
pub mod leader_selection;
pub mod block_verifier;

pub use entropy::EntropyAccumulator;
pub use leader_selection::{LeaderSelection, LeaderConfig};
pub use block_verifier::BlockVerifier;
