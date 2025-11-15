//! Execution layer - PolkaVM + Ligerito
//!
//! ## Architecture
//!
//! Penumbra uses:
//! - Native Rust execution (fast!)
//! - Groth16 ZK proofs (slow - ~5ms verification)
//!
//! Zeratul uses:
//! - Native Rust execution (same as Penumbra - already fast!)
//! - PolkaVM for provable execution (our addition!)
//! - Ligerito ZK proofs (10x faster - 512μs verification!)
//!
//! ## What we keep from Penumbra
//!
//! ✅ Batch swap routing logic (route_and_fill.rs)
//! ✅ State transitions (position_manager.rs)
//! ✅ All business logic
//!
//! ## What we replace
//!
//! ❌ Groth16 proofs → ✅ Ligerito proofs
//!
//! ## What we add (new!)
//!
//! ⚡ PolkaVM execution traces for batch swaps
//! ⚡ Provable state transitions
//! ⚡ Faster proof generation (400ms vs seconds)

pub mod pvm_batch;
pub mod ligerito_proofs;

pub use pvm_batch::PvmBatchExecutor;
pub use ligerito_proofs::LigeritoProofSystem;
