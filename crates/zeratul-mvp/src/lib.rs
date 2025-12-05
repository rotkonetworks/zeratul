//! Zeratul - Workload-Agnostic Verification Layer
//!
//! A JAM-inspired blockchain where browser clients run arbitrary workloads
//! and Zeratul just verifies proofs and accumulates results.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                         BROWSER CLIENTS                             │
//! │  (Anyone can run computation - "Refine" phase happens here)         │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐                │
//! │  │Service A│  │Service B│  │Service C│  │ ...     │                │
//! │  │(rollup) │  │(compute)│  │(oracle) │  │         │                │
//! │  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘                │
//! │       │            │            │            │                      │
//! │       ▼            ▼            ▼            ▼                      │
//! │  ┌─────────────────────────────────────────────────┐               │
//! │  │         WorkPackages + Ligerito Proofs          │               │
//! │  └─────────────────────────────────────────────────┘               │
//! └─────────────────────────────────────────────────────────────────────┘
//!                              │
//!                              ▼
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                      ZERATUL CHAIN                                  │
//! │           (Verification + Accumulation Layer)                       │
//! ├─────────────────────────────────────────────────────────────────────┤
//! │                                                                     │
//! │  1. Verify proofs (Ligerito - fast, ~50ms)                         │
//! │  2. Accumulate results into service states                          │
//! │  3. 2/3+1 validators agree → Finality                              │
//! │                                                                     │
//! │  NO RE-EXECUTION - proofs make computation trustless               │
//! │  NO LEADER REQUIRED - anyone can propose valid blocks              │
//! │                                                                     │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Key Differences from Traditional Blockchains
//!
//! - **No built-in logic**: No transfers, staking, etc. - services define their own
//! - **Leaderless consensus**: Proofs are verifiable by anyone, no leader trust
//! - **Browser-native**: Computation happens in browsers, not on validators
//! - **2D ZODA DA**: 2D data availability instead of JAM's 1D erasure coding
//!
//! # Key Difference from JAM
//!
//! - JAM: 341 fixed validator cores run "Refine"
//! - Zeratul: Unlimited browser clients run "Refine"
//! - Both: Chain does "Accumulate" (verify + finalize)

pub mod types;
pub mod state;
pub mod accumulator;
pub mod prover;
pub mod consensus;
pub mod node;
pub mod da;
pub mod service_registry;

#[cfg(feature = "networking")]
pub mod network;

pub use types::*;
pub use state::State;
pub use accumulator::Accumulator;
pub use prover::BlockProver;
pub use consensus::InstantBFT;
pub use da::{Zoda, ZodaCommitment, ZodaConfig, ZodaMatrix, Shard};
pub use service_registry::{ServiceRegistry, ServiceMetadata, RegistryError};

#[cfg(feature = "networking")]
pub use network::{NetworkService, NetworkConfig, NetworkMessage, NetworkEvent, NetworkError};

/// Block time in milliseconds
pub const BLOCK_TIME_MS: u64 = 1000;

/// Proof verification budget in milliseconds
pub const VERIFY_BUDGET_MS: u64 = 100;

/// Proof generation budget (for block proof) in milliseconds
pub const PROOF_BUDGET_MS: u64 = 500;

/// Network propagation budget in milliseconds
pub const NETWORK_BUDGET_MS: u64 = 300;

/// Maximum work results per block
pub const MAX_RESULTS_PER_BLOCK: usize = 256;

// Legacy constant for backward compat
#[doc(hidden)]
pub const MAX_TXS_PER_BLOCK: usize = MAX_RESULTS_PER_BLOCK;
#[doc(hidden)]
pub const EXEC_BUDGET_MS: u64 = VERIFY_BUDGET_MS;

/// Validator threshold for finality (2/3 + 1)
///
/// With n validators, need at least `(n * 2 / 3) + 1` votes for finality.
/// This ensures Byzantine fault tolerance.
pub fn finality_threshold(n: usize) -> usize {
    (n * 2 / 3) + 1
}
