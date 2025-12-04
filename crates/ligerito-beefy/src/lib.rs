//! Ligerito BEEFY - Succinct proofs for Polkadot BEEFY finality
//!
//! This crate provides succinct verification of BEEFY (Bridge Efficiency Enabling Finality Yielder)
//! finality proofs using the Ligerito polynomial commitment scheme.
//!
//! # Overview
//!
//! BEEFY is Polkadot's secondary finality gadget that produces BLS-aggregatable signatures
//! on finalized blocks. This is designed for efficient bridge verification.
//!
//! With Ligerito, we can create constant-size proofs that verify:
//! 1. The aggregated BLS signature is valid
//! 2. The signers represent >2/3 of the total stake
//! 3. The commitment (block hash + validator set) is correctly formed
//!
//! # Security Properties
//!
//! Ligerito provides:
//! - **Soundness**: Cannot forge proofs for invalid statements
//! - **Witness Indistinguishability (WI)**: Different valid witnesses produce
//!   computationally indistinguishable proofs
//!
//! Note: Ligerito is NOT full zero-knowledge. The aggregate BLS signature may leak
//! information about which validators signed. For BEEFY, this is acceptable since
//! validator signatures are typically public on-chain anyway.
//!
//! # Architecture
//!
//! ```text
//! BEEFY SignedCommitment
//!         │
//!         ▼
//! ┌─────────────────────┐
//! │  BeefyWitness       │  (aggregated BLS sig + bit vector)
//! └─────────────────────┘
//!         │
//!         ▼
//! ┌─────────────────────┐
//! │  PolkaVM Verifier   │  (verification logic)
//! └─────────────────────┘
//!         │
//!         ▼
//! ┌─────────────────────┐
//! │  Ligerito Prover    │  (ZK proof generation)
//! └─────────────────────┘
//!         │
//!         ▼
//! ┌─────────────────────┐
//! │  Constant-size      │  (~150 KB proof)
//! │  Finality Proof     │
//! └─────────────────────┘
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use ligerito_beefy::{BeefyWitness, verify_finality, prove_finality};
//!
//! // Create witness from BEEFY signed commitment
//! let witness = BeefyWitness::from_signed_commitment(&signed_commitment, &authority_set);
//!
//! // Verify (for testing)
//! assert!(verify_finality(&witness));
//!
//! // Generate ZK proof
//! let proof = prove_finality(&witness)?;
//!
//! // Verify proof (constant time, no BLS verification needed)
//! assert!(verify_finality_proof(&proof));
//! ```

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

pub mod types;
pub mod verifier;
pub mod bls;

#[cfg(feature = "std")]
pub mod prover;

// Re-exports
pub use types::*;
pub use verifier::*;
