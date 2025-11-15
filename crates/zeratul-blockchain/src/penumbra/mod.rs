//! Penumbra Integration Module
//!
//! This module provides integration with Penumbra blockchain for:
//! - Oracle price feeds from Penumbra DEX batch swaps
//! - IBC asset transfers (Penumbra ↔ Zeratul)
//! - Light client verification of Penumbra state
//!
//! ## Architecture
//!
//! Each validator runs an embedded Penumbra light client (pclientd) that:
//! 1. Syncs with Penumbra network (~1GB storage)
//! 2. Queries batch swap prices for oracle feed
//! 3. Verifies IBC packet proofs
//! 4. Enables trustless cross-chain integration
//!
//! ## Security Model
//!
//! - Validators trust Penumbra's validator set (via light client)
//! - Oracle prices use median of all validator proposals (Byzantine resistant)
//! - IBC transfers verified via Merkle proofs
//! - No single point of failure

pub mod light_client;
pub mod oracle;
pub mod ibc;

pub use light_client::*;
pub use oracle::*;
pub use ibc::*;
