//! PolkaVM Polynomial Commitment Verification
//!
//! This crate provides constraint system and proving infrastructure for PolkaVM execution traces
//! using the Ligerito polynomial commitment scheme over binary fields.
//!
//! ## Features
//!
//! - **PolkaVM Integration**: Constraint generation for RISC-V instruction execution
//! - **State Continuity**: Cryptographic enforcement of execution chaining
//! - **Merkle Memory**: Authenticated memory via Merkle trees
//! - **Batched Constraints**: Schwartz-Zippel batching for efficient verification
//!
//! ## Architecture
//!
//! The library supports two execution models:
//!
//! ### 1. Register-only execution (Phase 1)
//! Simple register-based computations without memory
//!
//! ### 2. Full PolkaVM execution (Phase 2+)
//! Complete RISC-V instruction set with:
//! - 13 registers (a0-a7, t0-t2, sp, ra, zero)
//! - Merkle-authenticated memory
//! - State continuity constraints
//! - Windowed proving for continuous execution
//!
//! ## Usage
//!
//! Enable the `polkavm-integration` feature for full PolkaVM support:
//!
//! ```toml
//! [dependencies]
//! polkavm-pcvm = { version = "0.1", features = ["polkavm-integration"] }
//! ```

pub mod trace;
pub mod arithmetization;
pub mod constraints;
pub mod poseidon;
pub mod memory;
pub mod memory_merkle;
pub mod integration;
pub mod host_calls;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_adapter;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_tracer;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_constraints;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_arithmetization;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_prover;

pub use trace::{RegisterOnlyTrace, RegisterOnlyStep, Opcode};
pub use arithmetization::arithmetize_register_trace;
