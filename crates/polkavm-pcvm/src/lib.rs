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
//! ## Security
//!
//! This crate provides two versions of cryptographic primitives:
//!
//! ### Secure (128-bit field, 64-bit security)
//! - [`rescue`]: Rescue-Prime hash with x^(-1) sbox, SHAKE-256 round constants, verified MDS
//! - [`merkle128`]: Merkle tree using Rescue-Prime over GF(2^128)
//! - [`unified_memory128`]: Authenticated memory with 128-bit merkle proofs
//!
//! ### Deprecated (32-bit field, INSECURE)
//! - [`poseidon`]: **DEPRECATED** - Uses x^5 sbox which is not a permutation in binary fields
//! - [`memory_merkle`]: **DEPRECATED** - Uses insecure poseidon hash
//! - [`unified_memory`]: **DEPRECATED** - Uses insecure memory_merkle
//!
//! Always use the 128-bit versions for production systems.
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

// Secure cryptographic primitives (128-bit, 64-bit security)
pub mod rescue;
pub mod merkle128;
pub mod unified_memory128;

// DEPRECATED: Insecure 32-bit versions - DO NOT USE IN PRODUCTION
// These use x^5 sbox which is not a permutation in binary fields
#[deprecated(since = "0.2.0", note = "use rescue module instead - poseidon uses insecure x^5 sbox")]
pub mod poseidon;
#[deprecated(since = "0.2.0", note = "use merkle128 module instead - uses insecure poseidon")]
pub mod memory_merkle;
#[deprecated(since = "0.2.0", note = "use unified_memory128 module instead - uses insecure merkle")]
pub mod unified_memory;

pub mod memory;
pub mod integration;
pub mod host_calls;
pub mod sumcheck;
pub mod trace_opening;
pub mod evaluation_proof;

#[cfg(feature = "polkavm-integration")]
pub mod prover;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_adapter;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_tracer;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_constraints;

#[cfg(feature = "polkavm-integration")]
pub mod polkavm_arithmetization;

pub use trace::{
    RegisterOnlyTrace, RegisterOnlyStep, Opcode, Instruction, Program,
    execute_and_trace, execute_and_trace_with_proofs, ProvenTrace, program_to_bytes,
};
pub use arithmetization::arithmetize_register_trace;

// Export secure 128-bit versions as the default
pub use unified_memory128::{UnifiedMemory128, InstructionFetch128, InstructionFetchConstraint128};
pub use merkle128::{MerkleTree128, MerkleProof128};

// Re-export deprecated types for backwards compatibility (with deprecation warnings)
#[allow(deprecated)]
pub use unified_memory::{UnifiedMemory, InstructionFetch, InstructionFetchConstraint};
#[allow(deprecated)]
pub use memory_merkle::{MemoryMerkleTree, MerkleProof};
